//! A real PTY, not a command/output text box. All I/O happens off the GUI thread.
use crate::{ssh::Remote, theme::Palette};
use anyhow::{Context, Result};
use eframe::egui::{self, Color32, FontId, Key, Rect, Sense, Stroke, Vec2};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use std::{
    io::{BufReader, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
mod backend;
mod session;
mod view;
use backend::{Backend, Process, Protocol};
mod colors;
#[cfg(test)]
mod fixtures;
mod framing;
mod selection;
#[cfg(test)]
mod selection_tests;

/// Native responses and painted cell bounds from one terminal pass. UI adapters
/// can observe these without repeating hit testing or guessing font metrics.
pub(crate) struct TerminalView {
    pub response: egui::Response,
    pub error: Option<String>,
    pub first_cell: Rect,
    pub copy_selection: Option<egui::Response>,
}

/// One native terminal view, backed by a directly owned PTY, a persistent-service attachment,
/// or a debugger stream. I/O and parsing run on workers; drop cleans up only this view's owned
/// transport.
pub struct Terminal {
    parser: Arc<Mutex<vt100::Parser>>,
    backend: Backend,
    /// Shared liveness flag updated by the worker when the backing stream ends.
    pub alive: Arc<AtomicBool>,
    size: (u16, u16),
    /// User-visible terminal title.
    pub title: String,
    /// Interpreter selected when this shell was created, for display/activation context.
    pub python: Option<String>,
    visible: Arc<AtomicBool>,
    contrast: colors::Contrast,
    selection: Option<selection::Selection>,
}

/// Control messages sent from a debugger terminal view to its owning coordinator.
pub enum StreamControl {
    /// Raw terminal input bytes.
    Input(Vec<u8>),
    /// PTY size in rows then columns.
    Resize(u16, u16),
    /// Explicitly end this debug terminal rather than a project session.
    End,
}
/// Producer handle for feeding a debugger-owned terminal's bounded parser stream.
pub struct Stream {
    output: mpsc::SyncSender<Vec<u8>>,
    alive: Arc<AtomicBool>,
}
impl Stream {
    /// Try to enqueue output without blocking; on backpressure the caller retains/retries the
    /// returned bytes.
    pub fn feed(&self, bytes: Vec<u8>) -> std::result::Result<(), mpsc::TrySendError<Vec<u8>>> {
        self.output.try_send(bytes)
    }
    /// Mark the producer's terminal stream ended.
    pub fn end(&self) {
        self.alive.store(false, Ordering::Release);
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.end();
    }
}

impl Terminal {
    /// Return whether this view's backing stream is still alive.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Shell transports own an attachment/process; debugger streams are stopped
    /// through their coordinator and must not block project cleanup as shells.
    pub(crate) fn owns_shell(&self) -> bool {
        matches!(self.backend, Backend::Pty { .. } | Backend::Native { .. })
    }

    /// Retain bounded parsed output without retaining the PTY, input writer or
    /// process ownership. Late reader output remains visible after the original
    /// terminal is dropped, so an SSH exit cannot race away its final error.
    pub(crate) fn output_view(&self, title: String) -> Self {
        Self {
            parser: self.parser.clone(),
            backend: Backend::Output,
            alive: Arc::new(AtomicBool::new(false)),
            size: self.size,
            title,
            python: None,
            visible: self.visible.clone(),
            contrast: Default::default(),
            selection: None,
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.backend.resize(rows, cols)?;
        if !self.is_native()
            && let Ok(mut parser) = self.parser.lock()
        {
            parser.screen_mut().set_size(rows, cols);
        }
        self.size = (rows, cols);
        self.selection = None;
        Ok(())
    }

    /// Copy the parser's visible screen contents without escape sequences.
    pub fn contents(&self) -> String {
        self.parser
            .lock()
            .map(|p| p.screen().contents())
            .unwrap_or_default()
    }

    /// Copy selected text, falling back to the visible screen when no range is selected.
    pub fn copy_contents(&self) -> String {
        self.parser
            .lock()
            .map(|parser| {
                self.selection
                    .as_ref()
                    .and_then(|selection| selection.text(parser.screen()))
                    .unwrap_or_else(|| parser.screen().contents())
            })
            .unwrap_or_default()
    }

    /// Queue raw PTY input without blocking; a full/disconnected input queue returns an error.
    pub fn send(&self, bytes: Vec<u8>) -> Result<()> {
        anyhow::ensure!(
            bytes.len() <= 1024 * 1024,
            "Terminal paste exceeds 1 MiB. Paste a smaller chunk."
        );
        self.backend.send(bytes)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let alive = self.is_alive();
        self.backend.close(alive);
    }
}

/// Encode shell keys, including application-cursor mode and control modifiers;
/// unsupported/editor-only keys return `None`.
pub fn key_bytes(key: Key, modifiers: egui::Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    if modifiers.ctrl && matches!(key, Key::ArrowLeft | Key::ArrowRight) {
        // Modified cursor keys use xterm CSI parameters even in application
        // cursor mode. Passing a plain arrow loses readline's word movement.
        let modifier = 5 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt);
        let direction = if key == Key::ArrowLeft { 'D' } else { 'C' };
        return Some(format!("\x1b[1;{modifier}{direction}").into_bytes());
    }
    if modifiers.ctrl && !modifiers.alt && !modifiers.shift {
        if key == Key::Space {
            return Some(vec![0]);
        }
        let letter = match key {
            Key::A => b'a',
            Key::B => b'b',
            Key::C => b'c',
            Key::D => b'd',
            Key::E => b'e',
            Key::F => b'f',
            Key::G => b'g',
            Key::H => b'h',
            Key::I => b'i',
            Key::J => b'j',
            Key::K => b'k',
            Key::L => b'l',
            Key::M => b'm',
            Key::N => b'n',
            Key::O => b'o',
            Key::P => b'p',
            Key::Q => b'q',
            Key::R => b'r',
            Key::S => b's',
            Key::T => b't',
            Key::U => b'u',
            Key::V => b'v',
            Key::W => b'w',
            Key::X => b'x',
            Key::Y => b'y',
            Key::Z => b'z',
            Key::OpenBracket => b'[',
            Key::Backslash => b'\\',
            Key::CloseBracket => b']',
            _ => 0,
        };
        if letter != 0 {
            return Some(vec![letter & 0x1f]);
        }
    }
    let prefix = if app_cursor { "\x1bO" } else { "\x1b[" };
    let value = match key {
        Key::Enter => "\r".into(),
        Key::Backspace => "\x7f".into(),
        Key::Tab if modifiers.shift => "\x1b[Z".into(),
        Key::Tab => "\t".into(),
        Key::Escape => "\x1b".into(),
        Key::ArrowUp => format!("{prefix}A"),
        Key::ArrowDown => format!("{prefix}B"),
        Key::ArrowRight => format!("{prefix}C"),
        Key::ArrowLeft => format!("{prefix}D"),
        Key::Home => format!("{prefix}H"),
        Key::End => format!("{prefix}F"),
        Key::Delete => "\x1b[3~".into(),
        Key::Insert => "\x1b[2~".into(),
        Key::PageUp => "\x1b[5~".into(),
        Key::PageDown => "\x1b[6~".into(),
        Key::F1 => "\x1bOP".into(),
        Key::F2 => "\x1bOQ".into(),
        Key::F3 => "\x1bOR".into(),
        Key::F4 => "\x1bOS".into(),
        Key::F5 => "\x1b[15~".into(),
        Key::F6 => "\x1b[17~".into(),
        Key::F7 => "\x1b[18~".into(),
        Key::F8 => "\x1b[19~".into(),
        Key::F9 => "\x1b[20~".into(),
        Key::F10 => "\x1b[21~".into(),
        Key::F11 => "\x1b[23~".into(),
        Key::F12 => "\x1b[24~".into(),
        _ => return None,
    };
    Some(value.into_bytes())
}

/// Resolve a vt100 default/indexed/RGB color through the active terminal palette.
pub fn ansi_color(color: vt100::Color, default: Color32, ansi: &[Color32; 16]) -> Color32 {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        vt100::Color::Idx(index) => {
            if index < 16 {
                ansi[index as usize]
            } else if index >= 232 {
                Color32::from_gray(8 + (index - 232) * 10)
            } else {
                let n = index - 16;
                let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
                Color32::from_rgb(level(n / 36), level((n / 6) % 6), level(n % 6))
            }
        }
    }
}

#[cfg(test)]
mod lifecycle_tests;

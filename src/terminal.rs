//! A real PTY, not a command/output text box. All I/O happens off the GUI thread.
use crate::{ssh::Remote, theme::Palette};
use anyhow::{Context, Result};
use eframe::egui::{self, Color32, FontId, Key, Rect, Sense, Stroke, Vec2};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
mod colors;

const SHELL_KEYS: egui::EventFilter = egui::EventFilter {
    tab: true,
    horizontal_arrows: true,
    vertical_arrows: true,
    escape: true,
};

/// One native terminal view, backed by a directly owned PTY, a persistent-service attachment,
/// or a debugger stream. I/O and parsing run on workers; drop cleans up only this view's owned
/// transport.
pub struct Terminal {
    parser: Arc<Mutex<vt100::Parser>>,
    input: mpsc::SyncSender<Vec<u8>>,
    master: Option<Box<dyn MasterPty + Send>>,
    killer: Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    control: Option<Box<dyn Fn(StreamControl) + Send + Sync>>,
    /// Shared liveness flag updated by the worker when the backing stream ends.
    pub alive: Arc<AtomicBool>,
    size: (u16, u16),
    /// User-visible terminal title.
    pub title: String,
    /// Whether this view uses the native framed terminal/session transport.
    pub native: bool,
    persistent: Arc<AtomicBool>,
    /// Interpreter selected when this shell was created, for display/activation context.
    pub python: Option<String>,
    visible: Arc<AtomicBool>,
    contrast: colors::Contrast,
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
    /// A project-host debug PTY, carried over the bounded debugger transport.
    pub fn stream(
        ctx: &egui::Context,
        title: String,
        control: impl Fn(StreamControl) + Send + Sync + 'static,
    ) -> (Self, Stream) {
        let parser = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 4000)));
        let alive = Arc::new(AtomicBool::new(true));
        let visible = Arc::new(AtomicBool::new(true));
        let (output, rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let read_parser = parser.clone();
        let read_ctx = ctx.clone();
        let read_visible = visible.clone();
        std::thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if let Ok(mut parser) = read_parser.lock() {
                    parser.process(&bytes);
                }
                if read_visible.load(Ordering::Acquire) {
                    read_ctx.request_repaint_after(Duration::from_millis(16));
                }
            }
        });
        let control = Arc::new(control);
        let input_control = control.clone();
        let (input, rx) = mpsc::sync_channel::<Vec<u8>>(16);
        std::thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                for chunk in bytes.chunks(16384) {
                    input_control(StreamControl::Input(chunk.to_vec()));
                }
            }
        });
        let end_control = control.clone();
        (
            Self {
                parser,
                input,
                master: None,
                killer: None,
                control: Some(Box::new(move |event| end_control(event))),
                alive: alive.clone(),
                size: (24, 80),
                title,
                native: false,
                persistent: Arc::new(AtomicBool::new(false)),
                python: None,
                visible,
                contrast: Default::default(),
            },
            Stream { output, alive },
        )
    }
    /// Start a directly owned local login shell rooted at the supplied directory.
    pub fn local(ctx: &egui::Context, root: &str) -> Result<Self> {
        Self::local_project(
            ctx,
            &crate::sandbox::Plan {
                root: root.into(),
                ..Default::default()
            },
        )
    }

    /// Start a directly owned local project shell using the prepared sandbox plan.
    pub fn local_project(ctx: &egui::Context, plan: &crate::sandbox::Plan) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut command = if plan.active {
            let mut command = CommandBuilder::new(&plan.prefix[0]);
            command.args(&plan.prefix[1..]);
            command.arg(&shell);
            command
        } else {
            CommandBuilder::new(&shell)
        };
        command.cwd(&plan.root);
        Self::spawn(ctx, command, format!("LOCAL / {}", plan.label()))
    }

    /// Start the interactive OpenSSH authentication master in a real local PTY.
    pub fn remote(ctx: &egui::Context, remote: &Remote) -> Result<Self> {
        let mut command = CommandBuilder::new("ssh");
        command.args(remote.master_args());
        Self::spawn(
            ctx,
            command,
            format!("SSH LOGIN / {}", remote.profile.endpoint()),
        )
    }

    /// Start a project shell through the existing authenticated SSH master and sandbox plan.
    pub fn remote_project(
        ctx: &egui::Context,
        remote: &Remote,
        plan: &crate::sandbox::Plan,
    ) -> Result<Self> {
        anyhow::ensure!(
            remote.socket.exists(),
            "SSH connection is offline. Reconnect before starting a terminal."
        );
        let mut command = CommandBuilder::new("ssh");
        command.args(remote.channel_args(true));
        command.arg(remote.shell_command(plan));
        Self::spawn(
            ctx,
            command,
            format!("SSH {} / {}", remote.profile.endpoint(), plan.label()),
        )
    }

    /// Spawn a directly owned PTY command with background I/O and scoped cleanup.
    pub fn spawn(ctx: &egui::Context, command: CommandBuilder, title: String) -> Result<Self> {
        Self::spawn_mode(ctx, command, title, false, false)
    }
    /// Spawn a command using the native framed attachment mode rather than raw terminal output.
    pub fn spawn_native(
        ctx: &egui::Context,
        command: CommandBuilder,
        title: String,
    ) -> Result<Self> {
        Self::spawn_mode(ctx, command, title, true, true)
    }
    /// Attach a view to an existing native host session without restarting its shell or
    /// replaying startup input.
    pub fn native_session(
        ctx: &egui::Context,
        remote: Option<&Remote>,
        reply: &crate::sessions::Reply,
        info: &rode_session::Info,
    ) -> Result<Self> {
        let mut args = reply.prefix.clone();
        args.extend([
            "attach".into(),
            "--framed".into(),
            info.id.clone(),
            info.policy.clone(),
        ]);
        let mut command = if let Some(remote) = remote {
            anyhow::ensure!(
                remote.socket.exists(),
                "Reconnect SSH before attaching a terminal"
            );
            let mut command = CommandBuilder::new("ssh");
            command.args(remote.channel_args(true));
            let mut argv = vec![reply.executable.clone()];
            argv.extend(args);
            command.arg(
                argv.iter()
                    .map(|arg| crate::ssh::quote(arg))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            command
        } else {
            let mut command = CommandBuilder::new(&reply.executable);
            command.args(args);
            command
        };
        command.env_remove("TMUX");
        command.env_remove("TMUX_PANE");
        Self::spawn_mode(
            ctx,
            command,
            format!("NATIVE · {} / {}", info.name, info.mode),
            true,
            info.persistent,
        )
    }
    /// Set visibility so hidden output does not request unnecessary GUI repaints.
    pub fn set_visible(&self, visible: bool) {
        self.visible.store(visible, Ordering::Release);
    }
    /// Return the persistent-service lifetime state observed by this native attachment.
    pub fn persistent(&self) -> bool {
        self.native && self.persistent.load(Ordering::Acquire)
    }
    fn spawn_mode(
        ctx: &egui::Context,
        mut command: CommandBuilder,
        title: String,
        native: bool,
        keep_running: bool,
    ) -> Result<Self> {
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let pair = portable_pty::native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        if native && let Some(fd) = pair.master.as_raw_fd() {
            let mut attributes: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(fd, &mut attributes) } == 0 {
                unsafe {
                    libc::cfmakeraw(&mut attributes);
                    // portable-pty's master writer sends newline + VEOF on
                    // drop. This PTY carries a native attach protocol: EOF
                    // bytes would be forwarded as shell input, potentially
                    // exiting a persistent shell when its view disconnects.
                    attributes.c_cc[libc::VEOF] = 0;
                    libc::tcsetattr(fd, libc::TCSANOW, &attributes);
                }
            }
        }
        let mut child = pair
            .slave
            .spawn_command(command)
            .context("Could not start terminal")?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(24, 100, 4000)));
        let alive = Arc::new(AtomicBool::new(true));
        let persistent = Arc::new(AtomicBool::new(keep_running));
        let read_persistent = persistent.clone();
        let visible = Arc::new(AtomicBool::new(true));
        let read_visible = visible.clone();
        let read_parser = parser.clone();
        let read_ctx = ctx.clone();
        let (drained_tx, drained_rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                if native {
                    let (kind, bytes) = match rode_session::read_stream(&mut reader) {
                        Ok(frame) => frame,
                        Err(_) => break,
                    };
                    if let Ok(mut parser) = read_parser.lock() {
                        match kind {
                            b'S' if bytes.len() == 4 => {
                                parser.screen_mut().set_size(
                                    u16::from_be_bytes([bytes[0], bytes[1]]).clamp(2, 200),
                                    u16::from_be_bytes([bytes[2], bytes[3]]).clamp(10, 400),
                                );
                            }
                            b'O' | b'X' => parser.process(&bytes),
                            b'P' if bytes.len() == 1 => {
                                read_persistent.store(bytes[0] != 0, Ordering::Release);
                            }
                            _ => {}
                        }
                    }
                } else {
                    let count = match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if let Ok(mut parser) = read_parser.lock() {
                        parser.process(&buffer[..count]);
                    }
                }
                if read_visible.load(Ordering::Acquire) {
                    read_ctx.request_repaint_after(Duration::from_millis(16));
                }
            }
            let _ = drained_tx.send(());
            if read_visible.load(Ordering::Acquire) {
                read_ctx.request_repaint();
            }
        });
        let wait_alive = alive.clone();
        let wait_ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = child.wait();
            // The attachment process may exit while its final PTY frames are
            // still buffered. Publish completion only after the parser drains
            // them; bound the wait for descendants retaining a slave handle.
            let _ = drained_rx.recv_timeout(Duration::from_secs(1));
            wait_alive.store(false, Ordering::Release);
            wait_ctx.request_repaint();
        });
        let (input, rx) = mpsc::sync_channel::<Vec<u8>>(256);
        std::thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        Ok(Self {
            parser,
            input,
            master: Some(pair.master),
            killer: Some(killer),
            control: None,
            alive,
            size: (24, 100),
            title,
            native,
            persistent,
            python: None,
            visible,
            contrast: Default::default(),
        })
    }

    /// Return whether this view's backing stream is still alive.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Retain bounded parsed output without retaining the PTY, input writer or
    /// process ownership. Late reader output remains visible after the original
    /// terminal is dropped, so an SSH exit cannot race away its final error.
    pub(crate) fn output_view(&self, title: String) -> Self {
        let (input, closed) = mpsc::sync_channel(1);
        drop(closed);
        Self {
            parser: self.parser.clone(),
            input,
            master: None,
            killer: None,
            control: None,
            alive: Arc::new(AtomicBool::new(false)),
            size: self.size,
            title,
            native: false,
            persistent: Arc::new(AtomicBool::new(false)),
            python: None,
            visible: self.visible.clone(),
            contrast: Default::default(),
        }
    }

    /// Return whether the native window and terminal input widget have focus. Do not call
    /// inside an egui `input`/`memory` lock closure.
    pub fn focused(ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.has_focus(egui::Id::new("terminal-input")))
    }

    /// Request terminal input focus after the current egui input barrier permits it.
    pub fn focus(ctx: &egui::Context) {
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("terminal-input")));
        // A command/manager modal blocks background focus for one additional
        // frame. Retry once the terminal layer can receive interaction.
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("terminal-wants-focus"), true));
        ctx.request_repaint();
    }

    /// Prepare shell input before editor shortcuts and widgets handle it.
    /// The caller excludes hidden terminals, dialogs, and open menus.
    pub fn prepare_input(ctx: &egui::Context) {
        let id = egui::Id::new("terminal-input");
        let escape = ctx.input(|input| input.key_pressed(Key::Escape));
        ctx.memory_mut(|memory| {
            if memory.has_focus(id) || (escape && memory.had_focus_last_frame(id)) {
                // egui decides navigation before widgets run, but a fresh focus
                // request cannot install a filter until the following frame.
                // Cancel that pending navigation before it reaches any widget.
                // Escape may already have cleared the terminal's focus.
                if !memory.has_focus(id) {
                    memory.request_focus(id);
                }
                memory.move_focus(egui::FocusDirection::None);
                memory.set_focus_lock_filter(id, SHELL_KEYS);
            }
        });
    }

    /// Copy the parser's visible screen contents without escape sequences.
    pub fn contents(&self) -> String {
        self.parser
            .lock()
            .map(|p| p.screen().contents())
            .unwrap_or_default()
    }

    /// Queue raw PTY input without blocking; a full/disconnected input queue returns an error.
    pub fn send(&self, bytes: Vec<u8>) -> Result<()> {
        anyhow::ensure!(
            bytes.len() <= 1024 * 1024,
            "Terminal paste exceeds 1 MiB. Paste a smaller chunk."
        );
        self.input.try_send(bytes).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => anyhow::anyhow!("Terminal input queue is full. Input was not sent; try again when the shell responds."),
            mpsc::TrySendError::Disconnected(_) => anyhow::anyhow!("Terminal session has ended."),
        })
    }

    /// Render visible terminal cells and process focused shell input. Returns copied output
    /// when requested; ordinary control keys remain shell input outside editor escape routes.
    pub fn ui(&mut self, ui: &mut egui::Ui, font_size: f32, palette: Palette) -> Option<String> {
        let font = FontId::monospace(font_size);
        let char_size =
            ui.fonts_mut(|fonts| Vec2::new(fonts.glyph_width(&font, 'M'), fonts.row_height(&font)));
        let available = ui.available_size();
        let (rect, _) = ui.allocate_exact_size(available.max(Vec2::splat(1.0)), Sense::hover());
        let response = ui.interact(rect, egui::Id::new("terminal-input"), Sense::click());
        if ui.memory(|memory| memory.allows_interaction(ui.layer_id()))
            && ui
                .ctx()
                .data_mut(|data| data.remove_temp::<bool>(egui::Id::new("terminal-wants-focus")))
                .unwrap_or(false)
        {
            response.request_focus();
        }
        if response.clicked() {
            response.request_focus();
        }
        if response.has_focus() {
            ui.memory_mut(|memory| memory.set_focus_lock_filter(response.id, SHELL_KEYS));
        }
        response.clone().on_hover_cursor(egui::CursorIcon::Text);
        let rows = ((rect.height() - 8.0) / char_size.y)
            .floor()
            .clamp(2.0, 200.0) as u16;
        let cols = ((rect.width() - 12.0) / char_size.x)
            .floor()
            .clamp(10.0, 400.0) as u16;
        if self.size != (rows, cols) {
            self.size = (rows, cols);
            if let Some(control) = &self.control {
                control(StreamControl::Resize(rows, cols));
            }
            if let Some(master) = &self.master {
                let _ = master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
            if !self.native
                && let Ok(mut parser) = self.parser.lock()
            {
                parser.screen_mut().set_size(rows, cols);
            }
        }
        let mut bytes_to_send = Vec::new();
        if let Ok(mut parser) = self.parser.lock() {
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll.abs() > 0.1 {
                    let old = parser.screen().scrollback();
                    parser.screen_mut().set_scrollback(
                        (old as isize + (scroll / char_size.y).round() as isize).max(0) as usize,
                    );
                }
            }
            if response.has_focus() && self.is_alive() {
                for event in ui.input(|i| i.events.clone()) {
                    match event {
                        egui::Event::Text(text) => {
                            if ui.input(|i| i.modifiers.alt && !i.modifiers.ctrl) {
                                bytes_to_send.push(0x1b);
                            }
                            bytes_to_send.extend(text.as_bytes());
                        }
                        egui::Event::Paste(text) => {
                            if parser.screen().bracketed_paste() {
                                bytes_to_send.extend(b"\x1b[200~");
                            }
                            if parser.screen().bracketed_paste() {
                                bytes_to_send.extend(text.as_bytes());
                            } else {
                                bytes_to_send.extend(
                                    text.replace("\r\n", "\n").replace('\n', "\r").as_bytes(),
                                );
                            }
                            if parser.screen().bracketed_paste() {
                                bytes_to_send.extend(b"\x1b[201~");
                            }
                        }
                        egui::Event::Copy
                            if ui.input(|i| i.modifiers.shift || i.modifiers.mac_cmd) =>
                        {
                            ui.ctx().copy_text(parser.screen().contents())
                        }
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            if let Some(bytes) =
                                key_bytes(key, modifiers, parser.screen().application_cursor())
                            {
                                bytes_to_send.extend(bytes);
                            }
                        }
                        _ => {}
                    }
                }
                if !bytes_to_send.is_empty() {
                    parser.screen_mut().set_scrollback(0);
                }
            }
            let painter = ui.painter().with_clip_rect(rect);
            painter.rect_filled(rect, 0, palette.terminal_background);
            let origin = rect.min + egui::vec2(6.0, 4.0);
            let screen = parser.screen();
            let focused = response.has_focus();
            let cursor = if self.is_alive() && !screen.hide_cursor() && screen.scrollback() == 0 {
                let (row, col) = screen.cursor_position();
                let mut col = col.min(cols.saturating_sub(1));
                if screen
                    .cell(row, col)
                    .is_some_and(|cell| cell.is_wide_continuation())
                {
                    col = col.saturating_sub(1);
                }
                (row < rows).then_some((row, col))
            } else {
                None
            };
            for row in 0..rows {
                for col in 0..cols {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let pos =
                        origin + egui::vec2(col as f32 * char_size.x, row as f32 * char_size.y);
                    // Bash PS1 commonly uses SGR 1;30 / 1;34. Match xterm's
                    // default boldColors behavior: brighten only the basic
                    // foreground bank, never backgrounds, RGB or indices 8+.
                    let foreground = match cell.fgcolor() {
                        vt100::Color::Idx(index @ 0..=7) if cell.bold() => {
                            vt100::Color::Idx(index + 8)
                        }
                        color => color,
                    };
                    let mut fg = ansi_color(foreground, palette.terminal_foreground, &palette.ansi);
                    let mut bg =
                        ansi_color(cell.bgcolor(), palette.terminal_background, &palette.ansi);
                    if cell.inverse() {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    if focused && cursor == Some((row, col)) {
                        bg = palette.accent;
                        fg = palette.terminal_background;
                    }
                    if bg != palette.terminal_background {
                        painter.rect_filled(
                            Rect::from_min_size(
                                pos,
                                Vec2::new(
                                    char_size.x * if cell.is_wide() { 2.0 } else { 1.0 },
                                    char_size.y,
                                ),
                            ),
                            0,
                            bg,
                        );
                    }
                    if cell.has_contents() {
                        fg = self.contrast.foreground(
                            fg,
                            bg,
                            cell.contents(),
                            palette.terminal_minimum_contrast,
                        );
                        painter.text(
                            pos,
                            egui::Align2::LEFT_TOP,
                            cell.contents(),
                            font.clone(),
                            fg,
                        );
                    }
                    if cell.underline() {
                        painter.line_segment(
                            [pos + egui::vec2(0.0, char_size.y - 1.0), pos + char_size],
                            Stroke::new(1.0_f32, fg),
                        );
                    }
                }
            }
            if !focused && let Some((row, col)) = cursor {
                let pos = origin + egui::vec2(col as f32 * char_size.x, row as f32 * char_size.y);
                let width = if screen.cell(row, col).is_some_and(|cell| cell.is_wide()) {
                    2.0
                } else {
                    1.0
                };
                painter.rect_stroke(
                    Rect::from_min_size(pos, egui::vec2(char_size.x * width, char_size.y)),
                    0,
                    Stroke::new(1.0_f32, palette.muted),
                    egui::StrokeKind::Inside,
                );
            }
        }
        if !bytes_to_send.is_empty() {
            return self.send(bytes_to_send).err().map(|e| e.to_string());
        }
        None
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // clone_killer retains a numeric PID even after the waiter reaps it.
        // A finished view must not signal a later process reusing that PID.
        if self.is_alive()
            && let Some(killer) = &mut self.killer
        {
            let _ = killer.kill();
        }
        if let Some(control) = &self.control {
            control(StreamControl::End);
        }
    }
}

/// Encode shell keys, including application-cursor mode and control modifiers;
/// unsupported/editor-only keys return `None`.
pub fn key_bytes(key: Key, modifiers: egui::Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
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
mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn glyph_color(shapes: &[egui::epaint::ClippedShape], text: &str) -> Color32 {
        shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == text => {
                    Some(t.galley.job.sections[0].format.color)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing rendered glyph {text}"))
    }

    #[test]
    fn bash_prompt_bold_colors_follow_theme_with_normal_resets_and_backgrounds() {
        let ctx = egui::Context::default();
        let (mut terminal, stream) = Terminal::stream(&ctx, "Bash prompt".into(), |_| {});
        stream.end();
        terminal.parser.lock().unwrap().process(
            b"\x1b[1;30mH\x1b[1;34mP\x1b[0;35mG\x1b[1;30m>\x1b[0mN\x1b[1;38;2;9;10;11mR\x1b[1;38;5;17mX\x1b[1;90mB\x1b[22;30mD\x1b[1;30;41mI\x1b[22mJ\x1b[0;34mK"
        );
        let original = terminal.contents();
        for theme in crate::theme::Theme::builtins() {
            let palette = theme.palette;
            assert_eq!(palette.terminal_minimum_contrast, 1.);
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800., 240.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        terminal.ui(ui, 14., palette);
                    });
                },
            );
            for (glyph, expected) in [
                ("H", palette.ansi[8]),
                ("P", palette.ansi[12]),
                ("G", palette.ansi[5]),
                (">", palette.ansi[8]),
                ("N", palette.terminal_foreground),
                ("R", Color32::from_rgb(9, 10, 11)),
                ("X", Color32::from_rgb(0, 0, 95)),
                ("B", palette.ansi[8]),
                ("D", palette.ansi[0]),
                ("I", palette.ansi[8]),
                ("J", palette.ansi[0]),
                ("K", palette.ansi[4]),
            ] {
                assert_eq!(
                    glyph_color(&output.shapes, glyph),
                    expected,
                    "{}: {glyph}",
                    theme.id
                );
            }
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == palette.ansi[1]))
            );
            assert!(
                !output
                    .shapes
                    .iter()
                    .any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == palette.ansi[9]))
            );
            assert_eq!(terminal.contents(), original);
            let parser = terminal.parser.lock().unwrap();
            let host = parser.screen().cell(0, 0).unwrap();
            assert!(host.bold());
            assert_eq!(host.fgcolor(), vt100::Color::Idx(0));
            assert!(!parser.screen().cell(0, 8).unwrap().bold());
        }
    }

    #[test]
    fn rendering_protects_ansi_extended_rgb_and_inverse_without_changing_output() {
        let ctx = egui::Context::default();
        let (mut terminal, stream) = Terminal::stream(&ctx, "Colors".into(), |_| {});
        stream.end();
        terminal.parser.lock().unwrap().process(
            "\x1b[30mA\x1b[38;5;17mB\x1b[38;2;9;10;11mC\x1b[7mD\x1b[0m\x1b[97mE\x1b[38;2;9;10;11m█"
                .as_bytes(),
        );
        let original = terminal.contents();
        for minimum in [1., 4.5, 1.] {
            let palette = Palette {
                terminal_minimum_contrast: minimum,
                ..Default::default()
            };
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800., 240.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        terminal.ui(ui, 14., palette);
                    });
                },
            );
            for (letter, raw) in [
                ("A", palette.ansi[0]),
                ("B", Color32::from_rgb(0, 0, 95)),
                ("C", Color32::from_rgb(9, 10, 11)),
                ("D", palette.terminal_background),
            ] {
                let actual = glyph_color(&output.shapes, letter);
                if minimum == 1. {
                    assert_eq!(actual, raw);
                } else {
                    assert_ne!(actual, raw);
                }
            }
            assert_eq!(glyph_color(&output.shapes, "E"), palette.ansi[15]);
            assert_eq!(
                glyph_color(&output.shapes, "█"),
                Color32::from_rgb(9, 10, 11)
            );
            assert_eq!(terminal.contents(), original);
            let parser = terminal.parser.lock().unwrap();
            assert_eq!(
                parser.screen().cell(0, 2).unwrap().fgcolor(),
                vt100::Color::Rgb(9, 10, 11)
            );
            assert!(parser.screen().cell(0, 3).unwrap().inverse());
        }
    }

    #[derive(Debug)]
    struct CountSignals(Arc<AtomicUsize>);
    impl portable_pty::ChildKiller for CountSignals {
        fn kill(&mut self) -> std::io::Result<()> {
            self.0.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(Self(self.0.clone()))
        }
    }
    #[test]
    fn dropping_a_finished_view_does_not_signal_its_reaped_child() {
        for finished in [false, true] {
            let signals = Arc::new(AtomicUsize::new(0));
            let (mut terminal, stream) =
                Terminal::stream(&egui::Context::default(), "Fixture".into(), |_| {});
            terminal.killer = Some(Box::new(CountSignals(signals.clone())));
            if finished {
                stream.end();
            }
            drop(terminal);
            assert_eq!(signals.load(Ordering::Acquire), usize::from(!finished));
        }
    }

    #[test]
    fn output_view_keeps_late_error_bytes_without_input_or_process_ownership() {
        let signals = Arc::new(AtomicUsize::new(0));
        let (mut terminal, stream) =
            Terminal::stream(&egui::Context::default(), "SSH fixture".into(), |_| {});
        terminal.killer = Some(Box::new(CountSignals(signals.clone())));
        let output = terminal.output_view("SSH ERROR / fixture".into());
        drop(terminal);
        assert_eq!(signals.load(Ordering::Acquire), 1);
        assert!(!output.is_alive());
        assert!(!output.persistent());
        assert!(output.send(b"must not reach SSH\n".to_vec()).is_err());
        // Child exit can arrive before the PTY reader's final stderr bytes.
        stream
            .feed(b"\x1b[31mPermission denied (publickey).\x1b[0m\r\n".to_vec())
            .unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(3);
        while !output.contents().contains("Permission denied (publickey).") {
            assert!(
                std::time::Instant::now() < until,
                "Late diagnostic output was lost"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!output.contents().contains('\x1b'));
        drop(output);
        assert_eq!(
            signals.load(Ordering::Acquire),
            1,
            "The output view must never signal the old process"
        );
    }
}

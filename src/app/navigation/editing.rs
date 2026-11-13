//! Source-only shortcut styles. Mutations become ordinary TextEdit input so
//! its undo history and the editor's versioned save/tool pipeline stay intact.
//!
//! Keep character indices separate from UTF-8 byte offsets. Consume mode commands before
//! TextEdit sees printable events; defer remaining native input only within the same
//! document/style/focus context and bound the queue/register/counts.
//!
//! # Design decisions
//!
//! Emacs/Vim-style commands translate into the existing TextEdit edit/undo path
//! instead of introducing a second buffer engine or embedding either editor.
//! This preserves ordinary saves, cursor identity and tool-version guards; the
//! tradeoff is a documented subset of those keymaps rather than full emulation.
//! Shell readline and modal form fields keep their own key handling, since
//! source-mode commands would otherwise consume valid terminal or dialog input.
use super::*;
use bindings::Style;
use egui::text::{CCursor, CCursorRange};
mod standard;
#[cfg(test)]
mod tests;
mod text;
mod vim;
use text::Text;
const LIMIT: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Transient Vim editing mode; its cursor/selection changes still use native TextEdit history.
pub(super) enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    Line,
}
#[derive(Default)]
/// Transient editing-mode state bound to a document and shortcut style. Registers/queued
/// native events are never preferences; changing context discards pending input before it can
/// reach another widget.
pub(in crate::app) struct State {
    doc: Option<u64>,
    style: Style,
    mode: Mode,
    mark: bool,
    register: String,
    linewise: bool,
    count: usize,
    operator: Option<(char, usize)>,
    g: bool,
    anchor: usize,
    command: Option<String>,
    after: Option<usize>,
    after_range: Option<CCursorRange>,
    composing: bool,
    normalize: bool,
    scroll: bool,
    checkpoint: bool,
    checkpoint_after: bool,
    pub(super) notice: Option<String>,
}
impl State {
    /// Native focus/context changes end composition and any partial keymap command.
    pub(in crate::app) fn cancel_context(&mut self) {
        self.composing = false;
        self.cancel_sequence();
    }
    pub(in crate::app) fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }
    /// Drop a partial operator/count/command when context or input permission changes.
    pub(in crate::app) fn cancel_sequence(&mut self) {
        self.count = 0;
        self.operator = None;
        self.g = false;
        self.command = None;
    }
    pub fn label(&self, style: Style) -> String {
        if style != self.style {
            return style.name().into();
        }
        if let Some(command) = &self.command {
            return format!("VIM  :{command}");
        }
        match style {
            Style::Standard => String::new(),
            Style::Emacs => if self.mark {
                "EMACS  ·  mark"
            } else {
                "EMACS  ·  Ctrl+X commands"
            }
            .into(),
            Style::Vim => format!(
                "VIM  ·  {}{}{}",
                match self.mode {
                    Mode::Normal => "NORMAL · i insert · v select",
                    Mode::Insert => "INSERT · Esc normal",
                    Mode::Visual => "VISUAL · d/c/y",
                    Mode::Line => "VISUAL LINE · d/c/y",
                },
                self.operator
                    .map_or(String::new(), |(c, _)| format!("  {c}")),
                if self.count == 0 {
                    String::new()
                } else {
                    format!(" {}", self.count)
                }
            ),
        }
    }
    /// Bind editing state to a document/style, discarding pending input while preserving the register.
    pub(in crate::app) fn bind(&mut self, id: u64, style: Style) {
        if self.doc != Some(id) || self.style != style {
            let register = std::mem::take(&mut self.register);
            let linewise = self.linewise;
            *self = Self {
                doc: Some(id),
                style,
                register,
                linewise,
                ..Default::default()
            };
        }
    }
    fn register(
        &mut self,
        ctx: &egui::Context,
        text: &Text<'_>,
        range: std::ops::Range<usize>,
        linewise: bool,
    ) {
        if range.is_empty() && !linewise {
            return;
        }
        let value = text.slice(range);
        if value.len() <= LIMIT {
            self.register = value.into();
            self.linewise = linewise;
            if linewise && !self.register.ends_with('\n') && self.register.len() < LIMIT {
                self.register.push('\n');
            }
            ctx.copy_text(self.register.clone());
        }
    }
    fn replace(
        &mut self,
        text: &Text<'_>,
        range: &mut CCursorRange,
        target: std::ops::Range<usize>,
        value: String,
        native: &mut Vec<egui::Event>,
    ) -> bool {
        let removed = text.slice(target.clone()).len();
        if text
            .text
            .len()
            .saturating_sub(removed)
            .saturating_add(value.len())
            > LIMIT.max(text.text.len())
        {
            return false;
        }
        *range = Text::range(target.start, target.end);
        native.push(if value.is_empty() {
            key(Key::Backspace, egui::Modifiers::NONE)
        } else {
            egui::Event::Paste(value)
        });
        self.scroll = true;
        self.checkpoint = true;
        self.checkpoint_after = true;
        true
    }
    pub fn route(
        &mut self,
        ctx: &egui::Context,
        scope: input::Scope,
        source: &str,
        path: Option<&str>,
        queue: &mut input::Replay,
    ) -> Vec<NavAction> {
        let input::Scope {
            document: id,
            style,
            ..
        } = scope;
        queue.bind(scope);
        self.bind(id, style);
        let target = egui::Id::new(("document", id));
        let mut edit = egui::TextEdit::load_state(ctx, target).unwrap_or_default();
        let text = Text::new(source);
        let mut range = edit.cursor.char_range().unwrap_or_default();
        range.primary.index = range.primary.index.min(text.len);
        range.secondary.index = range.secondary.index.min(text.len);
        let original = range;
        let events = ctx.input_mut(|i| std::mem::take(&mut i.events));
        let mut native = vec![];
        let mut effects = vec![];
        let mut pending = false;
        let mut halt = false;
        for event in events {
            if halt {
                queue.defer(event);
                continue;
            }
            let standard_action = (style == Style::Standard)
                .then(|| standard::action(&event))
                .flatten()
                .filter(|_| !self.composing);
            let handled = match (&event, style) {
                (_, Style::Standard) => standard_action.is_some(),
                (
                    egui::Event::Key {
                        key,
                        modifiers,
                        pressed: true,
                        ..
                    },
                    Style::Emacs,
                ) => {
                    modifiers.ctrl
                        && !modifiers.alt
                        && !modifiers.shift
                        && matches!(
                            key,
                            Key::A
                                | Key::E
                                | Key::B
                                | Key::F
                                | Key::P
                                | Key::N
                                | Key::D
                                | Key::H
                                | Key::K
                                | Key::Y
                                | Key::W
                                | Key::Space
                                | Key::G
                                | Key::S
                                | Key::Slash
                        )
                }
                (egui::Event::Text(_), Style::Vim) => self.mode != Mode::Insert,
                (
                    egui::Event::Key {
                        key,
                        modifiers,
                        pressed: true,
                        ..
                    },
                    Style::Vim,
                ) => {
                    *key == Key::Escape
                        || (modifiers.matches_exact(egui::Modifiers::CTRL)
                            && (matches!(key, Key::OpenBracket | Key::C)
                                || (self.mode != Mode::Insert
                                    && matches!(key, Key::R | Key::D | Key::U | Key::F | Key::B))))
                        || (self.mode != Mode::Insert
                            && (modifiers.matches_exact(egui::Modifiers::NONE)
                                || modifiers.matches_exact(egui::Modifiers::SHIFT))
                            && !matches!(
                                key,
                                Key::F1
                                    | Key::F2
                                    | Key::F3
                                    | Key::F4
                                    | Key::F5
                                    | Key::F6
                                    | Key::F7
                                    | Key::F8
                                    | Key::F9
                                    | Key::F10
                                    | Key::F11
                                    | Key::F12
                            ))
                }
                (egui::Event::Ime(_), Style::Vim) => self.mode != Mode::Insert,
                (egui::Event::Copy, Style::Vim) => matches!(self.mode, Mode::Visual | Mode::Line),
                _ => false,
            };
            let native_undo = matches!(&event,egui::Event::Key{key:Key::Z,pressed:true,modifiers,..} if modifiers.command);
            if (handled || native_undo) && pending {
                queue.defer(event);
                halt = true;
                continue;
            }
            if handled {
                if let Some(action) = standard_action {
                    // Keep the inferred unit with this source widget: indenting
                    // every line must not change the next command's tab width.
                    let indent = ctx.data_mut(|data| {
                        *data.get_temp_mut_or_insert_with(target.with("indentation"), || {
                            standard::indentation(source, path)
                        })
                    });
                    halt = self.standard(&text, &mut range, action, path, indent, &mut native);
                } else if style == Style::Emacs {
                    if let egui::Event::Key { key, .. } = event {
                        halt = self.emacs(ctx, &text, &mut range, key, &mut native, &mut effects);
                    }
                } else {
                    match event {
                        egui::Event::Text(value) => {
                            for (command_index, (offset, c)) in value.char_indices().enumerate() {
                                if command_index >= 32 {
                                    queue.defer(egui::Event::Text(value[offset..].into()));
                                    halt = true;
                                    break;
                                }
                                if self.mode == Mode::Insert {
                                    native.push(egui::Event::Text(value[offset..].into()));
                                    pending = true;
                                    break;
                                }
                                halt = self.vim_char(
                                    ctx,
                                    &text,
                                    &mut range,
                                    c,
                                    &mut native,
                                    &mut effects,
                                );
                                if halt {
                                    let next = offset + c.len_utf8();
                                    if next < value.len() {
                                        queue.defer(egui::Event::Text(value[next..].into()));
                                    }
                                    break;
                                }
                            }
                        }
                        egui::Event::Key { key, modifiers, .. } => {
                            halt = self.vim_key(
                                ctx,
                                &text,
                                &mut range,
                                key,
                                modifiers,
                                &mut native,
                                &mut effects,
                            )
                        }
                        egui::Event::Copy => {
                            self.register(
                                ctx,
                                &text,
                                range.as_sorted_char_range(),
                                self.mode == Mode::Line,
                            );
                            self.mode = Mode::Normal;
                            range = Text::range(range.primary.index, range.primary.index);
                        }
                        _ => {}
                    }
                }
            } else {
                match &event {
                    egui::Event::Ime(egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_)) => {
                        self.composing = true;
                    }
                    egui::Event::Ime(egui::ImeEvent::Disabled) => self.composing = false,
                    egui::Event::Ime(egui::ImeEvent::Commit(value))
                        if value != "\n" && value != "\r" =>
                    {
                        self.composing = false;
                    }
                    _ => {}
                }
                if style == Style::Vim && matches!(event, egui::Event::Key { pressed: true, .. }) {
                    self.cancel_sequence();
                }
                pending |= matches!(
                    event,
                    egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Ime(_)
                        | egui::Event::Cut
                        | egui::Event::Key {
                            pressed: true,
                            key: Key::ArrowLeft
                                | Key::ArrowRight
                                | Key::ArrowUp
                                | Key::ArrowDown
                                | Key::Home
                                | Key::End
                                | Key::Backspace
                                | Key::Delete
                                | Key::Enter
                                | Key::Tab
                                | Key::A
                                | Key::Z,
                            ..
                        }
                );
                native.push(event);
            }
        }
        if self.checkpoint {
            self.checkpoint = false;
            let mut undo = edit.undoer();
            undo.add_undo(&(original, source.to_owned()));
            edit.set_undoer(undo);
        }
        if native.iter().any(
            |event| matches!(event,egui::Event::Key{key:Key::Z,pressed:true,modifiers,..} if modifiers.command),
        ) {
            let redo=native.iter().any(|event|matches!(event,egui::Event::Key{key:Key::Z,pressed:true,modifiers,..} if modifiers.shift));
            let mut undo = edit.undoer();
            let mut current = (range, source.to_owned());
            undo.feed_state(ctx.input(|i| i.time), &current);
            let mut steps = 0;
            for _ in 0..101 {
                let next = if redo {
                    undo.redo(&current)
                } else {
                    undo.undo(&current)
                };
                let Some((next_range, next_text)) = next else {
                    break;
                };
                steps += 1;
                if next_text != source {
                    break;
                }
                current.0 = *next_range;
            }
            native.retain(|event|!matches!(event,egui::Event::Key{key:Key::Z,pressed:true,modifiers,..} if modifiers.command));
            native.extend((0..steps).map(|_| key_event_undo(redo)));
        }
        if range != original || self.checkpoint_after {
            edit.cursor.set_char_range(Some(range));
            self.scroll = true;
            edit.store(ctx, target);
        }
        ctx.input_mut(|i| i.events = native);
        if queue.pending() {
            ctx.request_repaint();
        }
        effects
    }
    fn emacs(
        &mut self,
        ctx: &egui::Context,
        text: &Text<'_>,
        range: &mut CCursorRange,
        key: Key,
        native: &mut Vec<egui::Event>,
        effects: &mut Vec<NavAction>,
    ) -> bool {
        let point = range.primary.index;
        let (start, end) = text.line(point);
        let next = match key {
            Key::A => Some(start),
            Key::E => Some(end),
            Key::B => Some(point.saturating_sub(1)),
            Key::F => Some((point + 1).min(text.len)),
            Key::P => Some(text.vertical(point, -1)),
            Key::N => Some(text.vertical(point, 1)),
            _ => None,
        };
        if let Some(next) = next {
            range.primary = CCursor::new(next);
            if !self.mark {
                range.secondary = range.primary;
            }
            return false;
        }
        match key {
            Key::Space => {
                self.mark = !self.mark;
                if self.mark {
                    range.secondary = range.primary;
                }
            }
            Key::G => {
                self.mark = false;
                *range = Text::range(point, point);
            }
            Key::S => {
                effects.push(NavAction::Find);
                return true;
            }
            Key::Slash => {
                native.push(key_event_undo(false));
                return true;
            }
            Key::K | Key::W => {
                let selected = if key == Key::W {
                    range.as_sorted_char_range()
                } else {
                    point..if point == end {
                        (end + 1).min(text.len)
                    } else {
                        end
                    }
                };
                self.register(ctx, text, selected.clone(), false);
                self.mark = false;
                return !selected.is_empty()
                    && self.replace(text, range, selected, String::new(), native);
            }
            Key::Y => {
                if self.register.is_empty() {
                    return false;
                }
                self.mark = false;
                return self.replace(
                    text,
                    range,
                    range.as_sorted_char_range(),
                    self.register.clone(),
                    native,
                );
            }
            Key::D | Key::H => {
                let selected = if !range.is_empty() {
                    range.as_sorted_char_range()
                } else if key == Key::D {
                    point..(point + 1).min(text.len)
                } else {
                    point.saturating_sub(1)..point
                };
                self.mark = false;
                return !selected.is_empty()
                    && self.replace(text, range, selected, String::new(), native);
            }
            _ => {}
        }
        false
    }
    pub fn after_source(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        output: &mut egui::text_edit::TextEditOutput,
        text: &str,
        palette: theme::Palette,
    ) {
        if self.doc != Some(id) {
            return;
        }
        if output.response.gained_focus() || output.response.lost_focus() {
            self.composing = false;
        }
        if self.style == Style::Vim && output.response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    output.response.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
        }

        if let Some(mut range) = output.cursor_range {
            let original_range = range;
            if let Some(selection) = self.after_range.take() {
                range = selection;
            }
            if let Some(point) = self.after.take() {
                range = Text::range(point, point);
            }
            if std::mem::take(&mut self.normalize) {
                let text = Text::new(text);
                let point = text.normal(range.primary.index);
                range = Text::range(point, point);
            }
            let checkpoint = std::mem::take(&mut self.checkpoint_after);
            if checkpoint {
                let mut undo = output.state.undoer();
                undo.add_undo(&(range, text.to_owned()));
                output.state.set_undoer(undo);
            }
            if range != original_range || checkpoint {
                output.state.cursor.set_char_range(Some(range));
                output.state.clone().store(ui.ctx(), output.response.id);
                output.cursor_range = Some(range);
            }
            let caret = output
                .galley
                .pos_from_cursor(range.primary)
                .translate(output.galley_pos.to_vec2());
            if std::mem::take(&mut self.scroll) {
                ui.scroll_to_rect(caret, None);
            }
            if self.style == Style::Vim && self.mode != Mode::Insert && output.response.has_focus()
            {
                let rect = egui::Rect::from_min_size(
                    caret.min,
                    egui::vec2((caret.height() * 0.6).max(5.), caret.height()),
                );
                let painter = ui.painter().with_clip_rect(output.text_clip_rect);
                painter.rect_filled(rect, 0, palette.accent.gamma_multiply(0.32));
            }
        }
    }
}
fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}
fn key_event_undo(redo: bool) -> egui::Event {
    key(
        Key::Z,
        if redo {
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::COMMAND
        },
    )
}

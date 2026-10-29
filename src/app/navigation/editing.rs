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
use std::collections::VecDeque;
#[cfg(test)]
mod tests;
mod text;
mod vim;
use text::Text;
const LIMIT: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    deferred: VecDeque<egui::Event>,
    deferred_bytes: usize,
    after: Option<usize>,
    normalize: bool,
    scroll: bool,
    checkpoint: bool,
    checkpoint_after: bool,
    pub(super) notice: Option<String>,
}
impl State {
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
    fn defer(&mut self, event: egui::Event) {
        let bytes = match &event {
            egui::Event::Text(s) | egui::Event::Paste(s) => s.len(),
            _ => 0,
        };
        if self.deferred.len() < 512 && self.deferred_bytes + bytes <= LIMIT {
            self.deferred.push_back(event);
            self.deferred_bytes += bytes;
        } else {
            self.notice = Some(
                "Queued keyboard input exceeded the editor limit; remaining input was discarded."
                    .into(),
            );
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
    pub fn replay(&mut self, ctx: &egui::Context, id: Option<u64>, style: Style, allowed: bool) {
        if self.deferred.is_empty() {
            return;
        }
        if !allowed
            || self.doc != id
            || self.style != style
            || !id.is_some_and(|id| ctx.memory(|m| m.has_focus(egui::Id::new(("document", id)))))
        {
            self.deferred.clear();
            self.deferred_bytes = 0;
            return;
        }
        ctx.input_mut(|i| {
            let mut events: Vec<_> = self.deferred.drain(..).collect();
            events.append(&mut i.events);
            i.events = events;
        });
        self.deferred_bytes = 0;
    }
    pub fn route(
        &mut self,
        ctx: &egui::Context,
        id: u64,
        source: &str,
        style: Style,
    ) -> Vec<NavAction> {
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
                self.defer(event);
                continue;
            }
            let handled = match (&event, style) {
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
                self.defer(event);
                halt = true;
                continue;
            }
            if handled {
                if style == Style::Emacs {
                    if let egui::Event::Key { key, .. } = event {
                        halt = self.emacs(ctx, &text, &mut range, key, &mut native, &mut effects);
                    }
                } else {
                    match event {
                        egui::Event::Text(value) => {
                            for (command_index, (offset, c)) in value.char_indices().enumerate() {
                                if command_index >= 32 {
                                    self.defer(egui::Event::Text(value[offset..].into()));
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
                                        self.defer(egui::Event::Text(value[next..].into()));
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
                if style == Style::Vim && matches!(event, egui::Event::Key { pressed: true, .. }) {
                    self.cancel_sequence();
                }
                pending |= matches!(
                    event,
                    egui::Event::Text(_)
                        | egui::Event::Paste(_)
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
            |event| matches!(event,egui::Event::Key{key:Key::Z,modifiers,..} if modifiers.command),
        ) {
            let redo=native.iter().any(|event|matches!(event,egui::Event::Key{key:Key::Z,modifiers,..} if modifiers.shift));
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
            native.retain(|event|!matches!(event,egui::Event::Key{key:Key::Z,modifiers,..} if modifiers.command));
            native.extend((0..steps).map(|_| key_event_undo(redo)));
        }
        if range != original || self.checkpoint_after {
            edit.cursor.set_char_range(Some(range));
            self.scroll = true;
            edit.store(ctx, target);
        }
        ctx.input_mut(|i| i.events = native);
        if !self.deferred.is_empty() {
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
        if self.doc != Some(id) || self.style == Style::Standard {
            return;
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

impl EditorApp {
    pub(in crate::app) fn editing_hint<'a>(&self, hint: &'a str) -> &'a str {
        match (self.navigation.bindings.style(), hint) {
            (Style::Emacs, "Ctrl+N") => "",
            (Style::Emacs, "Ctrl+P") => "Ctrl+X Ctrl+F",
            (Style::Emacs, "Ctrl+S") => "Ctrl+X Ctrl+S",
            (Style::Emacs, "Ctrl+Shift+S") => "Ctrl+X Ctrl+W",
            (Style::Emacs, "Ctrl+W") => "Ctrl+X k",
            (Style::Emacs, "Ctrl+F") => "Ctrl+S",
            (Style::Vim, "Ctrl+W") => "Ctrl+W c",
            (Style::Vim, "Ctrl+F") => "Normal: /",
            _ => hint,
        }
    }
    pub(in crate::app) fn style_find_match(&mut self, reverse: bool, ctx: &egui::Context) {
        if self.styled_source().is_none() || self.find.is_empty() {
            return;
        }
        let Some(doc) = self.documents.get_mut(self.active) else {
            return;
        };
        let range = egui::TextEdit::load_state(ctx, egui::Id::new(("document", doc.id)))
            .and_then(|s| s.cursor.char_range())
            .unwrap_or_default();
        let matches = find_matches(&doc.text, &self.find, self.case_sensitive);
        let point = if reverse {
            range.as_sorted_char_range().start
        } else {
            range.as_sorted_char_range().end
        };
        let byte = doc
            .text
            .char_indices()
            .nth(point)
            .map_or(doc.text.len(), |(byte, _)| byte);
        let found = if reverse {
            matches
                .iter()
                .rev()
                .find(|(start, _)| *start < byte)
                .or(matches.last())
        } else {
            matches
                .iter()
                .find(|(start, _)| *start > byte || (!range.is_empty() && *start == byte))
                .or(matches.first())
        };
        if let Some((start, end)) = found {
            doc.selection = Some((
                doc.text[..*start].chars().count(),
                doc.text[..*end].chars().count(),
            ));
            self.focus_editor(ctx);
        }
    }
    fn styled_source(&self) -> Option<(u64, &str)> {
        let doc = self.documents.get(self.active)?;
        if doc.image.is_some()
            || doc.pdf.is_some()
            || doc.hex.is_some()
            || doc.notebook.as_ref().is_some_and(|v| !v.source_mode())
            || doc.markdown.as_ref().is_some_and(|v| v.showing)
            || doc.marp.as_ref().is_some_and(|v| v.showing)
        {
            return None;
        }
        Some((doc.id, &doc.text))
    }
    fn editing_allowed(&self, ctx: &egui::Context) -> bool {
        ctx.input(|i| i.focused)
            && self.dialog == Dialog::None
            && self.pending.is_none()
            && !self.workbench_modal()
            && !self.vaults.locking
            && !egui::Popup::is_any_open(ctx)
    }
    pub(in crate::app) fn replay_style_input(&mut self, ctx: &egui::Context) {
        let id = self.styled_source().map(|(id, _)| id);
        let allowed = self.editing_allowed(ctx);
        self.navigation
            .editing
            .replay(ctx, id, self.navigation.bindings.style(), allowed);
    }
    pub(in crate::app) fn editing_shortcuts(&mut self, ctx: &egui::Context) {
        let style = self.navigation.bindings.style();
        if style == Style::Standard || !self.editing_allowed(ctx) {
            self.navigation.editing.cancel_sequence();
            return;
        }
        let Some((id, _)) = self.styled_source() else {
            return;
        };
        let target = egui::Id::new(("document", id));
        let escape_reclaim = style == Style::Vim
            && ctx.input(|i| i.key_pressed(Key::Escape))
            && ctx.memory(|m| m.had_focus_last_frame(target) && m.focused().is_none());
        if escape_reclaim {
            ctx.memory_mut(|m| m.request_focus(target));
        }
        let keyboard = ctx.input(|i| {
            i.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Cut
                        | egui::Event::Copy
                        | egui::Event::Ime(_)
                        | egui::Event::Key { pressed: true, .. }
                )
            })
        });
        if !ctx.memory(|m| m.has_focus(target)) {
            self.navigation.editing.cancel_sequence();
            return;
        }
        if !keyboard {
            return;
        }
        let text = &self.documents[self.active].text;
        for action in self.navigation.editing.route(ctx, id, text, style) {
            self.navigate(action, ctx);
        }
        if let Some(message) = self.navigation.editing.notice.take() {
            self.notify(message, true);
        }
    }
}

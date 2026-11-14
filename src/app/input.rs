//! Ordered source input admission, separate from pane layout and keymap interpretation.
//!
//! Native TextEdit remains the buffer/undo engine. A command following source input
//! waits until that widget applies the preceding batch. Keymap translation can add
//! earlier replay events, so those always precede the command tail. Replay belongs
//! to one document, style and viewport and never crosses a modal or focus barrier.
use super::*;
use navigation::Style;
mod control;
pub(super) use control::Replay;
pub(super) use control::{Context, Frame, Router, Scope};

/// Share tab recognition between source admission and shortcuts in other widgets.
fn tab_shortcut(ctx: &egui::Context) -> Option<isize> {
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Tab)
            || i.consume_key(egui::Modifiers::COMMAND, Key::PageUp)
        {
            Some(-1)
        } else if i.consume_key(egui::Modifiers::CTRL, Key::Tab)
            || i.consume_key(egui::Modifiers::COMMAND, Key::PageDown)
        {
            Some(1)
        } else {
            None
        }
    })
}

enum DocumentShortcut {
    Cycle(isize),
    New,
    Close,
}
impl DocumentShortcut {
    /// These commands establish a new source destination before translating later input.
    /// Emacs owns Ctrl+N/Ctrl+W as source edits; navigation already consumes Vim prefixes.
    fn take(ctx: &egui::Context, style: Style) -> Option<Self> {
        if let Some(direction) = tab_shortcut(ctx) {
            Some(Self::Cycle(direction))
        } else if style != Style::Emacs && Command::New.consume(ctx) {
            Some(Self::New)
        } else if style != Style::Emacs
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::W))
        {
            Some(Self::Close)
        } else {
            None
        }
    }
}

impl EditorApp {
    fn source_input_scope(&self, ctx: &egui::Context) -> Option<Scope> {
        (self.editors.contains(ctx.viewport_id())
            && self.documents.viewport() == ctx.viewport_id()
            && self.editing_allowed(ctx))
        .then(|| {
            self.documents
                .active_source()
                .map(|source| Scope::new(ctx, source.id, self.navigation.bindings.style()))
        })
        .flatten()
        .filter(|s| s.focused(ctx))
    }

    fn input_context(&self, ctx: &egui::Context) -> Context {
        Context {
            source: self.source_input_scope(ctx),
            pane: self.navigation.active,
        }
    }

    /// Route global editor actions after honoring modal barriers and native terminal focus.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.pending.modal()
            || self.workbench_modal()
            || self.exit_pending()
            || self.vaults.locking()
        {
            return;
        }
        if self.terminal_any_visible()
            && self.tool_viewport(workbench::ToolTab::Terminal) == ctx.viewport_id()
            && self.dialog == Dialog::None
            && !egui::Popup::is_any_open(ctx)
        {
            Terminal::prepare_input(ctx);
        }
        let terminal_focus = Terminal::focused(ctx)
            && self.terminal_any_visible()
            && self.tool_viewport(workbench::ToolTab::Terminal) == ctx.viewport_id();
        let consume = |mods, key| ctx.input_mut(|i| i.consume_key(mods, key));
        if consume(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::P) {
            self.set_dialog(Dialog::Commands);
        }
        self.consume_command(Command::Folder, ctx);
        self.consume_command(Command::Extensions, ctx);
        if self.dialog != Dialog::None {
            return;
        }
        if !egui::Popup::is_any_open(ctx) {
            self.consume_command(Command::Sidebar, ctx);
        }
        self.consume_command(Command::Terminal, ctx);
        self.debugger_shortcuts(ctx, terminal_focus);
        if let Some(direction) = tab_shortcut(ctx) {
            self.cycle_tab(direction, ctx);
        }
        for panel in [Panel::Git, Panel::Problems, Panel::Agent] {
            self.consume_command(Command::Panel(panel), ctx);
        }
        if !terminal_focus {
            self.consume_command(Command::Format, ctx);
            if !self.consume_command(Command::SaveAs, ctx) && Command::Save.consume(ctx) {
                if self.navigation.bindings.style() == navigation::Style::Emacs {
                    self.editor.show_find = true;
                    self.editor.find_focus = true;
                } else {
                    self.execute_command(Command::Save, ctx);
                }
            }
            self.consume_command(Command::Open, ctx);
            self.consume_command(Command::New, ctx);
            if consume(egui::Modifiers::COMMAND, Key::W)
                && let Some(doc) = self.documents.active()
            {
                self.guarded(Action::CloseTab(doc.id), ctx);
            }
            for command in [Command::Explorer, Command::Find, Command::Markdown] {
                self.consume_command(command, ctx);
            }
            if (consume(egui::Modifiers::COMMAND, Key::Equals)
                || consume(egui::Modifiers::COMMAND, Key::Plus))
                && !self.zoom_image_shortcut(ctx, 1.25)
            {
                self.font_size = (self.font_size + 1.0).min(26.0);
                self.font_save_pending = true;
            }
            if consume(egui::Modifiers::COMMAND, Key::Minus)
                && !self.zoom_image_shortcut(ctx, 1. / 1.25)
            {
                self.font_size = (self.font_size - 1.0).max(10.0);
                self.font_save_pending = true;
            }
            self.consume_command(Command::UiScaleReset, ctx);
        }
        if !terminal_focus && self.editor.show_find && consume(egui::Modifiers::NONE, Key::Escape) {
            self.editor.show_find = false;
            self.focus_editor(ctx);
        }
    }
    /// One production entry point for source replay, navigation, editing and global commands.
    pub(super) fn route_input(&mut self, ctx: &egui::Context) {
        self.navigation.bind_input(ctx);
        let context = self.input_context(ctx);
        let frame = self
            .editor
            .input
            .begin(ctx, context, self.navigation.input_rules());
        if frame.resets_prefix() {
            self.navigation.cancel_input();
        }
        self.navigation_shortcuts(ctx);
        if !self
            .editor
            .input
            .accepts(ctx, &frame, self.input_context(ctx))
        {
            return;
        }
        if context.source.is_some()
            && let Some(command) = DocumentShortcut::take(ctx, self.navigation.bindings.style())
        {
            // Earlier source operations have already crossed the native edit
            // boundary. Input after this explicit document command belongs to its target;
            // translate it only after that editor has established focus/cursor state.
            match command {
                DocumentShortcut::Cycle(direction) => self.cycle_tab(direction, ctx),
                DocumentShortcut::New => self.execute_command(Command::New, ctx),
                DocumentShortcut::Close => {
                    self.guarded_source_close(&frame, ctx);
                }
            }
            let next = self
                .editing_allowed(ctx)
                .then(|| self.documents.active_source())
                .flatten()
                .map(|source| Scope::new(ctx, source.id, self.navigation.bindings.style()));
            self.editor.input.handoff(ctx, frame, next);
            self.finish_input(ctx);
            return;
        }
        if self.editors.contains(ctx.viewport_id())
            && self.documents.viewport() == ctx.viewport_id()
        {
            self.editing_shortcuts(ctx);
        }
        if !self
            .editor
            .input
            .accepts(ctx, &frame, self.input_context(ctx))
        {
            return;
        }
        self.shortcuts(ctx);
        if !self
            .editor
            .input
            .accepts(ctx, &frame, self.input_context(ctx))
        {
            return;
        }
        self.editor.input.finish(frame);
        self.finish_input(ctx);
    }

    fn finish_input(&mut self, ctx: &egui::Context) {
        if self.editor.input.pending() {
            ctx.request_repaint();
        }
        if let Some(notice) = self.editor.input.take_notice() {
            self.notify(notice, true);
        }
    }
}

impl EditorApp {
    /// Return the active source-style mode indicator for the editor chrome.
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
    /// Navigate the configured source search in either direction using character cursor offsets.
    pub(in crate::app) fn style_find_match(&mut self, reverse: bool, ctx: &egui::Context) {
        if self.documents.active_source().is_none() || self.editor.find.is_empty() {
            return;
        }
        let Some(doc) = self.documents.active_mut() else {
            return;
        };
        let range = egui::TextEdit::load_state(ctx, egui::Id::new(("document", doc.id)))
            .and_then(|s| s.cursor.char_range())
            .unwrap_or_default();
        let matches = find_matches(&doc.text, &self.editor.find, self.editor.case_sensitive);
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
    pub(in crate::app) fn editing_allowed(&self, ctx: &egui::Context) -> bool {
        ctx.input(|i| i.focused)
            && self.dialog == Dialog::None
            && !self.pending.modal()
            && !self.workbench_modal()
            && !self.vaults.locking()
            && !egui::Popup::is_any_open(ctx)
    }
    /// Translate supported Standard/Emacs/Vim input into the existing source edit/undo path;
    /// modal barriers, terminal focus and unsupported document modes keep their normal input.
    pub(in crate::app) fn editing_shortcuts(&mut self, ctx: &egui::Context) {
        let actions = self.editor.input.translate_source(
            ctx,
            self.documents.active_source(),
            self.navigation.bindings.style(),
            self.editing_allowed(ctx),
        );
        for action in actions {
            self.navigate(action, ctx);
        }
        if let Some(message) = self.editor.input.take_notice() {
            self.notify(message, true);
        }
    }
}

#[cfg(test)]
mod tests;

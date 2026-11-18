//! Ordered source input admission, separate from pane layout and keymap interpretation.
//!
//! Native TextEdit remains the buffer/undo engine. A command following source input
//! waits until that widget applies the preceding batch. Explicit preview switches
//! hand later commands to the same document's rendered view. Keymap translation can
//! add earlier replay events, so those always precede the command tail. Replay belongs
//! to one document, view, style and viewport and never crosses a modal or focus barrier.
use super::*;
use navigation::Style;
mod control;
pub(super) use control::Replay;
pub(super) use control::{Context, Router, Scope};

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
    ClosePreview(u64),
    Markdown,
}
impl DocumentShortcut {
    /// These commands establish a document/view destination before translating later input.
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
        } else if Command::Markdown.consume(ctx) {
            Some(Self::Markdown)
        } else {
            None
        }
    }
}

impl EditorApp {
    /// Preview Close/return commands establish the destination before later
    /// shortcuts in the same packet. Only the first keyboard event may initiate
    /// a handoff, so Close cannot overtake an earlier palette or input command.
    fn preview_document_shortcut(&self, ctx: &egui::Context) -> Option<DocumentShortcut> {
        if !self.editors.contains(ctx.viewport_id())
            || self.documents.viewport() != ctx.viewport_id()
            || !self.editing_allowed(ctx)
            || Terminal::focused(ctx)
        {
            return None;
        }
        let doc = self.documents.active()?;
        let markdown = doc.markdown.as_ref().is_some_and(|preview| preview.showing)
            || doc.marp.as_ref().is_some_and(|preview| preview.showing);
        if !markdown && doc.pdf.is_none() && doc.image.is_none() {
            return None;
        }
        let shortcut = Command::Markdown.shortcut().unwrap();
        ctx.input_mut(|input| {
            let index = input.events.iter().position(|event| match event {
                egui::Event::Key { pressed: true, .. }
                | egui::Event::Text(_)
                | egui::Event::Paste(_)
                | egui::Event::Cut
                | egui::Event::Copy => true,
                egui::Event::Ime(
                    egui::ImeEvent::Preedit(value) | egui::ImeEvent::Commit(value),
                ) => !value.is_empty(),
                // Native IME enable/disable and empty preedit notifications
                // contain no text and may precede the first real shortcut.
                _ => false,
            })?;
            let egui::Event::Key { key, modifiers, .. } = &input.events[index] else {
                return None;
            };
            let command = if *key == Key::W && modifiers.matches_logically(egui::Modifiers::COMMAND)
            {
                DocumentShortcut::ClosePreview(doc.id)
            } else if markdown
                && *key == shortcut.logical_key
                && modifiers.matches_logically(shortcut.modifiers)
            {
                DocumentShortcut::Markdown
            } else {
                return None;
            };
            input.events.remove(index);
            Some(command)
        })
    }

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
        let source = self.source_input_scope(ctx);
        Context {
            source,
            target: source.or_else(|| self.preview_input_scope(ctx)),
            pane: self.navigation.active,
        }
    }

    /// Preview commands remain bound to their editor/document even without a
    /// TextEdit widget. Other panes, dialogs and native windows cannot receive them.
    fn preview_input_scope(&self, ctx: &egui::Context) -> Option<Scope> {
        if !self.editors.contains(ctx.viewport_id())
            || self.documents.viewport() != ctx.viewport_id()
            || self.navigation.active != navigation::Pane::Editor
            || !self.editing_allowed(ctx)
            || Terminal::focused(ctx)
        {
            return None;
        }
        self.documents.active().and_then(|doc| {
            (doc.image.is_some()
                || doc.pdf.is_some()
                || doc.markdown.as_ref().is_some_and(|preview| preview.showing)
                || doc.marp.as_ref().is_some_and(|preview| preview.showing))
            .then(|| Scope::preview(ctx, doc.id, self.navigation.bindings.style()))
        })
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
            if Command::New.consume(ctx) {
                self.new_file();
            }
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
        self.prepare_editor_input(ctx);
        self.navigation.bind_input(ctx);
        keyboard::prepare(ctx, self.keyboard_scope());
        keyboard::resume(ctx);
        // Discovery already owns this packet. Source admission must not move a
        // later Save ahead of text waiting in the focus handoff.
        if keyboard::capture(ctx) {
            self.finish_input(ctx);
            return;
        }
        let context = self.input_context(ctx);
        if context.source.is_none() {
            let prefix = self
                .navigation
                .bindings
                .prefix(Terminal::focused(ctx) || self.dialog != Dialog::None);
            keyboard::fence(
                ctx,
                |event| matches!(event, egui::Event::Key { key, modifiers, pressed: true, .. } if prefix.matches(*key,*modifiers,false)),
            );
        }
        let frame = self
            .editor
            .input
            .begin(ctx, context, self.navigation.input_rules());
        if frame.resets_prefix() {
            self.navigation.cancel_input();
        }
        self.navigation_shortcuts(ctx);
        if keyboard::capture(ctx) {
            self.editor.input.finish(frame);
            self.finish_input(ctx);
            return;
        }
        if !self
            .editor
            .input
            .accepts(ctx, &frame, self.input_context(ctx))
        {
            return;
        }
        let document_command = if context.source.is_some() {
            DocumentShortcut::take(ctx, self.navigation.bindings.style())
        } else {
            self.preview_document_shortcut(ctx)
        };
        if let Some(command) = document_command {
            // Earlier source operations have already crossed the native edit
            // boundary. Input after this explicit document command belongs to its target;
            // translate it only after that editor has established focus/cursor state.
            match command {
                DocumentShortcut::Cycle(direction) => self.cycle_tab(direction, ctx),
                DocumentShortcut::New => self.new_file(),
                DocumentShortcut::Close => {
                    if let Some(id) = frame.document() {
                        self.guarded_input_close(id, ctx);
                    }
                }
                DocumentShortcut::ClosePreview(id) => self.guarded_input_close(id, ctx),
                DocumentShortcut::Markdown => self.toggle_markdown(ctx),
            }
            let next = self
                .editing_allowed(ctx)
                .then(|| self.documents.active_source())
                .flatten()
                .map(|source| Scope::new(ctx, source.id, self.navigation.bindings.style()))
                .or_else(|| self.preview_input_scope(ctx));
            self.editor.input.handoff(ctx, frame, next);
            self.finish_input(ctx);
            return;
        }
        // Consume printable Alt-chord text before a source keymap can treat it
        // as an edit or mode command. Earlier edits have already been fenced.
        if self
            .source_input_scope(ctx)
            .is_some_and(|scope| scope.focused(ctx))
        {
            self.consume_command(Command::WordWrap, ctx);
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
        if let Some(notice) = keyboard::take_notice(ctx) {
            self.notify(notice, true);
        }
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
    /// Whether the current native host accepts source-style editing at all: it must own focus and
    /// have no dialog, guarded modal, workbench modal, vault lock or popup barrier.
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

impl EditorApp {
    pub(super) fn start_keyboard_controls(&mut self, search: bool, ctx: &egui::Context) {
        keyboard::prepare(ctx, self.keyboard_scope());
        keyboard::start(ctx, search);
    }
    pub(super) fn keyboard_scope(&self) -> egui::Id {
        egui::Id::new((
            self.workers.generation(),
            self.documents.active().map(|d| (d.id, d.version())),
            (
                std::mem::discriminant(&self.dialog),
                match self.dialog {
                    Dialog::Save(id) | Dialog::MarpExport(id, _) => id,
                    _ => 0,
                },
            ),
            self.pending.modal(),
            self.pending.presentation().map(|(decision, _)| decision),
            self.workbench_modal(),
            self.exit_pending(),
            self.vaults.locking(),
            self.dialog_viewport,
            self.sessions
                .active
                .as_ref()
                .map(|active| active.key.as_str()),
            self.navigation.bindings.style().name(),
        ))
    }
}

//! Ordered source input admission, separate from pane layout and keymap interpretation.
//!
//! Native TextEdit remains the buffer/undo engine. A command following source input
//! waits until that widget applies the preceding batch. Keymap translation can add
//! earlier replay events, so those always precede the command tail. Replay belongs
//! to one document, style and viewport and never crosses a modal or focus barrier.
use super::*;
use navigation::Style;
use std::collections::VecDeque;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Scope {
    pub document: u64,
    pub style: navigation::Style,
    viewport: egui::ViewportId,
}
impl Scope {
    pub fn new(ctx: &egui::Context, document: u64, style: navigation::Style) -> Self {
        Self {
            document,
            style,
            viewport: ctx.viewport_id(),
        }
    }
    fn focused(self, ctx: &egui::Context) -> bool {
        self.viewport == ctx.viewport_id()
            && ctx.memory(|m| m.has_focus(egui::Id::new(("document", self.document))))
    }
}

#[derive(Default)]
pub(super) struct Router {
    pub editing: navigation::editing::State,
    pub queue: Replay,
}

/// Bounded native events waiting for an acknowledged source application boundary.
#[derive(Default)]
pub(super) struct Replay {
    scope: Option<Scope>,
    events: VecDeque<egui::Event>,
    bytes: usize,
    applied: bool,
    notice: Option<String>,
}
impl Replay {
    pub fn acknowledge(&mut self, ctx: &egui::Context, id: u64) {
        if self
            .scope
            .is_some_and(|s| s.document == id && s.viewport == ctx.viewport_id())
        {
            self.applied = true;
        }
    }
    pub fn pending(&self) -> bool {
        !self.events.is_empty()
    }
    pub fn bind(&mut self, scope: Scope) {
        if self.scope != Some(scope) {
            self.clear();
            self.scope = Some(scope);
        }
    }
    fn clear(&mut self) {
        self.events.clear();
        self.bytes = 0;
        self.applied = false;
    }
    pub fn defer(&mut self, event: egui::Event) {
        let bytes = match &event {
            egui::Event::Text(s) | egui::Event::Paste(s) => s.len(),
            egui::Event::Ime(egui::ImeEvent::Preedit(s) | egui::ImeEvent::Commit(s)) => s.len(),
            _ => 0,
        };
        if self.events.len() < 512 && self.bytes.saturating_add(bytes) <= 8 * 1024 * 1024 {
            self.events.push_back(event);
            self.bytes += bytes;
            self.applied = false;
        } else {
            self.notice = Some(
                "Queued keyboard input exceeded the editor limit; remaining input was discarded."
                    .into(),
            );
        }
    }
    pub fn replay(&mut self, ctx: &egui::Context, scope: Option<Scope>) {
        // A detached tool window cannot consume or cancel the root editor's queue.
        if self.scope.is_some_and(|s| s.viewport != ctx.viewport_id()) {
            return;
        }
        if self.scope != scope || !scope.is_some_and(|s| s.focused(ctx)) {
            self.clear();
            self.scope = scope;
            return;
        }
        if !self.pending() {
            return;
        }
        if !self.applied {
            for event in ctx.input_mut(|i| std::mem::take(&mut i.events)) {
                self.defer(event);
            }
            ctx.request_repaint();
            return;
        }
        ctx.input_mut(|i| {
            let mut events: Vec<_> = self.events.drain(..).collect();
            events.append(&mut i.events);
            i.events = events;
        });
        self.bytes = 0;
        self.applied = false;
    }
}
impl Router {
    /// Only this native widget can acknowledge edits for the queued document.
    pub fn after_source(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        output: &mut egui::text_edit::TextEditOutput,
        text: &str,
        palette: theme::Palette,
    ) {
        self.editing.after_source(ui, id, output, text, palette);
        self.queue.acknowledge(ui.ctx(), id);
    }
    /// Fence commands behind earlier native edits/cursor movement. Translation may
    /// add its own replay events before this tail is appended by the adapter.
    fn fence(
        &mut self,
        ctx: &egui::Context,
        scope: Scope,
        navigation: &navigation::State,
    ) -> Vec<egui::Event> {
        self.queue.bind(scope);
        ctx.input_mut(|i| {
            let prefix = navigation.bindings.prefix(false);
            let cut_modifiers = i
                .events
                .iter()
                .find_map(|event| match event {
                    egui::Event::Key { key, modifiers, .. } if *key == prefix.key => {
                        Some(*modifiers)
                    }
                    _ => None,
                })
                .unwrap_or(i.modifiers);
            let extension_prefix = Command::Extensions.shortcut().filter(|binding| {
                let modifiers = i
                    .events
                    .iter()
                    .find_map(|event| match event {
                        egui::Event::Key { key, modifiers, .. } if *key == binding.logical_key => {
                            Some(*modifiers)
                        }
                        _ => None,
                    })
                    .unwrap_or(i.modifiers);
                modifiers.matches_exact(binding.modifiers)
            });
            let mut source_input = false;
            let mut cut = None;
            for (index, event) in i.events.iter().enumerate() {
                let navigation_command = navigation.command_event(event, cut_modifiers);
                let global_command = (extension_prefix.is_some()
                    && matches!(event, egui::Event::Cut))
                    || matches!(event, egui::Event::Key {key, modifiers, pressed: true, ..}
                    if modifiers.ctrl || modifiers.alt || modifiers.command || matches!(key,
                        Key::F1 | Key::F2 | Key::F3 | Key::F4 | Key::F5 | Key::F6 |
                        Key::F7 | Key::F8 | Key::F9 | Key::F10 | Key::F11 | Key::F12));
                if source_input && (navigation_command || global_command) {
                    cut = Some(index);
                    break;
                }
                source_input |= !navigation_command
                    && matches!(
                        event,
                        egui::Event::Key { pressed: true, .. }
                            | egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Cut
                            | egui::Event::Copy
                            | egui::Event::Ime(_)
                    );
            }
            let Some(index) = cut else {
                return Vec::new();
            };
            let mut tail = i.events.split_off(index);
            // Native Cut has no modifiers. Capture the recognized Emacs prefix
            // before the physical Ctrl release can change a later frame.
            let clipboard_prefix = extension_prefix
                .map(|b| (b.logical_key, b.modifiers))
                .or_else(|| {
                    (prefix.key == Key::X && cut_modifiers.matches_exact(prefix.modifiers))
                        .then_some((prefix.key, prefix.modifiers))
                });
            if let Some((key, modifiers)) = clipboard_prefix {
                for event in &mut tail {
                    if matches!(event, egui::Event::Cut) {
                        *event = egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers,
                        };
                    }
                }
            }
            tail
        })
    }
}

impl EditorApp {
    fn source_input_scope(&self, ctx: &egui::Context) -> Option<Scope> {
        (ctx.viewport_id() == egui::ViewportId::ROOT && self.editing_allowed(ctx))
            .then(|| {
                self.styled_source()
                    .map(|(id, _)| Scope::new(ctx, id, self.navigation.bindings.style()))
            })
            .flatten()
            .filter(|s| s.focused(ctx))
    }

    /// A command may replace the document or focus before layout. Remaining source
    /// events from that batch have no authority to edit the new target.
    fn reject_changed_input_context(
        &mut self,
        ctx: &egui::Context,
        scope: Option<Scope>,
        pane: navigation::Pane,
    ) -> bool {
        if scope.is_none()
            || (self.source_input_scope(ctx) == scope && self.navigation.active == pane)
        {
            return false;
        }
        self.input.queue.clear();
        self.input.editing.cancel_sequence();
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Cut
                        | egui::Event::Copy
                        | egui::Event::Ime(_)
                )
            })
        });
        true
    }
    /// Route global editor actions after honoring modal barriers and native terminal focus.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.pending.is_some()
            || self.workbench_modal()
            || self.exit_pending()
            || self.vaults.locking
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
                    self.show_find = true;
                    self.find_focus = true;
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
        if !terminal_focus && self.show_find && consume(egui::Modifiers::NONE, Key::Escape) {
            self.show_find = false;
            self.focus_editor(ctx);
        }
    }
    /// One production entry point for source replay, navigation, editing and global commands.
    pub(super) fn route_input(&mut self, ctx: &egui::Context) {
        // Source editing lives in the root viewport; tool windows retain native form/PTY input.
        let scope = self.source_input_scope(ctx);
        let pane = self.navigation.active;
        if ctx.viewport_id() == egui::ViewportId::ROOT && self.input.queue.scope != scope {
            self.navigation.cancel_input();
        }
        self.input.queue.replay(ctx, scope);
        let tail = scope
            .map(|s| self.input.fence(ctx, s, &self.navigation))
            .unwrap_or_default();
        self.navigation_shortcuts(ctx);
        if self.reject_changed_input_context(ctx, scope, pane) {
            return;
        }
        if scope.is_some()
            && let Some(command) = DocumentShortcut::take(ctx, self.navigation.bindings.style())
        {
            // Earlier source operations have already crossed the native edit
            // boundary. Input after this explicit document command belongs to its target;
            // translate it only after that editor has established focus/cursor state.
            match command {
                DocumentShortcut::Cycle(direction) => self.cycle_tab(direction, ctx),
                DocumentShortcut::New => self.execute_command(Command::New, ctx),
                DocumentShortcut::Close => {
                    if let Some(id) = self.documents.selected_id() {
                        self.guarded(Action::CloseTab(id), ctx);
                    }
                }
            }
            self.input.editing.cancel_context();
            self.input.queue.clear();
            let remaining = ctx.input_mut(|i| std::mem::take(&mut i.events));
            if self.editing_allowed(ctx)
                && let Some((id, _)) = self.styled_source()
            {
                let next = Scope::new(ctx, id, self.navigation.bindings.style());
                self.input.queue.bind(next);
                for event in remaining.into_iter().chain(tail) {
                    self.input.queue.defer(event);
                }
            }
            self.finish_input(ctx);
            return;
        }
        if ctx.viewport_id() == egui::ViewportId::ROOT {
            self.editing_shortcuts(ctx);
        }
        if self.reject_changed_input_context(ctx, scope, pane) {
            return;
        }
        self.shortcuts(ctx);
        if self.reject_changed_input_context(ctx, scope, pane) {
            return;
        }
        for event in tail {
            self.input.queue.defer(event);
        }
        self.finish_input(ctx);
    }

    fn finish_input(&mut self, ctx: &egui::Context) {
        if self.input.queue.pending() {
            ctx.request_repaint();
        }
        if let Some(notice) = self.input.queue.notice.take() {
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
        if self.styled_source().is_none() || self.find.is_empty() {
            return;
        }
        let Some(doc) = self.documents.active_mut() else {
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
    pub(in crate::app) fn styled_source(&self) -> Option<(u64, &str)> {
        let doc = self.documents.active()?;
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
    pub(in crate::app) fn editing_allowed(&self, ctx: &egui::Context) -> bool {
        ctx.input(|i| i.focused)
            && self.dialog == Dialog::None
            && self.pending.is_none()
            && !self.workbench_modal()
            && !self.vaults.locking
            && !egui::Popup::is_any_open(ctx)
    }
    /// Translate supported Standard/Emacs/Vim input into the existing source edit/undo path;
    /// modal barriers, terminal focus and unsupported document modes keep their normal input.
    pub(in crate::app) fn editing_shortcuts(&mut self, ctx: &egui::Context) {
        let style = self.navigation.bindings.style();
        if !self.editing_allowed(ctx) {
            self.input.editing.cancel_context();
            return;
        }
        let Some((id, _)) = self.styled_source() else {
            self.input.editing.cancel_context();
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
            self.input.editing.cancel_context();
            return;
        }
        if !keyboard {
            return;
        }
        let text = &self.documents.active().unwrap().text;
        for action in self.input.editing.route(
            ctx,
            input::Scope::new(ctx, id, style),
            text,
            self.documents[self.documents.active_index()].display_path(),
            &mut self.input.queue,
        ) {
            self.navigate(action, ctx);
        }
        if let Some(message) = self.input.editing.take_notice() {
            self.notify(message, true);
        }
    }
}

#[cfg(test)]
mod tests;

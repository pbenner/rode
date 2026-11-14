//! Source admission owns replay and keymap state; adapters provide value projections.
//! Frame tokens retain the ordered tail across commands without exposing queue mutation.
use super::super::{Command, Key, documents, navigation, theme};
use eframe::egui;
use navigation::Style;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct Scope {
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
    pub(super) fn focused(self, ctx: &egui::Context) -> bool {
        self.viewport == ctx.viewport_id()
            && ctx.memory(|m| m.has_focus(egui::Id::new(("document", self.document))))
    }
}

/// Read-only admission authority for one viewport's current source and pane.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct Context {
    pub source: Option<Scope>,
    pub pane: navigation::Pane,
}

/// A routing transaction retains its original context and ordered command tail.
/// Accepted frames finish or hand off to a document; context changes discard rejected tails.
#[must_use]
pub(in crate::app) struct Frame {
    context: Context,
    tail: Vec<egui::Event>,
    reset_prefix: bool,
}
impl Frame {
    /// Source command admission has already fenced earlier edits behind a widget acknowledgement.
    pub fn document(&self) -> Option<u64> {
        self.context.source.map(|scope| scope.document)
    }
    pub fn resets_prefix(&self) -> bool {
        self.reset_prefix
    }
}

#[derive(Default)]
pub(in crate::app) struct Router {
    editing: navigation::editing::State,
    queue: Replay,
    rendered: Option<(Scope, u64)>,
}

/// Bounded native events waiting for an acknowledged source application boundary.
#[derive(Default)]
pub(in crate::app) struct Replay {
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
        // A tool viewport cannot consume or cancel another editor's queue.
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
    /// Admit this frame without borrowing pane layout or mutable document state.
    pub fn begin(
        &mut self,
        ctx: &egui::Context,
        context: Context,
        rules: navigation::InputRules<'_>,
    ) -> Frame {
        let reset_prefix = self
            .queue
            .scope
            .is_none_or(|scope| scope.viewport == ctx.viewport_id())
            && self.queue.scope != context.source;
        let rules = if reset_prefix {
            rules.without_prefix()
        } else {
            rules
        };
        self.queue.replay(ctx, context.source);
        let tail = context
            .source
            .map(|scope| self.fence(ctx, scope, rules))
            .unwrap_or_default();
        Frame {
            context,
            tail,
            reset_prefix,
        }
    }

    /// Unrelated commands cannot authorize the old source batch in a new context.
    pub fn accepts(&mut self, ctx: &egui::Context, frame: &Frame, current: Context) -> bool {
        if frame.context.source.is_none() || frame.context == current {
            return true;
        }
        self.queue.clear();
        self.editing.cancel_sequence();
        ctx.input_mut(|i| {
            i.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Cut
                        | egui::Event::Copy
                        | egui::Event::Ime(_)
                )
            })
        });
        false
    }

    /// An explicit document command transfers later events only after the new
    /// native widget acknowledges its focus/cursor state. Modal targets discard them.
    pub fn handoff(&mut self, ctx: &egui::Context, frame: Frame, next: Option<Scope>) {
        self.editing.cancel_context();
        self.queue.clear();
        let remaining = ctx.input_mut(|i| std::mem::take(&mut i.events));
        if let Some(next) = next {
            self.queue.bind(next);
            for event in remaining.into_iter().chain(frame.tail) {
                self.queue.defer(event);
            }
        }
    }

    pub fn finish(&mut self, frame: Frame) {
        for event in frame.tail {
            self.queue.defer(event);
        }
    }
    pub fn pending(&self) -> bool {
        self.queue.pending()
    }
    /// Menu/picker commands drawn before TextEdit must let this frame's edits apply first.
    pub fn source_pending(&self, ctx: &egui::Context) -> bool {
        self.pending()
            || self.queue.scope.is_some_and(|scope| {
                scope.focused(ctx)
                    && self.rendered != Some((scope, ctx.cumulative_pass_nr()))
                    && ctx.input(|input| {
                        input.events.iter().any(|event| {
                            matches!(
                                event,
                                egui::Event::Text(_)
                                    | egui::Event::Paste(_)
                                    | egui::Event::Cut
                                    | egui::Event::Ime(_)
                                    | egui::Event::Key { pressed: true, .. }
                            )
                        })
                    })
            })
    }
    pub fn take_notice(&mut self) -> Option<String> {
        self.queue
            .notice
            .take()
            .or_else(|| self.editing.take_notice())
    }
    pub fn cancel_sequence(&mut self) {
        self.editing.cancel_sequence();
    }
    pub fn label(&self, style: Style) -> String {
        self.editing.label(style)
    }
    pub fn bind_source(&mut self, id: u64, style: Style) {
        self.editing.bind(id, style);
    }

    /// Keep source keymaps behind the native widget's focus and mode boundary.
    pub fn translate_source(
        &mut self,
        ctx: &egui::Context,
        source: Option<documents::Source<'_>>,
        style: Style,
        allowed: bool,
    ) -> Vec<navigation::NavAction> {
        let Some(source) = source.filter(|_| allowed) else {
            self.editing.cancel_context();
            return Vec::new();
        };
        let target = egui::Id::new(("document", source.id));
        let escape_reclaim = style == Style::Vim
            && ctx.input(|i| i.key_pressed(Key::Escape))
            && ctx.memory(|m| m.had_focus_last_frame(target) && m.focused().is_none());
        if escape_reclaim {
            ctx.memory_mut(|m| m.request_focus(target));
        }
        if !ctx.memory(|m| m.has_focus(target)) {
            self.editing.cancel_context();
            return Vec::new();
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
        if !keyboard {
            return Vec::new();
        }
        self.editing.route(
            ctx,
            Scope::new(ctx, source.id, style),
            source.text,
            source.path,
            &mut self.queue,
        )
    }

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
        if let Some(scope) = self
            .queue
            .scope
            .filter(|scope| scope.document == id && scope.viewport == ui.ctx().viewport_id())
        {
            self.rendered = Some((scope, ui.ctx().cumulative_pass_nr()));
        }
    }
    /// Fence commands behind earlier native edits/cursor movement. Translation may
    /// add its own replay events before this tail is appended by the adapter.
    fn fence(
        &mut self,
        ctx: &egui::Context,
        scope: Scope,
        navigation: navigation::InputRules<'_>,
    ) -> Vec<egui::Event> {
        self.queue.bind(scope);
        ctx.input_mut(|i| {
            let prefix = navigation.prefix();
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

//! Editor-only native viewports. Buffers and every project service keep one owner.
//!
//! A document belongs to one tab group. Viewport callbacks borrow its presentation
//! on the UI thread; they never create another application, transport or chat.
use super::*;

pub(super) struct View {
    pub tabs: chrome::Tabs,
    pub input: input::Router,
    pub git_gutter: git_gutter::State,
    pub show_find: bool,
    pub find: String,
    pub replacement: String,
    pub find_focus: bool,
    pub case_sensitive: bool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            tabs: Default::default(),
            input: Default::default(),
            git_gutter: Default::default(),
            show_find: false,
            find: String::new(),
            replacement: String::new(),
            find_focus: false,
            case_sensitive: true,
        }
    }
}

#[derive(Clone, Copy)]
struct Window {
    number: u64,
    viewport: egui::ViewportId,
}

pub(super) struct Windows {
    /// Ownership changes wait until all editor callbacks publish this frame's input.
    pub(super) input_frame_pending: bool,
    current: egui::ViewportId,
    focused: egui::ViewportId,
    windows: Vec<Window>,
    returning: HashSet<egui::ViewportId>,
    parked: HashMap<egui::ViewportId, View>,
    next: u64,
}
impl Default for Windows {
    fn default() -> Self {
        Self {
            input_frame_pending: false,
            current: egui::ViewportId::ROOT,
            focused: egui::ViewportId::ROOT,
            windows: Vec::new(),
            returning: HashSet::new(),
            parked: HashMap::new(),
            next: 1,
        }
    }
}
impl Windows {
    pub(super) fn contains(&self, viewport: egui::ViewportId) -> bool {
        viewport == egui::ViewportId::ROOT
            || self
                .windows
                .iter()
                .any(|window| window.viewport == viewport)
    }
}

impl EditorApp {
    /// Switch the explicit editor context, including its input replay and diff cache.
    /// The document owner retains selection independently for every tab group.
    pub(super) fn activate_editor(&mut self, viewport: egui::ViewportId) {
        let viewport = if self.editors.contains(viewport) {
            viewport
        } else {
            egui::ViewportId::ROOT
        };
        if self.editors.current != viewport {
            let next = self.editors.parked.remove(&viewport).unwrap_or_default();
            let previous = std::mem::replace(&mut self.editor, next);
            self.editors.parked.insert(self.editors.current, previous);
            self.editors.current = viewport;
        }
        self.documents.set_viewport(viewport);
    }

    pub(super) fn in_editor<R>(
        &mut self,
        viewport: egui::ViewportId,
        render: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous = self.editors.current;
        self.activate_editor(viewport);
        let result = render(self);
        self.activate_editor(previous);
        result
    }

    pub(super) fn remember_editor_focus(&mut self, viewport: egui::ViewportId) {
        if self.editors.focused != viewport {
            self.navigation.cancel_input();
        }
        self.editors.focused = viewport;
    }

    pub(super) fn focused_editor(&self) -> egui::ViewportId {
        self.editors.focused
    }

    /// Reuse the existing host when another pane opens or navigates an existing document.
    pub(super) fn focus_document(&mut self, id: u64, ctx: &egui::Context) {
        let Some(viewport) = self.documents.owner(id) else {
            return;
        };
        self.in_editor(viewport, |app| {
            if let Some(index) = app.documents.iter().position(|doc| doc.id == id) {
                app.documents.select_index(index);
            }
            app.reveal_tab(id);
            app.focus_editor(ctx);
        });
    }

    fn allocate_editor_window(&mut self) -> egui::ViewportId {
        let number = self.editors.next;
        self.editors.next = number
            .checked_add(1)
            .expect("Editor window identity space exhausted");
        let viewport = egui::ViewportId::from_hash_of(("rode-editor", number));
        self.editors.windows.push(Window { number, viewport });
        viewport
    }

    pub(super) fn new_editor_window(&mut self, ctx: &egui::Context) {
        let viewport = self.allocate_editor_window();
        self.in_editor(viewport, |app| app.focus_editor(ctx));
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    pub(super) fn editor_input_pending(&self, ctx: &egui::Context) -> bool {
        self.editor.input.source_pending(ctx)
            || self
                .editors
                .parked
                .values()
                .any(|view| view.input.pending())
    }

    pub(super) fn move_editor_tab(
        &mut self,
        id: u64,
        destination: Option<egui::ViewportId>,
        ctx: &egui::Context,
    ) {
        if self.documents.owner(id).is_none() {
            return;
        }
        if self.editor_input_pending(ctx) {
            self.notify(
                "Wait for pending editor input before moving this tab.",
                false,
            );
            return;
        }
        let viewport = destination.unwrap_or_else(|| self.allocate_editor_window());
        if !self.editors.contains(viewport) {
            return;
        }
        self.editor.input.cancel_sequence();
        self.editor.tabs.cancel_focus();
        self.documents.move_to(id, viewport);
        self.focus_document(id, ctx);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    pub(super) fn editor_tab_destinations(&self) -> Vec<(egui::ViewportId, String)> {
        std::iter::once((egui::ViewportId::ROOT, "Main window".into()))
            .chain(
                self.editors
                    .windows
                    .iter()
                    .map(|window| (window.viewport, format!("Editor window {}", window.number))),
            )
            .filter(|(id, _)| *id != self.documents.viewport())
            .collect()
    }

    fn reattach_editor(&mut self, viewport: egui::ViewportId, ctx: &egui::Context) {
        self.documents.reattach(viewport);
        self.editors
            .windows
            .retain(|window| window.viewport != viewport);
        self.editors.returning.remove(&viewport);
        if self.dialog_viewport == viewport {
            self.dialog_viewport = egui::ViewportId::ROOT;
        }
        if self.editors.focused == viewport {
            self.editors.focused = egui::ViewportId::ROOT;
        }
        self.activate_editor(egui::ViewportId::ROOT);
        self.editors.parked.remove(&viewport);
        self.focus_editor(ctx);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Invalidating host work reaches every editor presentation, including hidden tabs.
    pub(super) fn reset_editor_contexts(&mut self, retention: documents::Retention) {
        for view in std::iter::once(&mut self.editor).chain(self.editors.parked.values_mut()) {
            view.git_gutter.reset();
            view.input = Default::default();
            view.show_find = false;
            if retention == documents::Retention::Discard {
                view.tabs = Default::default();
            }
        }
    }

    pub(super) fn complete_editor_gutter(
        &mut self,
        viewport: egui::ViewportId,
        result: git_gutter::Completed,
    ) {
        if self.editors.contains(viewport) {
            self.in_editor(viewport, |app| app.editor.git_gutter.complete(result));
        }
    }

    pub(super) fn detached_editors(&mut self, ctx: &egui::Context) {
        for window in self.editors.windows.clone() {
            let viewport = window.viewport;
            let title = format!("RODE · Editor {}", window.number);
            ctx.show_viewport_immediate(
                viewport,
                egui::ViewportBuilder::default()
                    .with_title(&title)
                    .with_inner_size([1000., 760.])
                    .with_min_inner_size([480., 320.]),
                |ctx, class| {
                    // Rode's native backend uses immediate viewports. A backend without
                    // native viewports leaves tabs accessible through the main editor.
                    if class == egui::ViewportClass::Embedded {
                        self.reattach_editor(viewport, ctx);
                        return;
                    }
                    self.editor_window_ui(ctx);
                },
            );
        }
    }

    /// Returning a window waits for its native edits and ordered command tail.
    /// Removing the presentation earlier would destroy its input replay queue.
    fn editor_window_ui(&mut self, ctx: &egui::Context) {
        let viewport = ctx.viewport_id();
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.editors.returning.insert(viewport);
        }
        let previous_ui = self.ui_viewport;
        self.ui_viewport = viewport;
        self.in_editor(viewport, |app| {
            if ctx.input(|i| i.focused) {
                app.remember_editor_focus(viewport);
            }
            app.route_input(ctx);
            app.editor_window_toolbar(ctx);
            app.git_gutter_tick(ctx);
            app.editor(ctx);
            app.dialogs(ctx);
            app.workbench_windows(ctx);
            app.notice_ui(ctx);
            app.navigation_finish(ctx);
            if app.editors.returning.contains(&viewport) {
                if app.editor.input.source_pending(ctx) {
                    ctx.request_repaint();
                } else {
                    app.reattach_editor(viewport, ctx);
                }
            }
        });
        self.ui_viewport = previous_ui;
    }

    fn editor_window_toolbar(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        egui::TopBottomPanel::top("editor-window-actions").frame(theme::frame(palette.panel)).show(ctx, |ui| {
            if self.foreign_modal(ctx) || self.dialog != Dialog::None || self.pending.modal() { ui.disable(); }
            ui.horizontal_wrapped(|ui| {
                for command in [Command::New, Command::Open, Command::Save, Command::SaveAs, Command::Find] {
                    let definition = command.definition();
                    let button = ui.add(IconButton::new(definition.icon, definition.label));
                    ui_probe::button(ui, &format!("editor-window:{}", definition.label), &button);
                    if button.clicked() { self.execute_command(command, ctx); }
                }
                let reattach = ui.button("Return tabs to main window").on_hover_text("Keeps all buffers and unsaved edits. Chat and project tools stay in their existing windows.");
                ui_probe::button(ui, "editor-window-reattach", &reattach);
                if reattach.clicked() { self.editors.returning.insert(self.documents.viewport()); }
            });
        });
    }
}

#[cfg(test)]
mod tests;

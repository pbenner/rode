//! Editor-only native viewports. Buffers and every project service keep one owner.
//!
//! A document belongs to one tab group. Viewport callbacks borrow its presentation
//! on the UI thread; they never create another application, transport or chat.
use super::*;

/// Presentation and input-replay state for one editor viewport. Buffers, directory listings,
/// transports, chat and runtime ownership remain in the shared application.
pub(super) struct View {
    /// Tab-strip layout and focus state; the document collection owns buffers and tab groups.
    pub tabs: chrome::Tabs,
    /// Ordered native input owned and acknowledged by this editor viewport.
    pub input: input::Router,
    /// Versioned Git comparison and peek state for this editor's selection.
    pub git_gutter: git_gutter::State,
    pub show_find: bool,
    pub find: String,
    pub replacement: String,
    pub find_focus: bool,
    pub case_sensitive: bool,
    /// Child-window sidebar visibility; root visibility is kept by the application layout.
    pub show_explorer: bool,
    /// Requested child sidebar width in logical points, consumed when its panel is rendered.
    pub explorer_width: Option<f32>,
    /// Child-window zoom choice; root pane zoom belongs to the navigation owner.
    pub pane_zoom: Option<navigation::Pane>,
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
            show_explorer: true,
            explorer_width: None,
            pane_zoom: None,
        }
    }
}

#[derive(Clone, Copy)]
struct Window {
    number: u64,
    viewport: egui::ViewportId,
}

/// Registry of native editor hosts and parked presentation state. `current` is the temporary
/// render/input context; `focused` is the remembered command destination.
pub(super) struct Windows {
    /// Editor whose presentation is currently installed in `EditorApp::editor`.
    current: egui::ViewportId,
    /// Remembered editor destination for commands issued from tools or root menus.
    focused: egui::ViewportId,
    windows: Vec<Window>,
    /// Presentation states temporarily outside the active render/input context.
    parked: HashMap<egui::ViewportId, View>,
    next: u64,
}
impl Default for Windows {
    fn default() -> Self {
        Self {
            current: egui::ViewportId::ROOT,
            focused: egui::ViewportId::ROOT,
            windows: Vec::new(),
            parked: HashMap::new(),
            next: 1,
        }
    }
}
impl Windows {
    /// Whether a viewport is the root editor or a still-registered native editor window.
    pub(super) fn contains(&self, viewport: egui::ViewportId) -> bool {
        viewport == egui::ViewportId::ROOT
            || self
                .windows
                .iter()
                .any(|window| window.viewport == viewport)
    }
}

impl EditorApp {
    /// Borrow a registered presentation without switching document/render context. An allocated
    /// window may not yet have instantiated its default view.
    pub(super) fn editor_view(&self, viewport: egui::ViewportId) -> Option<&View> {
        if self.editors.current == viewport {
            Some(&self.editor)
        } else {
            self.editors.parked.get(&viewport)
        }
    }

    /// Address presentation state without changing the rendering context.
    pub(super) fn editor_view_mut(&mut self, viewport: egui::ViewportId) -> Option<&mut View> {
        if !self.editors.contains(viewport) {
            return None;
        }
        if self.editors.current == viewport {
            Some(&mut self.editor)
        } else {
            Some(self.editors.parked.entry(viewport).or_default())
        }
    }

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

    /// Temporarily enter an editor’s render/input context and restore the caller afterward.
    /// Operational code should address documents and views by ID instead of using this scope.
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

    /// Remember the editor command destination and cancel a navigation prefix when ownership of
    /// native focus changes.
    pub(super) fn remember_editor_focus(&mut self, viewport: egui::ViewportId) {
        if self.editors.focused != viewport {
            self.navigation.cancel_input();
        }
        self.editors.focused = viewport;
    }

    /// Last remembered editor host for commands initiated from the root menus or tool windows.
    pub(super) fn focused_editor(&self) -> egui::ViewportId {
        self.editors.focused
    }

    /// Reuse the existing host when another pane opens or navigates an existing document.
    pub(super) fn focus_document(&mut self, id: u64, ctx: &egui::Context) {
        self.submit_editor_operation(
            editor_operations::Operation::Select {
                document: id,
                position: None,
            },
            ctx,
        );
    }

    /// Reserve a monotonic native-window identity for viewport creation by the renderer.
    /// No process or project service is started here.
    pub(super) fn allocate_editor_window(&mut self) -> egui::ViewportId {
        let number = self.editors.next;
        self.editors.next = number
            .checked_add(1)
            .expect("Editor window identity space exhausted");
        let viewport = egui::ViewportId::from_hash_of(("rode-editor", number));
        self.editors.windows.push(Window { number, viewport });
        viewport
    }

    /// Request an empty editor window through the input-ordered operation coordinator.
    pub(super) fn new_editor_window(&mut self, ctx: &egui::Context) {
        self.submit_editor_operation(editor_operations::Operation::NewWindow, ctx);
    }

    /// Whether accepted source input or an ordered command tail still needs a native widget
    /// acknowledgement in any editor.
    pub(super) fn editor_input_pending(&self, ctx: &egui::Context) -> bool {
        self.editor.input.source_pending(ctx)
            || self
                .editors
                .parked
                .values()
                .any(|view| view.input.pending())
    }

    /// Submit a tab transfer after native input; `None` requests a newly allocated editor window.
    pub(super) fn move_editor_tab(
        &mut self,
        id: u64,
        destination: Option<egui::ViewportId>,
        ctx: &egui::Context,
    ) {
        self.submit_editor_operation(
            editor_operations::Operation::Move {
                document: id,
                destination,
            },
            ctx,
        );
    }

    /// Apply an admitted transfer, retaining buffer identity and undo state. Invalid
    /// documents/destinations are ignored; pending input prevents transfer.
    pub(super) fn transfer_editor_tab(
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
        if let Some(view) = self
            .documents
            .owner(id)
            .and_then(|owner| self.editor_view_mut(owner))
        {
            view.input.cancel_sequence();
            view.tabs.cancel_focus();
        }
        self.documents.move_to(id, viewport);
        self.focus_document(id, ctx);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Other registered editor hosts available in the tab context menu, excluding the document’s
    /// current rendering host.
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

    /// Return tabs and pending reads to the root without discarding buffers. The coordinator must
    /// drain input first; presentation/focus caches are then retired.
    pub(super) fn reattach_editor(&mut self, viewport: egui::ViewportId, ctx: &egui::Context) {
        self.documents.reattach(viewport);
        self.navigation.forget_viewport(viewport);
        self.explorer_state.selection.scroll.remove(&viewport);
        self.editors
            .windows
            .retain(|window| window.viewport != viewport);
        if self.dialog_viewport == viewport {
            self.dialog_viewport = egui::ViewportId::ROOT;
        }
        if self.editors.focused == viewport {
            self.remember_editor_focus(egui::ViewportId::ROOT);
        }
        if self.editors.current == viewport {
            self.editor = self
                .editors
                .parked
                .remove(&egui::ViewportId::ROOT)
                .unwrap_or_default();
            self.editors.current = egui::ViewportId::ROOT;
            self.documents.set_viewport(egui::ViewportId::ROOT);
        } else {
            self.editors.parked.remove(&viewport);
        }
        self.focus_editor_in(egui::ViewportId::ROOT, ctx);
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

    /// Deliver an admitted Git result directly to its originating view cache. Results for an
    /// already-closed editor are ignored.
    pub(super) fn complete_editor_gutter(
        &mut self,
        viewport: egui::ViewportId,
        result: git_gutter::Completed,
    ) {
        if let Some(view) = self.editor_view_mut(viewport) {
            view.git_gutter.complete(result);
        }
    }

    /// Render registered editor-only viewports on the UI thread. Backends without native viewports
    /// return their tabs to the root instead.
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
                        self.submit_editor_operation(
                            editor_operations::Operation::Return(viewport),
                            ctx,
                        );
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
            self.submit_editor_operation(editor_operations::Operation::Return(viewport), ctx);
        }
        let previous_ui = self.ui_viewport;
        self.ui_viewport = viewport;
        self.in_editor(viewport, |app| {
            if ctx.input(|i| i.focused) {
                app.remember_editor_focus(viewport);
            }
            app.route_input(ctx);
            app.navigation.begin_viewport_layout(viewport);
            app.editor_explorer_activity(ctx);
            app.explorer(ctx);
            app.git_gutter_tick(ctx);
            app.editor(ctx);
            app.dialogs(ctx);
            app.workbench_windows(ctx);
            app.notice_ui(ctx);
            app.navigation_finish(ctx);
        });
        self.ui_viewport = previous_ui;
    }

    fn editor_explorer_activity(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        egui::SidePanel::left(egui::Id::new(("editor-activity", ctx.viewport_id())))
            .exact_width(52.)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette.panel)
                    .inner_margin(egui::Margin::symmetric(7, 14)),
            )
            .show(ctx, |ui| {
                if self.foreign_modal(ctx) || self.dialog != Dialog::None || self.pending.modal() {
                    ui.disable();
                }
                self.activity_button(ui, Command::Explorer, 40., ctx);
                ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                    let reattach = ui.add_sized(
                        [36., 32.],
                        IconButton::icon(Icon::ChevronLeft, "Return tabs to main window"),
                    );
                    ui_probe::button(ui, "editor-window-reattach", &reattach);
                    if reattach.clicked() {
                        self.submit_editor_operation(
                            editor_operations::Operation::Return(ctx.viewport_id()),
                            ctx,
                        );
                    }
                });
            });
    }
}

#[cfg(test)]
mod tests;

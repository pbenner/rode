//! Window-local document fullscreen presentation. Reuse the existing reader and its workers;
//! leaving restores native fullscreen state without changing pane preferences or services.
use super::*;

/// Stable document identity and the window's native state before entering the reader.
pub(in crate::app) struct Fullscreen {
    document: u64,
    restore_native: bool,
}

impl EditorApp {
    /// Only the document that entered fullscreen may suppress ordinary layout
    /// changes during focus restoration. Selecting another tab keeps its normal intent.
    pub(in crate::app) fn pdf_fullscreen_active(&self, viewport: egui::ViewportId) -> bool {
        self.editor_view(viewport)
            .and_then(|view| view.pdf_fullscreen.as_ref())
            .is_some_and(|state| self.documents.selected_in(viewport) == Some(state.document))
    }

    /// Toggle the selected document in its owning editor window. A palette in a tool window
    /// addresses the remembered editor; native commands never target that tool window.
    pub(in crate::app) fn toggle_pdf_fullscreen(&mut self, ctx: &egui::Context) {
        let viewport = self.command_editor(ctx);
        self.toggle_pdf_fullscreen_in(viewport, ctx);
    }

    /// Reader toolbar actions name their visible host even while another editor
    /// remains the remembered destination for commands issued by tool windows.
    pub(super) fn toggle_pdf_fullscreen_in(
        &mut self,
        viewport: egui::ViewportId,
        ctx: &egui::Context,
    ) {
        self.in_editor(viewport, |app| {
            if app.editor.pdf_fullscreen.is_some() {
                app.leave_pdf_fullscreen(ctx);
                return;
            }
            let Some(document) = app
                .documents
                .active()
                .filter(|doc| doc.pdf.is_some())
                .map(|doc| doc.id)
            else {
                app.notify(
                    "Open a PDF, PowerPoint or Word file to use fullscreen reading.",
                    false,
                );
                return;
            };
            if app.vaults.locking() {
                return;
            }
            if app
                .documents
                .active()
                .and_then(|doc| doc.paged_feature())
                .is_some_and(|feature| !app.feature(feature))
            {
                app.notify(
                    "Enable this document's preview in View → Extensions.",
                    false,
                );
                return;
            }
            let restore_native = ctx
                .input(|input| {
                    input
                        .raw
                        .viewports
                        .get(&viewport)
                        .and_then(|info| info.fullscreen)
                })
                .unwrap_or(false);
            app.editor.pdf_fullscreen = Some(Fullscreen {
                document,
                restore_native,
            });
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Fullscreen(true));
            app.focus_editor_in(viewport, ctx);
            ctx.request_repaint_of(viewport);
        });
    }

    /// Restore only this presentation's native window; pane visibility and zoom were never mutated.
    fn leave_pdf_fullscreen(&mut self, ctx: &egui::Context) {
        if let Some(state) = self.editor.pdf_fullscreen.take() {
            let viewport = self.documents.viewport();
            ctx.send_viewport_cmd_to(
                viewport,
                egui::ViewportCommand::Fullscreen(state.restore_native),
            );
            let pane = self.pane_zoom(viewport).unwrap_or_else(|| {
                if viewport == egui::ViewportId::ROOT
                    && !self.tool_windows.detached
                    && self.tool_surface()
                {
                    if self.tool_windows.surface == tool_windows::Surface::Terminal {
                        navigation::Pane::Terminal
                    } else {
                        navigation::Pane::Tools
                    }
                } else {
                    navigation::Pane::Editor
                }
            });
            self.navigation.request_focus(pane, viewport);
            ctx.request_repaint_of(viewport);
        }
    }

    /// Reader-local F11 takes priority over debugger Step Into. Modified F11, terminals,
    /// tool viewports, modals and keyboard-discovery overlays retain their existing input.
    pub(in crate::app) fn pdf_fullscreen_shortcuts(&mut self, ctx: &egui::Context) {
        if self.documents.viewport() != ctx.viewport_id()
            || !self.editors.contains(ctx.viewport_id())
            || !self.editing_allowed(ctx)
            || Terminal::focused(ctx)
        {
            return;
        }
        if self.editor.pdf_fullscreen.is_some()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            self.leave_pdf_fullscreen(ctx);
        } else if (self.editor.pdf_fullscreen.is_some()
            || self.navigation.active == navigation::Pane::Editor)
            && self.documents.active().is_some_and(|doc| doc.pdf.is_some())
        {
            self.consume_command(Command::PdfFullscreen, ctx);
        }
    }

    /// Fill the owning viewport using the same reader. Selection changes, tab transfers,
    /// project replacement and vault locking end the presentation before ordinary layout.
    pub(in crate::app) fn pdf_fullscreen_ui(&mut self, ctx: &egui::Context) -> bool {
        if self.documents.viewport() != ctx.viewport_id() || self.editor.pdf_fullscreen.is_none() {
            return false;
        }
        let valid = !self.vaults.locking()
            && self.documents.active().is_some_and(|doc| {
                self.editor
                    .pdf_fullscreen
                    .as_ref()
                    .is_some_and(|state| state.document == doc.id)
                    && doc.pdf.is_some()
                    && doc
                        .paged_feature()
                        .is_some_and(|feature| self.feature(feature))
            });
        if !valid {
            self.leave_pdf_fullscreen(ctx);
            return false;
        }
        let palette = self.themes.active().palette;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.background).inner_margin(8.))
            .show(ctx, |ui| {
                if self.foreign_modal(ctx) || self.dialog != Dialog::None || self.pending.modal() {
                    ui.disable();
                }
                ui_probe::button(
                    ui,
                    "pdf-fullscreen",
                    &ui.interact(
                        ui.max_rect(),
                        egui::Id::new("pdf-fullscreen"),
                        Sense::hover(),
                    ),
                );
                let mut content = self.pane_ui(ui, navigation::Pane::Editor);
                self.pdf_ui(&mut content, ctx);
                if let Some(doc) = self.documents.active() {
                    self.editor.input.after_preview(ctx, doc.id);
                }
            });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(ctx: &egui::Context, viewport: egui::ViewportId) -> EditorApp {
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.activate_editor(viewport);
        app.new_file();
        let loaded = pdf::load_bytes(
            include_bytes!("../../../tests/fixtures/pages.pdf").to_vec(),
            1,
            &app.image_budget,
            128,
        )
        .unwrap();
        let doc = app.documents.active_mut().unwrap();
        doc.pdf = Some(PdfView::new(loaded));
        doc.set_dirty_fixture(false);
        app
    }

    fn run(
        ctx: &egui::Context,
        viewport: egui::ViewportId,
        native: bool,
        render: impl FnMut(&egui::Context),
    ) -> egui::FullOutput {
        let mut input = egui::RawInput {
            viewport_id: viewport,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 700.),
            )),
            ..Default::default()
        };
        input.viewports.entry(viewport).or_default().fullscreen = Some(native);
        ctx.run(input, render)
    }

    #[test]
    fn fullscreen_restores_native_state_and_retains_reader_and_pane_preferences() {
        for native in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, egui::ViewportId::ROOT);
            app.navigation.zoom = Some(navigation::Pane::Explorer);
            let explorer = app.sidebar.selected;
            let source = app
                .documents
                .active()
                .unwrap()
                .pdf
                .as_ref()
                .unwrap()
                .source
                .clone();
            let output = run(&ctx, egui::ViewportId::ROOT, native, |ctx| {
                app.toggle_pdf_fullscreen(ctx);
                assert!(app.pdf_fullscreen_ui(ctx));
            });
            assert!(
                output.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::Fullscreen(true)))
            );
            assert!(Arc::ptr_eq(
                &source,
                &app.documents.active().unwrap().pdf.as_ref().unwrap().source
            ));
            let output = run(&ctx, egui::ViewportId::ROOT, true, |ctx| {
                app.toggle_pdf_fullscreen(ctx)
            });
            assert!(output.viewport_output[&egui::ViewportId::ROOT].commands.iter().any(|command| matches!(command, egui::ViewportCommand::Fullscreen(value) if *value == native)));
            assert!(app.editor.pdf_fullscreen.is_none());
            assert_eq!(app.sidebar.selected, explorer);
            assert_eq!(app.navigation.zoom, Some(navigation::Pane::Explorer));
            assert_eq!(app.navigation.active, navigation::Pane::Explorer);
        }
    }

    #[test]
    fn changing_documents_exits_fullscreen_and_detached_windows_keep_their_own_state() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, egui::ViewportId::ROOT);
        let root_document = app.documents.selected_id();
        let viewport = app.allocate_editor_window();
        let id = root_document.unwrap();
        app.transfer_editor_tab(id, Some(viewport), &ctx);
        app.activate_editor(viewport);
        app.editor.pane_zoom = Some(navigation::Pane::Explorer);
        run(&ctx, viewport, false, |ctx| app.toggle_pdf_fullscreen(ctx));
        assert!(app.editor.pdf_fullscreen.is_some());
        assert_eq!(app.editor.pane_zoom, Some(navigation::Pane::Explorer));
        app.activate_editor(egui::ViewportId::ROOT);
        run(&ctx, egui::ViewportId::ROOT, false, |ctx| {
            assert!(!app.pdf_fullscreen_ui(ctx))
        });
        assert!(app.editor.pdf_fullscreen.is_none());
        app.activate_editor(viewport);
        assert!(app.editor.pdf_fullscreen.is_some());
        app.new_file();
        assert_eq!(
            app.editor.pane_zoom, None,
            "The new source must reveal its editor"
        );
        let output = run(&ctx, viewport, true, |ctx| {
            assert!(!app.pdf_fullscreen_ui(ctx))
        });
        assert!(
            output.viewport_output[&viewport]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Fullscreen(false)))
        );
        assert!(app.editor.pdf_fullscreen.is_none());
        assert!(!app.documents.active().unwrap().is_preview());
    }

    #[test]
    fn fullscreen_from_tools_preserves_the_surface_through_focus_restoration() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, egui::ViewportId::ROOT);
        app.tool_windows.surface = tool_windows::Surface::Tools;
        app.tool_windows.panel = Some(Panel::Agent);
        run(&ctx, egui::ViewportId::ROOT, false, |ctx| {
            app.toggle_pdf_fullscreen(ctx);
            assert!(app.pdf_fullscreen_ui(ctx));
            // Closing a dialog schedules the same editor focus request.
            app.focus_editor(ctx);
        });
        assert_eq!(app.tool_windows.surface, tool_windows::Surface::Tools);
        run(&ctx, egui::ViewportId::ROOT, true, |ctx| {
            app.toggle_pdf_fullscreen(ctx)
        });
        assert_eq!(app.tool_windows.surface, tool_windows::Surface::Tools);
        assert_eq!(app.navigation.active, navigation::Pane::Tools);
    }

    #[test]
    fn fullscreen_keys_respect_modals_and_modified_debugger_keys() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, egui::ViewportId::ROOT);
        let event = |key, modifiers| egui::Event::Key {
            key,
            modifiers,
            physical_key: None,
            pressed: true,
            repeat: false,
        };
        for pane in [
            navigation::Pane::Explorer,
            navigation::Pane::Tools,
            navigation::Pane::Terminal,
        ] {
            app.navigation.active = pane;
            let expected = event(Key::F11, egui::Modifiers::NONE);
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![expected.clone()],
                    ..Default::default()
                },
                |ctx| {
                    let before = ctx.input(|input| input.events.clone());
                    app.pdf_fullscreen_shortcuts(ctx);
                    assert!(app.editor.pdf_fullscreen.is_none());
                    assert_eq!(ctx.input(|input| input.events.clone()), before);
                },
            );
        }
        app.navigation.active = navigation::Pane::Editor;
        for (dialog, events, expected) in [
            (
                Dialog::None,
                vec![event(Key::F11, egui::Modifiers::SHIFT)],
                false,
            ),
            (
                Dialog::None,
                vec![event(Key::F11, egui::Modifiers::NONE)],
                true,
            ),
            (
                Dialog::Commands,
                vec![event(Key::Escape, egui::Modifiers::NONE)],
                true,
            ),
            (
                Dialog::None,
                vec![event(Key::Escape, egui::Modifiers::NONE)],
                false,
            ),
        ] {
            app.dialog = dialog;
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| app.pdf_fullscreen_shortcuts(ctx),
            );
            assert_eq!(app.editor.pdf_fullscreen.is_some(), expected);
        }
    }
}

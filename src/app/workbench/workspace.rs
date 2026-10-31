//! One content surface beside the explorer. Detaching moves the same live tool
//! state into a native viewport; it never creates another chat or terminal.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Surface {
    #[default]
    Editor,
    Tools,
    Terminal,
}

impl EditorApp {
    pub(in crate::app) fn tools_viewport() -> egui::ViewportId {
        egui::ViewportId::from_hash_of("rode-project-tools")
    }

    pub(in crate::app) fn editor_intent(&mut self) {
        if !self.workbench.detached {
            self.workbench.surface = Surface::Editor;
        }
        self.navigation.editor_intent();
    }

    pub(in crate::app) fn terminal_intent(&mut self) {
        self.workbench.surface = Surface::Terminal;
        self.navigation.terminal_intent();
    }

    pub(in crate::app) fn tools_visible(&self) -> bool {
        self.workbench.surface == Surface::Tools
            && self.workbench.panel.is_some()
            && (self.workbench.detached || self.navigation.visible(navigation::Pane::Tools))
    }

    pub(in crate::app) fn terminal_visible(&self) -> bool {
        self.workbench.surface == Surface::Terminal
            && self.show_terminal
            && (self.workbench.detached || self.navigation.visible(navigation::Pane::Terminal))
    }

    fn tool_surface(&self) -> bool {
        self.tools_visible() || self.terminal_visible()
    }

    pub(in crate::app) fn foreign_modal(&self, ctx: &egui::Context) -> bool {
        let owner = if self.dialog != Dialog::None || self.pending.is_some() {
            Some(self.dialog_viewport)
        } else if self.workbench_modal() {
            Some(if self.workbench.detached {
                Self::tools_viewport()
            } else {
                egui::ViewportId::ROOT
            })
        } else {
            None
        };
        owner.is_some_and(|owner| owner != ctx.viewport_id())
    }

    fn dock_tools(&mut self, ctx: &egui::Context) {
        self.workbench.detached = false;
        self.dialog_viewport = egui::ViewportId::ROOT;
        self.navigation.zoom = None;
        let pane = if self.terminal_visible() {
            navigation::Pane::Terminal
        } else if self.tools_visible() {
            navigation::Pane::Tools
        } else {
            navigation::Pane::Editor
        };
        self.focus_pane(pane, ctx);
        if pane == navigation::Pane::Tools && self.workbench.panel == Some(Panel::Agent) {
            self.agents.focus = true;
        }
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    fn workspace_tabs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, child: bool) {
        if self.foreign_modal(ctx) {
            ui.disable();
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(
                    !child && (self.workbench.detached || !self.tool_surface()),
                    "Code",
                )
                .clicked()
            {
                self.focus_editor(ctx);
                self.editor_intent();
            }
            if ui
                .selectable_label(
                    self.terminal_visible() && (!self.workbench.detached || child),
                    "Terminal",
                )
                .clicked()
            {
                self.show_terminal = true;
                self.terminal_intent();
                if self.terminal.is_none() && self.ssh_master.is_none() && !self.session_busy() {
                    self.start_project_terminal(ctx);
                }
                self.focus_pane(navigation::Pane::Terminal, ctx);
            }
            for (panel, label) in [
                (Panel::Agent, "Chat"),
                (Panel::Git, "Git"),
                (Panel::Monitor, "Monitor"),
                (Panel::Problems, "Problems"),
                (Panel::Python, "Python"),
                (Panel::Settings, "Settings"),
                (Panel::Debug, "Debugger"),
            ] {
                if !self.panel_enabled(panel) {
                    continue;
                }
                let selected = self.tools_visible()
                    && self.workbench.panel == Some(panel)
                    && (!self.workbench.detached || child);
                if ui.selectable_label(selected, label).clicked() {
                    if !selected {
                        self.toggle_panel(panel, ctx);
                    }
                    if panel == Panel::Agent {
                        self.agents.focus = true;
                    }
                }
            }
            ui.separator();
            if self.workbench.detached {
                if !child && ui.small_button("Show tools window").clicked() {
                    ctx.send_viewport_cmd_to(Self::tools_viewport(), egui::ViewportCommand::Focus);
                }
                if ui.small_button("Dock back").clicked() {
                    self.dock_tools(ctx);
                }
            } else if self.tool_surface() && ui.small_button("Detach").clicked() {
                self.workbench.detached = true;
                self.navigation.zoom = None;
                ctx.request_repaint();
            }
        });
    }

    fn surface_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.foreign_modal(ctx) {
            ui.disable();
        }
        match self.workbench.surface {
            Surface::Terminal => self.terminal_content(ui, ctx),
            Surface::Tools => self.workbench_content(ui, ctx),
            Surface::Editor => {}
        }
    }

    pub(in crate::app) fn workspace_content(&mut self, ctx: &egui::Context) {
        if self.navigation.zoom == Some(navigation::Pane::Explorer) {
            return;
        }
        if !self.workbench.detached && !self.tool_surface() {
            self.editor(ctx);
            return;
        }
        let palette = self.themes.active().palette;
        egui::TopBottomPanel::top("workspace-tabs")
            .frame(theme::frame(palette.panel))
            .show(ctx, |ui| self.workspace_tabs(ui, ctx, false));
        if self.workbench.detached || !self.tool_surface() {
            self.editor(ctx);
        } else {
            egui::CentralPanel::default()
                .frame(theme::frame(palette.panel))
                .show(ctx, |ui| self.surface_ui(ui, ctx));
        }
    }

    pub(in crate::app) fn detached_tools(&mut self, ctx: &egui::Context) {
        if !self.workbench.detached {
            return;
        }
        if !self.tool_surface() {
            self.dock_tools(ctx);
            return;
        }
        let context = ctx.clone();
        context.show_viewport_immediate(
            Self::tools_viewport(),
            egui::ViewportBuilder::default()
                .with_title("RODE · Project tools")
                .with_inner_size([900., 760.])
                .with_min_inner_size([640., 420.]),
            |ctx, class| {
                if ctx.input(|input| input.viewport().close_requested()) {
                    self.dock_tools(ctx);
                    return;
                }
                self.ui_viewport = ctx.viewport_id();
                if class == egui::ViewportClass::Embedded {
                    let mut open = true;
                    egui::Window::new("Project tools")
                        .default_size([860., 700.])
                        .open(&mut open)
                        .show(ctx, |ui| {
                            self.workspace_tabs(ui, ctx, true);
                            ui.separator();
                            self.surface_ui(ui, ctx);
                        });
                    if !open {
                        self.dock_tools(ctx);
                    }
                    self.workbench_windows(ctx);
                } else {
                    self.navigation_shortcuts(ctx);
                    self.shortcuts(ctx);
                    if self.dialog != Dialog::None
                        || self.pending.is_some()
                        || self.workbench_modal()
                    {
                        ctx.memory_mut(|memory| {
                            memory.surrender_focus(egui::Id::new("terminal-input"))
                        });
                    }
                    let palette = self.themes.active().palette;
                    egui::TopBottomPanel::top("detached-tools-tabs")
                        .frame(theme::frame(palette.panel))
                        .show(ctx, |ui| self.workspace_tabs(ui, ctx, true));
                    egui::CentralPanel::default()
                        .frame(theme::frame(palette.panel))
                        .show(ctx, |ui| self.surface_ui(ui, ctx));
                    self.dialogs(ctx);
                    self.workbench_windows(ctx);
                    self.notice_ui(ctx);
                    self.navigation_finish(ctx);
                }
                self.ui_viewport = egui::ViewportId::ROOT;
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(app: &mut EditorApp, ctx: &egui::Context) {
        render_width(app, ctx, 1200.);
    }

    fn render_width(app: &mut EditorApp, ctx: &egui::Context, width: f32) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 800.),
                )),
                ..Default::default()
            },
            |ctx| {
                app.navigation.begin_layout();
                app.activity(ctx);
                app.explorer(ctx);
                app.workspace_content(ctx);
            },
        );
    }

    #[test]
    fn tools_and_terminal_replace_code_without_narrowing_the_workspace_or_losing_edits() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].text = "unsaved code".into();
        render(&mut app, &ctx);
        let code = app.navigation.rect(navigation::Pane::Editor).unwrap();
        app.toggle_panel(Panel::Settings, &ctx);
        render(&mut app, &ctx);
        let tools = app.navigation.rect(navigation::Pane::Tools).unwrap();
        assert!((code.width() - tools.width() - 24.).abs() < 1.);
        assert!(app.navigation.rect(navigation::Pane::Editor).is_none());
        app.show_terminal = true;
        app.terminal_intent();
        render(&mut app, &ctx);
        let terminal = app.navigation.rect(navigation::Pane::Terminal).unwrap();
        assert!((code.width() - terminal.width() - 24.).abs() < 1.);
        assert!(app.navigation.rect(navigation::Pane::Tools).is_none());
        assert!(app.navigation.rect(navigation::Pane::Editor).is_none());
        app.focus_editor(&ctx);
        render(&mut app, &ctx);
        assert!(app.navigation.rect(navigation::Pane::Editor).is_some());
        assert_eq!(app.documents[0].text, "unsaved code");
        assert!(app.documents[0].dirty());
    }

    #[test]
    fn opening_tools_at_minimum_width_does_not_resize_the_explorer() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        render_width(&mut app, &ctx, 850.);
        let before = app
            .navigation
            .rect(navigation::Pane::Explorer)
            .unwrap()
            .width();
        app.toggle_panel(Panel::Settings, &ctx);
        render_width(&mut app, &ctx, 850.);
        let after = app
            .navigation
            .rect(navigation::Pane::Explorer)
            .unwrap()
            .width();
        assert!((before - after).abs() < 1.);
    }

    #[test]
    fn tool_dialogs_keep_their_owner_until_docking_and_block_other_viewports() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.workbench.panel = Some(Panel::Settings);
        app.workbench.surface = Surface::Tools;
        app.workbench.detached = true;
        app.ui_viewport = EditorApp::tools_viewport();
        app.set_dialog(Dialog::Shortcuts);
        assert!(app.foreign_modal(&ctx));
        app.ui_viewport = egui::ViewportId::ROOT;
        app.dock_tools(&ctx);
        assert!(app.dialog == Dialog::Shortcuts);
        assert!(!app.foreign_modal(&ctx));
    }

    #[test]
    fn detaching_restores_code_and_docking_retains_the_selected_tool_and_chat() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.agents.chat.notice("retained conversation");
        app.toggle_panel(Panel::Settings, &ctx);
        app.workbench.detached = true;
        render(&mut app, &ctx);
        assert!(app.navigation.rect(navigation::Pane::Editor).is_some());
        app.focus_editor(&ctx);
        assert!(app.workbench.detached);
        assert_eq!(app.workbench.surface, Surface::Tools);
        app.dock_tools(&ctx);
        render(&mut app, &ctx);
        assert!(!app.workbench.detached);
        assert!(app.navigation.rect(navigation::Pane::Editor).is_none());
        assert_eq!(app.workbench.panel, Some(Panel::Settings));
        assert_eq!(
            app.agents.chat.messages.back().unwrap().text,
            "retained conversation"
        );
    }
}

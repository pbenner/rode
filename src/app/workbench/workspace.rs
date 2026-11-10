//! One selected docked surface plus optional sidebar and individual-tab viewports.
//! Each tool renders in exactly one host, sharing its existing controller and unsaved input.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// The single selected content surface; detached tools retain this same underlying state.
pub(in crate::app) enum Surface {
    #[default]
    Editor,
    Tools,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// Remembered tool destination restored when the tools/terminal surface is shown again.
pub(in crate::app) enum ToolTab {
    Terminal,
    Panel(Panel),
}

impl ToolTab {
    const ALL: [Self; 9] = [
        Self::Panel(Panel::Agent),
        Self::Terminal,
        Self::Panel(Panel::Git),
        Self::Panel(Panel::Monitor),
        Self::Panel(Panel::Problems),
        Self::Panel(Panel::Environment),
        Self::Panel(Panel::Tasks),
        Self::Panel(Panel::Settings),
        Self::Panel(Panel::Debug),
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::Panel(Panel::Agent) => "Chat",
            Self::Panel(Panel::Git) => "Git",
            Self::Panel(Panel::Monitor) => "Monitor",
            Self::Panel(Panel::Problems) => "Problems",
            Self::Panel(Panel::Environment) => "Environment",
            Self::Panel(Panel::Tasks) => "Tasks",
            Self::Panel(Panel::Settings) => "Settings",
            Self::Panel(Panel::Debug) => "Debugger",
        }
    }

    pub(in crate::app) fn pane(self) -> navigation::Pane {
        match self {
            Self::Terminal => navigation::Pane::Terminal,
            Self::Panel(_) => navigation::Pane::Tools,
        }
    }

    fn viewport(self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("rode-tool-tab", self))
    }
}

fn detached_builder(title: &str) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(format!("RODE · {title}"))
        .with_inner_size([900., 760.])
        .with_min_inner_size([640., 420.])
}

fn embedded_window(title: &str) -> egui::Window<'static> {
    egui::Window::new(title).default_size([860., 700.])
}

impl EditorApp {
    /// Return the stable native viewport identity shared by detached project tools.
    pub(in crate::app) fn tools_viewport() -> egui::ViewportId {
        egui::ViewportId::from_hash_of("rode-project-tools")
    }

    pub(in crate::app) fn tab_detached(&self, tab: ToolTab) -> bool {
        self.workbench.floating_tabs.contains(&tab)
    }

    /// Resolve the native owner for focus requests and modal dialogs.
    pub(in crate::app) fn tool_viewport(&self, tab: ToolTab) -> egui::ViewportId {
        if self.tab_detached(tab) {
            tab.viewport()
        } else if self.workbench.detached {
            Self::tools_viewport()
        } else {
            egui::ViewportId::ROOT
        }
    }

    pub(in crate::app) fn floating_tab(&self, viewport: egui::ViewportId) -> Option<ToolTab> {
        self.workbench
            .floating_tabs
            .iter()
            .copied()
            .find(|tab| tab.viewport() == viewport)
    }

    fn selected_tool_tab(&self) -> Option<ToolTab> {
        match self.workbench.surface {
            Surface::Tools => self.workbench.panel.map(ToolTab::Panel),
            Surface::Terminal => Some(ToolTab::Terminal),
            Surface::Editor => None,
        }
    }

    /// Background polling must include independently visible tabs, regardless of dock selection.
    pub(in crate::app) fn panel_visible(&self, panel: Panel) -> bool {
        self.tab_detached(ToolTab::Panel(panel))
            || (self.tools_visible() && self.workbench.panel == Some(panel))
    }

    pub(in crate::app) fn terminal_any_visible(&self) -> bool {
        self.tab_detached(ToolTab::Terminal) || self.terminal_visible()
    }

    /// Raise a tab's existing host without changing the selection of any other window.
    pub(in crate::app) fn focus_tool_tab(&mut self, tab: ToolTab, ctx: &egui::Context) {
        self.tabs.cancel_focus();
        self.workbench.last_tool = Some(tab);
        self.navigation
            .request_focus(tab.pane(), self.tool_viewport(tab));
        if tab == ToolTab::Panel(Panel::Agent) {
            self.agents.focus = true;
        }
        let viewport = self.tool_viewport(tab);
        ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
        ctx.request_repaint_of(viewport);
    }

    /// Move presentation only. Controllers and unsaved input stay in the same application state.
    fn detach_tab(&mut self, tab: ToolTab, ctx: &egui::Context) {
        if self.tab_detached(tab) {
            return;
        }
        let previous_owner = self.tool_viewport(tab);
        // Terminal startup records focus intent. Set its host first so it
        // cannot replace a different tool still selected in the sidebar.
        self.workbench.floating_tabs.push(tab);
        match tab {
            ToolTab::Terminal => {
                self.show_terminal = true;
                if self.terminal.is_none() && self.ssh_master.is_none() && !self.session_busy() {
                    self.start_project_terminal(ctx);
                }
            }
            ToolTab::Panel(Panel::Git) => self.refresh_git(ctx),
            ToolTab::Panel(Panel::Environment | Panel::Tasks) => {
                self.refresh_environment(false, ctx);
                self.refresh_runtimes(ctx);
            }
            _ => {}
        }
        if self.dialog_viewport == previous_owner && self.dialog != Dialog::None {
            self.dialog_viewport = tab.viewport();
        }
        if self.selected_tool_tab() == Some(tab) {
            self.workbench.surface = Surface::Editor;
            if self.workbench.detached {
                // Keep the sidebar useful when one of its tabs moves to another window.
                if let Some(ToolTab::Panel(panel)) = ToolTab::ALL.into_iter().find(|candidate| {
                    matches!(candidate, ToolTab::Panel(panel) if self.panel_enabled(*panel))
                        && !self.tab_detached(*candidate)
                }) {
                    self.toggle_panel(panel, ctx);
                } else {
                    self.dock_tools(ctx);
                }
            }
        }
        self.navigation.zoom = None;
        self.focus_tool_tab(tab, ctx);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Closing a tab window reattaches it; it never closes the underlying session.
    pub(in crate::app) fn reattach_tool_tab(&mut self, tab: ToolTab, ctx: &egui::Context) {
        self.workbench
            .floating_tabs
            .retain(|candidate| *candidate != tab);
        if self.dialog_viewport == tab.viewport() {
            self.dialog_viewport = self.tool_viewport(tab);
        }
        self.workbench.surface = Surface::Editor;
        match tab {
            ToolTab::Terminal => self.focus_pane(navigation::Pane::Terminal, ctx),
            ToolTab::Panel(panel) => self.toggle_panel(panel, ctx),
        }
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Record source-editor focus intent and select the docked editor without discarding tool state.
    pub(in crate::app) fn editor_intent(&mut self) {
        if !self.workbench.detached {
            self.remember_tool_tab();
            self.workbench.surface = Surface::Editor;
        }
        self.navigation.editor_intent();
    }

    /// Select and reveal the terminal surface while preserving its running shell.
    pub(in crate::app) fn terminal_intent(&mut self) {
        if !self.tab_detached(ToolTab::Terminal) {
            self.workbench.surface = Surface::Terminal;
            self.workbench.tools_hidden = false;
        }
        self.workbench.last_tool = Some(ToolTab::Terminal);
        self.navigation.terminal_intent();
        self.navigation.request_focus(
            navigation::Pane::Terminal,
            self.tool_viewport(ToolTab::Terminal),
        );
    }

    /// Remember the selected tool/terminal before leaving it; the editor does not replace that choice.
    pub(in crate::app) fn remember_tool_tab(&mut self) {
        let tab = match self.workbench.surface {
            Surface::Tools => self.workbench.panel.map(ToolTab::Panel),
            Surface::Terminal => Some(ToolTab::Terminal),
            Surface::Editor => None,
        };
        if let Some(tab) = tab {
            self.workbench.last_tool = Some(tab);
        }
    }

    /// Hide or restore the last tool surface, initially Chat, updating pane focus
    /// and detached-window visibility.
    pub(in crate::app) fn toggle_sidebar(&mut self, ctx: &egui::Context) {
        let visible = self.tool_surface();
        self.navigation.zoom = None;
        if visible {
            self.remember_tool_tab();
            if self.workbench.detached {
                self.workbench.tools_hidden = true;
                ctx.send_viewport_cmd_to(
                    Self::tools_viewport(),
                    egui::ViewportCommand::Visible(false),
                );
            }
            self.focus_pane(navigation::Pane::Editor, ctx);
        } else {
            let tab = self
                .workbench
                .last_tool
                .unwrap_or(ToolTab::Panel(Panel::Agent));
            match tab {
                ToolTab::Panel(panel) if self.panel_enabled(panel) => {
                    if self.workbench.surface == Surface::Tools
                        && self.workbench.panel == Some(panel)
                    {
                        self.focus_pane(navigation::Pane::Tools, ctx);
                        if panel == Panel::Agent {
                            self.agents.focus = true;
                        }
                    } else {
                        self.toggle_panel(panel, ctx);
                    }
                }
                _ => {
                    self.show_terminal = true;
                    self.focus_pane(navigation::Pane::Terminal, ctx);
                    Terminal::focus(ctx);
                }
            }
        }
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Report effective tool visibility, including detached and zoomed-pane state.
    pub(in crate::app) fn tools_visible(&self) -> bool {
        !self.workbench.tools_hidden
            && self.workbench.surface == Surface::Tools
            && self.workbench.panel.is_some()
            && !self
                .workbench
                .panel
                .is_some_and(|panel| self.tab_detached(ToolTab::Panel(panel)))
            && (self.workbench.detached || self.navigation.visible(navigation::Pane::Tools))
    }

    /// Report effective terminal visibility without inferring whether its shell is still alive.
    pub(in crate::app) fn terminal_visible(&self) -> bool {
        !self.workbench.tools_hidden
            && self.workbench.surface == Surface::Terminal
            && self.show_terminal
            && !self.tab_detached(ToolTab::Terminal)
            && (self.workbench.detached || self.navigation.visible(navigation::Pane::Terminal))
    }

    fn tool_surface(&self) -> bool {
        self.tools_visible() || self.terminal_visible()
    }

    /// Block input when an application modal belongs to another native viewport.
    pub(in crate::app) fn foreign_modal(&self, ctx: &egui::Context) -> bool {
        let owner = if self.dialog != Dialog::None || self.pending.is_some() {
            Some(self.dialog_viewport)
        } else if self.workbench_modal() {
            Some(self.tool_viewport(ToolTab::Panel(Panel::Git)))
        } else {
            None
        };
        owner.is_some_and(|owner| owner != ctx.viewport_id())
    }

    fn dock_tools(&mut self, ctx: &egui::Context) {
        self.workbench.detached = false;
        self.workbench.tools_hidden = false;
        if self.dialog_viewport == Self::tools_viewport() {
            self.dialog_viewport = egui::ViewportId::ROOT;
        }
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
            let code_selected = !child && (self.workbench.detached || !self.tool_surface());
            let code = ui.selectable_label(code_selected, "Code");
            ui_probe::button(ui, "workspace-code", &code);
            if code_selected {
                ui_probe::button(ui, "workspace-code-selected", &code);
            }
            if code.clicked() {
                self.focus_editor(ctx);
                self.editor_intent();
            }
            for tab in ToolTab::ALL {
                if matches!(tab, ToolTab::Panel(panel) if !self.panel_enabled(panel)) {
                    continue;
                }
                let detached = self.tab_detached(tab);
                let selected = self.tool_surface()
                    && self.selected_tool_tab() == Some(tab)
                    && (!self.workbench.detached || child);
                let response =
                    ui.add_enabled(!detached, egui::Button::selectable(selected, tab.label()));
                ui_probe::button(ui, &format!("workspace-tab:{}", tab.label()), &response);
                if detached {
                    response.on_disabled_hover_text(format!(
                        "{} is open in its own window. Reattach it there or close that window.",
                        tab.label()
                    ));
                    continue;
                }
                response.context_menu(|ui| {
                    if ui.button("Detach tab").clicked() {
                        self.detach_tab(tab, ctx);
                        ui.close();
                    }
                });
                if response.clicked() {
                    match tab {
                        ToolTab::Terminal => self.focus_pane(navigation::Pane::Terminal, ctx),
                        ToolTab::Panel(panel) if !selected => self.toggle_panel(panel, ctx),
                        _ => {}
                    }
                    if tab == ToolTab::Panel(Panel::Agent) {
                        self.agents.focus = true;
                    }
                }
            }
            ui.separator();
            if self.workbench.detached {
                let dock = ui.button("Dock back");
                ui_probe::button(ui, "workspace-dock", &dock);
                if dock.clicked() {
                    self.dock_tools(ctx);
                }
            } else if self.tool_surface() {
                let detach = ui.button("Detach");
                ui_probe::button(ui, "workspace-detach", &detach);
                if detach.clicked() {
                    self.workbench.detached = true;
                    self.navigation.zoom = None;
                    ctx.request_repaint();
                }
            }
            if self.tool_surface()
                && let Some(tab) = self.selected_tool_tab()
            {
                let detach = ui
                    .button("Detach tab")
                    .on_hover_text(format!("Open {} in its own window", tab.label()));
                ui_probe::button(
                    ui,
                    &format!("workspace-detach-tab:{}", tab.label()),
                    &detach,
                );
                if detach.clicked() {
                    self.detach_tab(tab, ctx);
                }
            }
        });
    }

    fn surface_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.themes.active().palette.paint_panel(ui);
        if self.foreign_modal(ctx) {
            ui.disable();
        }
        match self.workbench.surface {
            Surface::Terminal => self.terminal_content(ui, ctx),
            Surface::Tools => self.workbench_content(ui, ctx),
            Surface::Editor => {}
        }
    }

    /// Render one docked editor/tool/terminal surface with common focus and navigation boundaries.
    pub(in crate::app) fn workspace_content(&mut self, ctx: &egui::Context) {
        if self.navigation.zoom == Some(navigation::Pane::Explorer) {
            return;
        }
        if (self.workbench.detached || !self.tool_surface())
            && self.workbench.floating_tabs.is_empty()
        {
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

    /// Scope controller/UI ownership without changing docking or reattachment policy.
    fn in_tool_viewport(&mut self, ctx: &egui::Context, render: impl FnOnce(&mut Self)) {
        let previous = self.ui_viewport;
        self.ui_viewport = ctx.viewport_id();
        render(self);
        self.ui_viewport = previous;
    }

    /// Finish native tool viewports through the same modal, notice and focus boundary.
    fn finish_tool_viewport(&mut self, ctx: &egui::Context) {
        self.dialogs(ctx);
        self.workbench_windows(ctx);
        self.notice_ui(ctx);
        self.navigation_finish(ctx);
    }

    /// Closing the whole tools window docks it, retaining its selected surface.
    pub(in crate::app) fn detached_tools(&mut self, ctx: &egui::Context) {
        if !self.workbench.detached {
            return;
        }
        if !self.workbench.tools_hidden && !self.tool_surface() {
            self.dock_tools(ctx);
            return;
        }
        ctx.show_viewport_immediate(
            Self::tools_viewport(),
            detached_builder("Project tools").with_visible(!self.workbench.tools_hidden),
            |ctx, class| {
                if self.workbench.tools_hidden {
                    return;
                }
                if ctx.input(|input| input.viewport().close_requested()) {
                    self.dock_tools(ctx);
                    return;
                }
                self.in_tool_viewport(ctx, |app| {
                    if class == egui::ViewportClass::Embedded {
                        let mut open = true;
                        embedded_window("Project tools")
                            .open(&mut open)
                            .show(ctx, |ui| {
                                app.workspace_tabs(ui, ctx, true);
                                ui.separator();
                                app.surface_ui(ui, ctx);
                            });
                        if !open {
                            app.dock_tools(ctx);
                        }
                        // The root viewport already owns modal/notice finalization
                        // for this embedded whole-sidebar fallback.
                        app.workbench_windows(ctx);
                    } else {
                        app.detached_shortcuts(ctx);
                        let palette = app.themes.active().palette;
                        egui::TopBottomPanel::top("detached-tools-tabs")
                            .frame(theme::frame(palette.panel))
                            .show(ctx, |ui| app.workspace_tabs(ui, ctx, true));
                        egui::CentralPanel::default()
                            .frame(theme::frame(palette.panel))
                            .show(ctx, |ui| app.surface_ui(ui, ctx));
                        app.finish_tool_viewport(ctx);
                    }
                });
            },
        );
    }

    /// Each tab has a stable viewport; closing reattaches only that tab.
    pub(in crate::app) fn detached_tabs(&mut self, ctx: &egui::Context) {
        for tab in self.workbench.floating_tabs.clone() {
            ctx.show_viewport_immediate(
                tab.viewport(),
                detached_builder(tab.label()),
                |ctx, class| {
                    if ctx.input(|input| input.viewport().close_requested()) {
                        self.reattach_tool_tab(tab, ctx);
                        return;
                    }
                    self.in_tool_viewport(ctx, |app| {
                        if class == egui::ViewportClass::Embedded {
                            let mut open = true;
                            embedded_window(tab.label())
                                .id(egui::Id::new(("detached-tab", tab)))
                                .open(&mut open)
                                .show(ctx, |ui| {
                                    app.detached_tab_header(tab, ui, ctx);
                                    if app.tab_detached(tab) {
                                        app.tab_content(tab, ui, ctx);
                                    }
                                });
                            if !open {
                                app.reattach_tool_tab(tab, ctx);
                            }
                        } else {
                            app.detached_shortcuts(ctx);
                            let palette = app.themes.active().palette;
                            egui::TopBottomPanel::top("detached-tab-header")
                                .frame(theme::frame(palette.panel))
                                .show(ctx, |ui| app.detached_tab_header(tab, ui, ctx));
                            egui::CentralPanel::default()
                                .frame(theme::frame(palette.panel))
                                .show(ctx, |ui| {
                                    if app.tab_detached(tab) {
                                        app.tab_content(tab, ui, ctx);
                                    }
                                });
                        }
                        app.finish_tool_viewport(ctx);
                    });
                },
            );
        }
    }

    fn detached_tab_header(&mut self, tab: ToolTab, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.foreign_modal(ctx) {
            ui.disable();
        }
        ui.horizontal(|ui| {
            ui.strong(tab.label());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let reattach = ui.button("Reattach");
                ui_probe::button(
                    ui,
                    &format!("workspace-reattach:{}", tab.label()),
                    &reattach,
                );
                if reattach.clicked() {
                    self.reattach_tool_tab(tab, ctx);
                }
            });
        });
    }

    /// Apply the same modal input boundary in whole-sidebar and individual-tab windows.
    fn detached_shortcuts(&mut self, ctx: &egui::Context) {
        self.navigation_shortcuts(ctx);
        self.shortcuts(ctx);
        if self.dialog != Dialog::None || self.pending.is_some() || self.workbench_modal() {
            ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("terminal-input")));
        }
    }

    fn tab_content(&mut self, tab: ToolTab, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.themes.active().palette.paint_panel(ui);
        if self.foreign_modal(ctx) {
            ui.disable();
        }
        match tab {
            ToolTab::Terminal => self.terminal_content(ui, ctx),
            ToolTab::Panel(panel) => self.panel_content(panel, ui, ctx),
        }
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
    fn individual_tabs_keep_their_state_and_do_not_replace_other_windows_selections() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].text = "unsaved source".into();
        app.agents.chat.notice("retained conversation");
        app.toggle_panel(Panel::Agent, &ctx);
        app.detach_tab(ToolTab::Panel(Panel::Agent), &ctx);
        app.detach_tab(ToolTab::Panel(Panel::Settings), &ctx);
        app.toggle_panel(Panel::Monitor, &ctx);
        render(&mut app, &ctx);
        assert!(app.panel_visible(Panel::Agent));
        assert!(app.panel_visible(Panel::Settings));
        assert!(app.panel_visible(Panel::Monitor));
        assert_ne!(
            app.tool_viewport(ToolTab::Panel(Panel::Agent)),
            app.tool_viewport(ToolTab::Panel(Panel::Settings))
        );

        // Activity icons/shortcuts raise the floating tab without stealing the dock selection.
        app.toggle_panel(Panel::Agent, &ctx);
        app.detach_tab(ToolTab::Panel(Panel::Agent), &ctx);
        assert_eq!(app.workbench.panel, Some(Panel::Monitor));
        assert_eq!(app.workbench.floating_tabs.len(), 2);
        assert!(app.terminal.is_none());

        app.reattach_tool_tab(ToolTab::Panel(Panel::Agent), &ctx);
        assert_eq!(app.workbench.panel, Some(Panel::Agent));
        assert!(app.tools_visible());
        assert!(app.tab_detached(ToolTab::Panel(Panel::Settings)));
        assert_eq!(
            app.agents.chat.messages.back().unwrap().text,
            "retained conversation"
        );
        assert_eq!(app.documents[0].text, "unsaved source");
        assert!(app.documents[0].dirty());
    }

    #[test]
    fn detaching_an_unselected_terminal_keeps_the_sidebar_selection_and_visibility() {
        for (detached, hidden) in [(false, false), (false, true), (true, false), (true, true)] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            let mut workspace = crate::app::tests::remote_workspace();
            workspace.connected = false;
            app.workspace = Some(workspace);
            app.toggle_panel(Panel::Settings, &ctx);
            app.workbench.detached = detached;
            app.workbench.tools_hidden = hidden;

            // Even a failed shell startup must leave the other tool selected.
            app.detach_tab(ToolTab::Terminal, &ctx);
            assert_eq!(app.workbench.surface, Surface::Tools);
            assert_eq!(app.workbench.panel, Some(Panel::Settings));
            assert_eq!(app.workbench.tools_hidden, hidden);
            assert_eq!(app.tools_visible(), !hidden);
            assert_eq!(app.workbench.detached, detached);
            assert!(app.tab_detached(ToolTab::Terminal));
        }
    }

    #[test]
    fn individual_tabs_and_whole_sidebar_can_detach_and_dock_independently() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.toggle_panel(Panel::Settings, &ctx);
        app.workbench.detached = true;
        app.detach_tab(ToolTab::Panel(Panel::Settings), &ctx);
        assert!(app.workbench.detached);
        assert!(app.tools_visible());
        assert_ne!(app.workbench.panel, Some(Panel::Settings));
        app.reattach_tool_tab(ToolTab::Panel(Panel::Settings), &ctx);
        assert!(app.workbench.detached);
        assert_eq!(
            app.tool_viewport(ToolTab::Panel(Panel::Settings)),
            EditorApp::tools_viewport()
        );

        app.detach_tab(ToolTab::Panel(Panel::Settings), &ctx);
        app.dock_tools(&ctx);
        assert!(app.tab_detached(ToolTab::Panel(Panel::Settings)));
        assert!(!app.workbench.detached);
        app.workbench.reset_project();
        assert!(app.tab_detached(ToolTab::Panel(Panel::Settings)));
        app.reattach_tool_tab(ToolTab::Panel(Panel::Settings), &ctx);
        assert!(!app.tab_detached(ToolTab::Panel(Panel::Settings)));
        assert_eq!(app.workbench.panel, Some(Panel::Settings));
    }

    #[test]
    fn tab_dialog_owner_follows_only_its_own_tab_when_docking() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let settings = ToolTab::Panel(Panel::Settings);
        let chat = ToolTab::Panel(Panel::Agent);
        app.detach_tab(settings, &ctx);
        app.detach_tab(chat, &ctx);
        app.ui_viewport = app.tool_viewport(settings);
        app.set_dialog(Dialog::Shortcuts);
        app.workbench.detached = true;
        app.dock_tools(&ctx);
        assert_eq!(app.dialog_viewport, settings.viewport());
        app.reattach_tool_tab(chat, &ctx);
        assert_eq!(app.dialog_viewport, settings.viewport());
        assert!(app.foreign_modal(&ctx));
        app.reattach_tool_tab(settings, &ctx);
        assert_eq!(app.dialog_viewport, egui::ViewportId::ROOT);
        assert!(!app.foreign_modal(&ctx));
        assert!(app.dialog == Dialog::Shortcuts);
    }

    #[test]
    fn a_tool_command_from_a_floating_tab_focuses_the_requested_tools_host() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let chat = ToolTab::Panel(Panel::Agent);
        app.detach_tab(chat, &ctx);
        app.toggle_panel(Panel::Monitor, &ctx);
        let mut input = egui::RawInput {
            viewport_id: chat.viewport(),
            ..Default::default()
        };
        input.viewports.insert(chat.viewport(), Default::default());
        let output = ctx.run(input, |ctx| app.toggle_panel(Panel::Settings, ctx));
        assert_eq!(app.workbench.panel, Some(Panel::Settings));
        assert!(app.tools_visible());
        assert!(app.tab_detached(chat));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Focus))
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
        render(&mut app, &ctx);
        let code = app.navigation.rect(navigation::Pane::Editor).unwrap();
        app.toggle_panel(Panel::Settings, &ctx);
        app.workbench.detached = true;
        render(&mut app, &ctx);
        assert_eq!(app.navigation.rect(navigation::Pane::Editor), Some(code));
        app.focus_editor(&ctx);
        assert!(app.workbench.detached);
        assert_eq!(app.workbench.surface, Surface::Tools);
        app.toggle_sidebar(&ctx);
        render(&mut app, &ctx);
        assert!(app.workbench.tools_hidden);
        assert_eq!(app.navigation.rect(navigation::Pane::Editor), Some(code));
        app.toggle_sidebar(&ctx);
        app.dock_tools(&ctx);
        render(&mut app, &ctx);
        assert!(!app.workbench.detached);
        assert!(app.navigation.rect(navigation::Pane::Editor).is_none());
        assert!(app.navigation.rect(navigation::Pane::Tools).unwrap().top() > code.top());
        assert_eq!(app.workbench.panel, Some(Panel::Settings));
        assert_eq!(
            app.agents.chat.messages.back().unwrap().text,
            "retained conversation"
        );
    }
    fn shortcut(app: &mut EditorApp, ctx: &egui::Context, key: Key, modifiers: egui::Modifiers) {
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
    }

    #[test]
    fn sidebar_shortcut_remembers_chat_and_other_tabs_without_starting_a_terminal() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].text = "unsaved source".into();
        app.agents.chat.notice("retained conversation");
        // The first sidebar opening selects Chat without starting a shell or model turn.
        assert!(app.workbench.last_tool.is_none());
        shortcut(&mut app, &ctx, Key::Backtick, egui::Modifiers::CTRL);
        assert!(app.tools_visible());
        assert_eq!(app.workbench.panel, Some(Panel::Agent));
        assert!(app.agents.focus);
        assert!(app.terminal.is_none() && !app.agents.chat.busy);
        shortcut(&mut app, &ctx, Key::Backtick, egui::Modifiers::CTRL);
        assert_eq!(app.workbench.surface, Surface::Editor);
        for panel in [Panel::Agent, Panel::Settings, Panel::Monitor] {
            app.toggle_panel(panel, &ctx);
            shortcut(&mut app, &ctx, Key::Backtick, egui::Modifiers::CTRL);
            assert!(!app.tools_visible());
            assert_eq!(app.workbench.surface, Surface::Editor);
            // Ctrl+Shift+` is the physical Ctrl+~ chord on US keyboards.
            shortcut(
                &mut app,
                &ctx,
                Key::Backtick,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            );
            assert!(app.tools_visible());
            assert_eq!(app.workbench.panel, Some(panel));
            assert!(app.terminal.is_none());
        }
        assert_eq!(app.documents[0].text, "unsaved source");
        assert_eq!(
            app.agents.chat.messages.back().unwrap().text,
            "retained conversation"
        );
        // Selecting Code also preserves the last tools tab.
        app.focus_editor(&ctx);
        app.workbench.reset_project();
        app.toggle_sidebar(&ctx);
        assert_eq!(app.workbench.panel, Some(Panel::Monitor));
        app.navigation.zoom = Some(navigation::Pane::Editor);
        app.toggle_sidebar(&ctx);
        assert!(app.tools_visible());
        assert_eq!(app.workbench.panel, Some(Panel::Monitor));
    }

    #[test]
    fn closing_a_tool_and_hiding_a_detached_window_retain_the_selection() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.toggle_panel(Panel::Settings, &ctx);
        app.toggle_panel(Panel::Settings, &ctx);
        assert!(app.workbench.panel.is_none());
        app.toggle_sidebar(&ctx);
        assert!(app.tools_visible());
        assert_eq!(app.workbench.panel, Some(Panel::Settings));
        app.workbench.detached = true;
        app.toggle_sidebar(&ctx);
        assert!(app.workbench.detached);
        assert!(app.workbench.tools_hidden);
        assert!(!app.tools_visible());
        app.toggle_sidebar(&ctx);
        assert!(app.workbench.detached);
        assert!(!app.workbench.tools_hidden);
        assert!(app.tools_visible());
        assert_eq!(app.workbench.panel, Some(Panel::Settings));
    }

    #[test]
    fn terminal_shortcut_is_separate_and_dialogs_do_not_toggle_the_sidebar() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.show_terminal = true;
        app.terminal_intent();
        shortcut(
            &mut app,
            &ctx,
            Key::T,
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        );
        assert!(!app.terminal_visible());
        assert_eq!(app.workbench.last_tool, Some(ToolTab::Terminal));
        app.toggle_panel(Panel::Agent, &ctx);
        app.set_dialog(Dialog::Open);
        shortcut(&mut app, &ctx, Key::Backtick, egui::Modifiers::CTRL);
        assert!(app.tools_visible());
        assert_eq!(app.workbench.panel, Some(Panel::Agent));
    }
}

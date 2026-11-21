//! One selected docked surface plus optional sidebar and individual-tab viewports.
//! Each tool renders in exactly one host, sharing its existing controller and unsaved input.
//! Window placement and navigation belong to the application, independently of
//! project settings and caches. Project resets never reconstruct this registry.
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
/// Detachable view identity shared by root navigation and native window ownership.
pub(in crate::app) enum ToolTab {
    Terminal,
    Panel(Panel),
    /// The Connections sidebar keeps the same controller when detached.
    Connections,
    AppSettings,
}

impl ToolTab {
    pub(in crate::app) const ALL: [Self; 13] = [
        Self::Panel(Panel::Agent),
        Self::Terminal,
        Self::Panel(Panel::Git),
        Self::Panel(Panel::Monitor),
        Self::Panel(Panel::Problems),
        Self::Panel(Panel::Tasks),
        Self::Panel(Panel::SharedFolders),
        Self::Panel(Panel::Debug),
        Self::Panel(Panel::Settings),
        Self::Panel(Panel::Stocks),
        Self::Panel(Panel::Transfers),
        Self::Connections,
        Self::AppSettings,
    ];

    pub(in crate::app) fn command(self) -> Command {
        match self {
            Self::Terminal => Command::Terminal,
            Self::Panel(panel) => Command::Panel(panel),
            Self::Connections => Command::Connections,
            Self::AppSettings => Command::AppSettings,
        }
    }

    pub(in crate::app) fn for_command(command: Command) -> Option<Self> {
        match command {
            Command::Terminal => Some(Self::Terminal),
            Command::Panel(panel) => Some(Self::Panel(panel)),
            Command::Connections => Some(Self::Connections),
            Command::AppSettings => Some(Self::AppSettings),
            _ => None,
        }
    }

    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            Self::Connections | Self::AppSettings => self.command().definition().label,
            Self::Terminal => "Terminal",
            Self::Panel(Panel::Agent) => "Chat",
            Self::Panel(Panel::Git) => "Git",
            Self::Panel(Panel::Monitor) => "Monitor",
            Self::Panel(Panel::Problems) => "Problems",
            Self::Panel(Panel::Tasks) => "Tasks",
            Self::Panel(Panel::Settings) => "Settings",
            Self::Panel(Panel::Debug) => "Debugger",
            Self::Panel(Panel::Stocks) => "Stocks",
            Self::Panel(Panel::Transfers) => "Transfers",
            Self::Panel(Panel::SharedFolders) => "Shared folders",
        }
    }

    pub(in crate::app) fn scope(self) -> areas::Scope {
        match self {
            Self::Terminal => areas::Scope::Project,
            Self::Panel(panel) => panel.scope(),
            Self::Connections | Self::AppSettings => areas::Scope::Application,
        }
    }

    /// One ordered registry for project navigation and its fallback selection.
    fn project() -> impl Iterator<Item = Self> {
        Self::ALL
            .into_iter()
            .filter(|tab| tab.scope() == areas::Scope::Project)
    }
    /// Map a tool tab to the pane that owns its keyboard focus.
    pub(in crate::app) fn pane(self) -> navigation::Pane {
        match self {
            Self::Terminal => navigation::Pane::Terminal,
            Self::Connections => navigation::Pane::Connections,
            Self::Panel(_) | Self::AppSettings => navigation::Pane::Tools,
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

    /// Report whether this tab has its own presentation viewport.
    pub(in crate::app) fn tab_detached(&self, tab: ToolTab) -> bool {
        self.presentation.floating_tabs.contains(&tab)
    }

    /// Resolve the native owner for focus requests and modal dialogs.
    pub(in crate::app) fn tool_viewport(&self, tab: ToolTab) -> egui::ViewportId {
        if self.tab_detached(tab) {
            tab.viewport()
        } else if self.presentation.detached && tab.scope() == areas::Scope::Project {
            Self::tools_viewport()
        } else {
            egui::ViewportId::ROOT
        }
    }

    /// Resolve a detached tab by its stable viewport identity, independent of dock selection.
    pub(in crate::app) fn floating_tab(&self, viewport: egui::ViewportId) -> Option<ToolTab> {
        self.presentation
            .floating_tabs
            .iter()
            .copied()
            .find(|tab| tab.viewport() == viewport)
    }

    /// Background polling must include independently visible tabs, regardless of dock selection.
    pub(in crate::app) fn panel_visible(&self, panel: Panel) -> bool {
        self.tab_detached(ToolTab::Panel(panel))
            || self.presentation.area.panel() == Some(panel)
            || (self.tools_visible() && self.presentation.panel == Some(panel))
    }

    /// Report terminal visibility across the docked, combined and individual tool hosts.
    pub(in crate::app) fn terminal_any_visible(&self) -> bool {
        self.tab_detached(ToolTab::Terminal) || self.terminal_visible()
    }

    /// Raise a tab's existing host without changing the selection of any other window.
    pub(in crate::app) fn focus_tool_tab(&mut self, tab: ToolTab, ctx: &egui::Context) {
        self.editor.tabs.cancel_focus();
        self.presentation.remember(tab);
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
    pub(in crate::app) fn detach_tab(&mut self, tab: ToolTab, ctx: &egui::Context) {
        if self.tab_detached(tab) || !self.command_visible(tab.command()) {
            return;
        }
        if let ToolTab::Panel(panel) = tab {
            self.initialize_host_tool(panel);
        }
        let previous_owner = self.tool_viewport(tab);
        // Terminal startup records focus intent. Set its host first so it
        // cannot replace a different tool still selected in the sidebar.
        let selected = self.presentation.selected_tool() == Some(tab);
        self.presentation.detach_tab(tab);
        match tab {
            ToolTab::Terminal => {
                self.show_terminal = true;
                if self.terminal.is_none() && self.ssh_master.is_none() && !self.session_busy() {
                    self.start_project_terminal(ctx);
                }
            }
            ToolTab::Panel(panel) => self.refresh_project_panel(panel, ctx),
            ToolTab::Connections | ToolTab::AppSettings => {}
        }
        if self.dialog_viewport == previous_owner && self.dialog != Dialog::None {
            self.dialog_viewport = tab.viewport();
        }
        if selected && self.presentation.detached {
            // Keep the sidebar useful when one of its tabs moves to another window.
            if let Some(ToolTab::Panel(panel)) = ToolTab::project().find(|candidate| {
                matches!(candidate, ToolTab::Panel(panel) if self.panel_enabled(*panel))
                    && !self.tab_detached(*candidate)
            }) {
                self.toggle_panel(panel, ctx);
            } else {
                self.dock_tools(ctx);
            }
        }
        self.navigation.zoom = None;
        self.focus_tool_tab(tab, ctx);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Closing a tab window reattaches it; it never closes the underlying session.
    pub(in crate::app) fn reattach_tool_tab(&mut self, tab: ToolTab, ctx: &egui::Context) {
        keyboard::forget(ctx, tab.viewport());
        self.presentation.reattach_tab(tab);
        if self.dialog_viewport == tab.viewport() {
            self.dialog_viewport = self.tool_viewport(tab);
        }
        self.reveal_tool_tab(tab, ctx);
        ctx.send_viewport_cmd_to(self.tool_viewport(tab), egui::ViewportCommand::Focus);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Route the view to its own presentation destination, independently of its host.
    fn reveal_tool_tab(&mut self, tab: ToolTab, ctx: &egui::Context) {
        match tab {
            ToolTab::Terminal => self.focus_pane(navigation::Pane::Terminal, ctx),
            ToolTab::Panel(panel) => self.reveal_panel(panel, ctx),
            ToolTab::Connections => self.show_connections(ctx),
            ToolTab::AppSettings => self.open_area(areas::Area::Settings, ctx),
        }
    }

    /// Rail actions share the registry's detach, raise and reattach operations.
    pub(super) fn tool_window_menu(
        &mut self,
        ui: &mut egui::Ui,
        tab: ToolTab,
        ctx: &egui::Context,
    ) {
        if self.tab_detached(tab) {
            let show = crate::keyboard::button(ui, "Show window");
            ui_probe::button(ui, "area-show-window", &show);
            if show.clicked() {
                self.focus_tool_tab(tab, ctx);
                ui.close();
            }
            let reattach = crate::keyboard::button(ui, "Reattach to main window");
            ui_probe::button(ui, "area-reattach", &reattach);
            if reattach.clicked() {
                self.reattach_tool_tab(tab, ctx);
                ui.close();
            }
        } else {
            let detach = crate::keyboard::button(ui, "Open in separate window");
            ui_probe::button(ui, "area-detach", &detach);
            if detach.clicked() {
                self.detach_tab(tab, ctx);
                ui.close();
            }
        }
    }

    /// Record source-editor focus intent and select the docked editor without discarding tool state.
    pub(in crate::app) fn editor_intent(&mut self) {
        self.editor_intent_in(self.documents.viewport());
    }

    /// Record editor focus intent for a particular viewport and reveal its source surface.
    /// Child editor windows retain their own zoom; root tool state is changed only for the root
    /// host.
    pub(in crate::app) fn editor_intent_in(&mut self, viewport: egui::ViewportId) {
        if viewport == egui::ViewportId::ROOT {
            self.presentation.enter_area(areas::Area::Projects);
            if !self.presentation.detached {
                self.presentation.select_editor();
            }
            self.navigation.editor_intent();
        } else {
            self.set_pane_zoom(
                viewport,
                self.pane_zoom(viewport)
                    .filter(|pane| *pane == navigation::Pane::Editor),
            );
        }
        self.navigation
            .request_focus(navigation::Pane::Editor, viewport);
    }

    /// Select and reveal the terminal surface while preserving its running shell.
    pub(in crate::app) fn terminal_intent(&mut self) {
        self.presentation.select_terminal();
        self.navigation.terminal_intent();
        self.navigation.request_focus(
            navigation::Pane::Terminal,
            self.tool_viewport(ToolTab::Terminal),
        );
    }

    /// Hide or restore the last tool surface, initially Chat, updating pane focus
    /// and detached-window visibility.
    pub(in crate::app) fn toggle_sidebar(&mut self, ctx: &egui::Context) {
        let visible = self.presentation.area == areas::Area::Projects && self.tool_surface();
        if self.presentation.area != areas::Area::Projects {
            self.open_area(areas::Area::Projects, ctx);
        }
        self.navigation.zoom = None;
        if visible {
            self.presentation.remember_selected();
            if self.presentation.detached {
                self.presentation.hide_tools();
                keyboard::forget(ctx, Self::tools_viewport());
                ctx.send_viewport_cmd_to(
                    Self::tools_viewport(),
                    egui::ViewportCommand::Visible(false),
                );
            }
            self.focus_pane(navigation::Pane::Editor, ctx);
        } else {
            let enabled = |tab: &ToolTab| {
                tab.scope() == areas::Scope::Project
                    && match tab {
                        ToolTab::Panel(panel) => self.panel_enabled(*panel),
                        ToolTab::Terminal => true,
                        ToolTab::Connections | ToolTab::AppSettings => false,
                    }
            };
            let remembered = self.presentation.last_tool.filter(enabled);
            let docked_panel = |tab: &ToolTab| {
                matches!(tab, ToolTab::Panel(_)) && enabled(tab) && !self.tab_detached(*tab)
            };
            // Opening navigation must remain possible after its last tab moved
            // to a window or its extension was disabled. Prefer a docked panel;
            // if every panel is detached, raise an enabled window without
            // starting a shell for an unrelated, unavailable selection.
            let tab = remembered
                .filter(|tab| !self.tab_detached(*tab))
                .or_else(|| {
                    [Panel::Agent, Panel::Problems, Panel::Settings]
                        .into_iter()
                        .map(ToolTab::Panel)
                        .find(docked_panel)
                })
                .or_else(|| ToolTab::project().find(docked_panel))
                .or(remembered)
                .unwrap_or(ToolTab::Panel(Panel::Settings));
            match tab {
                ToolTab::Panel(panel) if self.panel_enabled(panel) => {
                    if self.presentation.surface == Surface::Tools
                        && self.presentation.panel == Some(panel)
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
        (self.presentation.area == areas::Area::Projects || self.presentation.detached)
            && !self.presentation.tools_hidden
            && self.presentation.surface == Surface::Tools
            && self.presentation.panel.is_some()
            && !self
                .presentation
                .panel
                .is_some_and(|panel| self.tab_detached(ToolTab::Panel(panel)))
            && (self.presentation.detached || self.navigation.visible(navigation::Pane::Tools))
    }

    /// Report effective terminal visibility without inferring whether its shell is still alive.
    pub(in crate::app) fn terminal_visible(&self) -> bool {
        (self.presentation.area == areas::Area::Projects || self.presentation.detached)
            && !self.presentation.tools_hidden
            && self.presentation.surface == Surface::Terminal
            && self.show_terminal
            && !self.tab_detached(ToolTab::Terminal)
            && (self.presentation.detached || self.navigation.visible(navigation::Pane::Terminal))
    }

    /// Report whether a shared tools or terminal surface is visible, rather than whether work is
    /// alive.
    pub(in crate::app) fn tool_surface(&self) -> bool {
        self.tools_visible() || self.terminal_visible()
    }

    /// Block input when an application modal belongs to another native viewport.
    pub(in crate::app) fn foreign_modal(&self, ctx: &egui::Context) -> bool {
        let owner = if self.dialog != Dialog::None || self.pending.modal() {
            Some(self.dialog_viewport)
        } else if self.workbench_modal() {
            Some(self.tool_viewport(ToolTab::Panel(Panel::Git)))
        } else {
            None
        };
        owner.is_some_and(|owner| owner != ctx.viewport_id())
    }

    fn dock_tools(&mut self, ctx: &egui::Context) {
        keyboard::forget(ctx, Self::tools_viewport());
        self.presentation.dock_tools();
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
        if pane == navigation::Pane::Tools && self.presentation.panel == Some(Panel::Agent) {
            self.agents.focus = true;
        }
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    fn project_tab_button(&mut self, tab: ToolTab, ui: &mut egui::Ui, ctx: &egui::Context) {
        let selected = self.presentation.selected_tool() == Some(tab);
        let response = ui.add_enabled(
            !self.tab_detached(tab),
            crate::keyboard::Button::selectable(selected, tab.label()),
        );
        ui_probe::button(ui, &format!("workspace-tab:{}", tab.label()), &response);
        crate::keyboard::context_menu(&response, |ui| {
            if crate::keyboard::button(ui, "Open in separate window").clicked() {
                self.detach_tab(tab, ctx);
                ui.close();
            }
        });
        if response.clicked() {
            self.reveal_tool_tab(tab, ctx);
        }
    }

    /// Tool hosts contain only tool tabs; root controls live in the main header.
    fn project_tabs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal_wrapped(|ui| {
            keyboard::group(ui, "workspace-tabs", |ui| {
                for tab in ToolTab::project() {
                    if matches!(tab, ToolTab::Panel(panel) if !self.panel_enabled(panel)) {
                        continue;
                    }
                    self.project_tab_button(tab, ui, ctx);
                }
            });
        });
    }

    /// Measure the actual themed button font and padding, including interface scale.
    fn workspace_button_width(ui: &egui::Ui, label: &str) -> f32 {
        egui::WidgetText::from(label)
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Button,
            )
            .size()
            .x
            .ceil()
            + 2. * ui.spacing().button_padding.x
    }

    fn workspace_window_buttons(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        dock: &str,
        tab: Option<ToolTab>,
    ) {
        if let Some(tab) = tab {
            let response = crate::keyboard::button(ui, "Detach tab")
                .on_hover_text(format!("Open {} in its own window", tab.label()));
            ui_probe::button(
                ui,
                &format!("workspace-detach-tab:{}", tab.label()),
                &response,
            );
            if response.clicked() {
                self.detach_tab(tab, ctx);
            }
        }
        let response =
            crate::keyboard::button(ui, dock).on_hover_text(if self.presentation.detached {
                "Return project tools to the main window"
            } else {
                "Move project tools into a separate window"
            });
        ui_probe::button(
            ui,
            if self.presentation.detached {
                "workspace-dock"
            } else {
                "workspace-detach"
            },
            &response,
        );
        if response.clicked() {
            if self.presentation.detached {
                self.dock_tools(ctx);
            } else {
                self.presentation.detach_tools();
                self.navigation.zoom = None;
                ctx.request_repaint();
            }
        }
        // Keep placement actions visually separate from the selectable project tabs.
        let (rule, separator) = ui.allocate_exact_size(
            egui::vec2(ui.spacing().item_spacing.x, layout::control_height(ui)),
            egui::Sense::hover(),
        );
        ui.painter().vline(
            rule.center().x,
            rule.y_range().shrink(3.),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        ui_probe::button(ui, "workspace-window-separator", &separator);
    }

    /// Both inline and wrapped rows allocate the same measured window controls.
    fn workspace_window_controls(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        dock: &str,
        tab: Option<ToolTab>,
        width: f32,
    ) {
        ui.allocate_ui_with_layout(
            egui::vec2(
                width.min(ui.available_width()),
                ui.spacing().interact_size.y,
            ),
            egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
            |ui| self.workspace_window_buttons(ui, ctx, dock, tab),
        );
    }

    /// Reserve the right edge for window controls; tabs wrap inside the remaining
    /// space. Very narrow hosts give the controls their own right-aligned row.
    fn workspace_tabs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, child: bool) {
        if self.foreign_modal(ctx) {
            ui.disable();
        }
        let dock = if child { "Dock back" } else { "Detach" };
        let tab = self.presentation.selected_tool();
        let spacing = ui.spacing().item_spacing.x;
        let actions_width = Self::workspace_button_width(ui, dock)
            + 2. * spacing
            + tab.map_or(0., |_| {
                Self::workspace_button_width(ui, "Detach tab") + spacing
            });
        let widest_tab = ToolTab::project()
            .map(|tab| Self::workspace_button_width(ui, tab.label()))
            .fold(0., f32::max);
        let inline = ui.available_width() >= actions_width + spacing + widest_tab;
        let tabs_width =
            (ui.available_width() - if inline { actions_width + spacing } else { 0. }).max(0.);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(tabs_width, ui.spacing().interact_size.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_width(tabs_width);
                    self.project_tabs(ui, ctx);
                },
            );
            if inline {
                self.workspace_window_controls(ui, ctx, dock, tab, actions_width);
            }
        });
        if !inline {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    self.workspace_window_controls(ui, ctx, dock, tab, actions_width);
                },
            );
        }
    }

    fn surface_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.themes.active().palette.paint_panel(ui);
        if self.foreign_modal(ctx) {
            ui.disable();
        }
        match self.presentation.surface {
            Surface::Terminal => self.terminal_content(ui, ctx),
            Surface::Tools => self.selected_panel_content(ui, ctx),
            Surface::Editor => {}
        }
    }

    /// Render one docked editor/tool/terminal surface with common focus and navigation boundaries.
    pub(in crate::app) fn workspace_content(&mut self, ctx: &egui::Context) {
        if self.navigation.zoom == Some(navigation::Pane::Connections) {
            return;
        }
        if self.presentation.area != areas::Area::Projects {
            self.global_content(ctx);
            return;
        }

        if self.navigation.zoom == Some(navigation::Pane::Explorer) {
            return;
        }
        let palette = self.themes.active().palette;
        // The editor needs no extra navigation row. Keep the row only in its
        // actual tool host, so hiding or detaching tools restores editor space.
        if !self.presentation.detached && self.tool_surface() {
            egui::TopBottomPanel::top("workspace-tabs")
                .frame(theme::frame(palette.panel))
                .show(ctx, |ui| {
                    self.in_editor(egui::ViewportId::ROOT, |app| {
                        app.workspace_tabs(ui, ctx, false)
                    })
                });
        }
        if self.presentation.detached || !self.tool_surface() {
            self.in_editor(egui::ViewportId::ROOT, |app| {
                app.git_gutter_tick(ctx);
                app.editor(ctx);
            });
        } else {
            egui::CentralPanel::default()
                .frame(layout::pane_frame(palette.panel))
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
        if !self.presentation.detached {
            return;
        }
        if !self.presentation.tools_hidden && !self.tool_surface() {
            self.dock_tools(ctx);
            return;
        }
        ctx.show_viewport_immediate(
            Self::tools_viewport(),
            detached_builder("Project tools").with_visible(!self.presentation.tools_hidden),
            |ctx, class| {
                if self.presentation.tools_hidden {
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
                            .frame(layout::pane_frame(palette.panel))
                            .show(ctx, |ui| app.surface_ui(ui, ctx));
                        app.finish_tool_viewport(ctx);
                    }
                });
            },
        );
    }

    /// Each tab has a stable viewport; closing reattaches only that tab.
    pub(in crate::app) fn detached_tabs(&mut self, ctx: &egui::Context) {
        for tab in self.presentation.floating_tabs.clone() {
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
                                .frame(layout::pane_frame(palette.panel))
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
                let reattach = crate::keyboard::button(ui, "Reattach");
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
        self.route_input(ctx);
        if self.dialog != Dialog::None || self.pending.modal() || self.workbench_modal() {
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
            ToolTab::Connections => self.connections_content(ui, ctx),
            ToolTab::AppSettings => self.area_content(areas::Area::Settings, ui, ctx),
        }
    }
}

impl EditorApp {
    /// Reveal a tool without toggling it off, including an independently detached tab.
    pub(in crate::app) fn reveal_panel(&mut self, panel: Panel, ctx: &egui::Context) {
        if let Some(area) = areas::Area::for_panel(panel) {
            self.open_area(area, ctx);
            return;
        }
        self.presentation.enter_area(areas::Area::Projects);
        if self.tab_detached(ToolTab::Panel(panel)) {
            self.focus_tool_tab(ToolTab::Panel(panel), ctx);
        } else if self.presentation.panel == Some(panel)
            && self.presentation.surface == Surface::Tools
        {
            self.focus_pane(navigation::Pane::Tools, ctx);
        } else {
            self.toggle_panel(panel, ctx);
        }
    }

    /// Select or hide a tool panel, refreshing its cached data when appropriate.
    /// An already selected detached panel is revealed and focused instead of hidden.
    pub(super) fn toggle_panel(&mut self, panel: Panel, ctx: &egui::Context) {
        if !self.panel_enabled(panel) {
            self.notify(
                "This extension is disabled. Enable it in View → Extensions.",
                false,
            );
            return;
        }
        if let Some(area) = areas::Area::for_panel(panel) {
            self.open_area(area, ctx);
            return;
        }
        let from_project = self.presentation.area == areas::Area::Projects;
        self.presentation.enter_area(areas::Area::Projects);
        self.presentation.remember(ToolTab::Panel(panel));
        if self.tab_detached(ToolTab::Panel(panel)) {
            self.focus_tool_tab(ToolTab::Panel(panel), ctx);
            return;
        }
        if self.presentation.detached
            && self.presentation.panel == Some(panel)
            && self.presentation.surface == Surface::Tools
        {
            self.presentation.show_tools();
            ctx.send_viewport_cmd_to(Self::tools_viewport(), egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd_to(Self::tools_viewport(), egui::ViewportCommand::Focus);
            return;
        }
        if from_project
            && self.presentation.panel == Some(panel)
            && self.presentation.surface == Surface::Tools
        {
            self.presentation.clear_panel();
        } else {
            self.presentation.select_panel(panel);
        }
        if self.presentation.panel.is_some() {
            self.refresh_project_panel(panel, ctx);
        }
        if panel == Panel::Agent && self.presentation.panel.is_some() {
            self.agents.focus = true;
        }
        self.focus_pane(
            if self.presentation.panel.is_some() {
                navigation::Pane::Tools
            } else {
                navigation::Pane::Editor
            },
            ctx,
        );
    }
}

#[cfg(test)]
mod tests;

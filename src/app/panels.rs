//! Panel rendering routes across application, host and project scopes. Shared
//! identities and metadata live in `tools`. Controllers retain state and activation guards;
//! selecting a view does not create another controller or widen its authority.
use super::*;

impl EditorApp {
    /// Render the active enabled tool view after checking host/project readiness.
    /// Monitoring needs only a connected host; project tools need a prepared project.
    pub(in crate::app) fn selected_panel_content(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        let Some(panel) = self.presentation.panel else {
            return;
        };
        self.panel_content(panel, ui, ctx);
    }

    /// Render a specific tool using its existing controller, independent of the dock selection.
    pub(in crate::app) fn panel_content(
        &mut self,
        panel: Panel,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        let mut content = self.pane_ui(ui, navigation::Pane::Tools);
        let ui = &mut content;
        if !self.panel_enabled(panel) {
            ui.label("This extension is disabled. Enable it in View → Extensions.");
            return;
        }
        let palette = self.themes.active().palette;
        if matches!(panel, Panel::Monitor | Panel::Transfers) {
            self.host_tool_header(panel, ui, ctx);
        }
        if panel == Panel::Transfers {
            self.transfers_ui(ui, ctx);
            return;
        }
        if panel == Panel::Stocks {
            layout::page(ui, 1740., |ui| self.stocks_ui(ui, ctx));
            return;
        }
        if panel == Panel::Debug {
            layout::page(ui, layout::PAGE_WIDTH, |ui| self.debugger_ui(ui, ctx));
            return;
        }
        if panel == Panel::Agent {
            self.agent_ui(ui, ctx);
            return;
        }
        layout::page(
            ui,
            if panel == Panel::Monitor {
                1740.
            } else {
                layout::PAGE_WIDTH
            },
            |ui| {
                let host = if panel == Panel::Monitor {
                    self.monitor_host_label()
                } else {
                    self.workspace
                        .as_ref()
                        .and_then(|w| w.remote.as_ref())
                        .map(|r| format!("SSH · {}", r.profile.endpoint()))
                        .unwrap_or_else(|| "LOCAL HOST".into())
                };
                let heading = ui
                    .add(egui::Label::new(palette.tag(&host)).truncate())
                    .on_hover_text(&host);
                ui_probe::button(ui, &format!("workbench-panel:{}", panel.title()), &heading);
                if panel == Panel::Monitor && !self.monitor_host_ready() {
                    layout::empty(
                        ui,
                        "Host disconnected",
                        "Connect or reconnect to the SSH host to view its system monitor.",
                        palette,
                    );
                    return;
                }
                if panel != Panel::Monitor && !self.project_ready() {
                    layout::empty(
                        ui,
                        "Open a project to get started",
                        "Open a project and connect to its host to use these tools.",
                        palette,
                    );
                    return;
                }
                if panel == Panel::Settings {
                    self.settings_ui(ui, ctx);
                    return;
                }
                if panel == Panel::SharedFolders && self.project_shared_folder_endpoint().is_none()
                {
                    layout::empty(
                        ui,
                        "Shared folders need an SSH project",
                        "Open a project on an SSH host to share folders with this computer.",
                        palette,
                    );
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt(("workbench-content", panel.title()))
                    // Keep the solid scrollbar gutter stable while content wraps at fractional zoom.
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .auto_shrink([false, false])
                    .show(ui, |ui| match panel {
                        Panel::Git => self.git_ui(ui, ctx),
                        Panel::Monitor => self.monitor_ui(ui),
                        Panel::Problems => self.problems_ui(ui, ctx),
                        Panel::Tasks => self.tasks_ui(ui, ctx),
                        Panel::SharedFolders => self.shared_folders_ui(ui, ctx),
                        Panel::Settings
                        | Panel::Agent
                        | Panel::Debug
                        | Panel::Stocks
                        | Panel::Transfers => {
                            unreachable!()
                        }
                    });
            },
        );
    }
}

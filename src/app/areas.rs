//! Application navigation is independent of project tool selection. Switching
//! areas changes presentation only; controllers, drafts and sessions keep one owner.
use super::*;

/// Presentation ownership only. Filesystem and process permissions remain with
/// the captured execution target or transfer endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    Application,
    Host,
    Project,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Area {
    #[default]
    Projects,
    Connections,
    Monitor,
    Transfers,
    Stocks,
    Settings,
}
impl Area {
    pub const RAIL: [Self; 5] = [
        Self::Projects,
        Self::Connections,
        Self::Monitor,
        Self::Transfers,
        Self::Stocks,
    ];
    pub fn command(self) -> Command {
        match self {
            Self::Projects => Command::Projects,
            Self::Connections => Command::Connections,
            Self::Monitor => Command::Panel(Panel::Monitor),
            Self::Transfers => Command::Panel(Panel::Transfers),
            Self::Stocks => Command::Panel(Panel::Stocks),
            Self::Settings => Command::AppSettings,
        }
    }
    pub fn for_panel(panel: Panel) -> Option<Self> {
        match panel {
            Panel::Monitor => Some(Self::Monitor),
            Panel::Transfers => Some(Self::Transfers),
            Panel::Stocks => Some(Self::Stocks),
            _ => None,
        }
    }
    pub fn panel(self) -> Option<Panel> {
        match self {
            Self::Monitor => Some(Panel::Monitor),
            Self::Transfers => Some(Panel::Transfers),
            Self::Stocks => Some(Panel::Stocks),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub pinned: Vec<String>,
    pub monitor_interval: u64,
    pub monitor_chart: workbench::monitoring::ChartView,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            pinned: Vec::new(),
            monitor_interval: 2,
            monitor_chart: Default::default(),
        }
    }
}
#[derive(Default)]
pub(super) struct State {
    pub active: Area,
    pub prefs: Preferences,
    pub pending: bool,
    saved: Preferences,
    pub monitor_configured: bool,
    saved_monitor: bool,
}
impl State {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let read = |key: &str| storage.and_then(|s| s.get_string(key));
        let interval = read("host_monitor_interval");
        let chart = read("host_monitor_chart");
        let configured = interval.is_some() || chart.is_some();
        let mut prefs = Preferences {
            pinned: read("project_toolbar_pins")
                .filter(|v| v.len() <= 1024)
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default(),
            monitor_interval: interval
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(2)
                .clamp(1, 30),
            monitor_chart: chart
                .filter(|v| v.len() <= 1024)
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default(),
        };
        let mut seen = HashSet::new();
        prefs.pinned.retain(|id| {
            workbench::ToolTab::ALL
                .iter()
                .any(|tab| tab.project_extra() && tab.label() == id)
                && seen.insert(id.clone())
        });
        prefs.pinned.truncate(2);
        Self {
            saved: prefs.clone(),
            prefs,
            monitor_configured: configured,
            saved_monitor: configured,
            ..Default::default()
        }
    }
    /// Each explicit setting updates its own preference field, so another window's
    /// unrelated pin or monitor edit is not overwritten by an old snapshot.
    pub fn store(&mut self, storage: &mut dyn eframe::Storage) {
        if !self.pending {
            return;
        }
        if self.prefs.pinned != self.saved.pinned {
            storage.set_string(
                "project_toolbar_pins",
                serde_json::to_string(&self.prefs.pinned).unwrap(),
            );
        }
        if self.monitor_configured {
            if !self.saved_monitor || self.prefs.monitor_interval != self.saved.monitor_interval {
                storage.set_string(
                    "host_monitor_interval",
                    self.prefs.monitor_interval.to_string(),
                );
            }
            if !self.saved_monitor || self.prefs.monitor_chart != self.saved.monitor_chart {
                storage.set_string(
                    "host_monitor_chart",
                    serde_json::to_string(&self.prefs.monitor_chart).unwrap(),
                );
            }
        }
        self.saved = self.prefs.clone();
        self.saved_monitor = self.monitor_configured;
        self.pending = false;
    }
    pub fn pin(&mut self, tab: workbench::ToolTab, pinned: bool) {
        self.prefs.pinned.retain(|id| id != tab.label());
        if pinned && self.prefs.pinned.len() < 2 && tab.project_extra() {
            self.prefs.pinned.push(tab.label().into());
        }
        self.pending = true;
    }
}

impl EditorApp {
    /// Do not mutate the selected project surface while visiting a global area.
    pub(super) fn open_area(&mut self, area: Area, ctx: &egui::Context) {
        if !self.command_visible(area.command()) {
            return;
        }
        self.areas.active = area;
        self.navigation.zoom = None;
        if let Some(panel) = area.panel() {
            if self.tab_detached(workbench::ToolTab::Panel(panel)) {
                self.focus_tool_tab(workbench::ToolTab::Panel(panel), ctx);
                return;
            }
            if matches!(panel, Panel::Monitor | Panel::Transfers) {
                self.initialize_host_tool(panel, ctx);
            }
        }
        if area == Area::Projects {
            let pane = match self.workbench.surface {
                workbench::Surface::Terminal => navigation::Pane::Terminal,
                workbench::Surface::Tools if self.workbench.panel.is_some() => {
                    navigation::Pane::Tools
                }
                _ => navigation::Pane::Editor,
            };
            self.focus_pane(pane, ctx);
            if self.workbench.panel == Some(Panel::Agent) {
                self.agents.focus = true;
            }
        } else {
            self.navigation
                .request_focus(navigation::Pane::Tools, egui::ViewportId::ROOT);
        }
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
    pub(super) fn global_content(&mut self, ctx: &egui::Context) {
        let area = self.areas.active;
        let palette = self.themes.active().palette;
        egui::CentralPanel::default()
            .frame(layout::pane_frame(palette.panel))
            .show(ctx, |ui| {
                if let Some(panel) = area.panel() {
                    if self.tab_detached(workbench::ToolTab::Panel(panel)) {
                        let mut ui = self.pane_ui(ui, navigation::Pane::Tools);
                        ui.label(format!(
                            "{} is open in its own window.",
                            area.command().definition().label
                        ));
                        if crate::keyboard::button(&mut ui, "Show window").clicked() {
                            self.focus_tool_tab(workbench::ToolTab::Panel(panel), ctx);
                        }
                    } else {
                        self.panel_content(panel, ui, ctx);
                    }
                    return;
                }
                let mut ui = self.pane_ui(ui, navigation::Pane::Tools);
                layout::page(&mut ui, layout::PAGE_WIDTH, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("application-area", area.command().definition().label))
                        .show(ui, |ui| match area {
                            Area::Connections => {
                                layout::heading(
                                    ui,
                                    "Connections",
                                    "Saved SSH connections and host administration",
                                    palette,
                                );
                                ui.horizontal_wrapped(|ui| {
                                    if ui
                                        .add(IconButton::new(Icon::Remote, "New connection…"))
                                        .clicked()
                                    {
                                        self.set_dialog(Dialog::Connect);
                                    }
                                    if crate::keyboard::button(ui, "SSH tunnels…").clicked() {
                                        self.set_dialog(Dialog::Tunnels);
                                    }
                                    if self.workspace.as_ref().is_some_and(|w| w.remote.is_some()) {
                                        if crate::keyboard::button(ui, "Reconnect").clicked() {
                                            self.reconnect_ssh(ctx);
                                        }
                                        if crate::keyboard::button(ui, "Disconnect").clicked() {
                                            self.disconnect_ssh(ctx);
                                        }
                                    }
                                });
                                ui.separator();
                                self.saved_connections_ui(ui, ctx);
                                if self.connection_profiles.profiles.is_empty() {
                                    ui.weak(
                                        "No saved connections. Add a connection to get started.",
                                    );
                                }
                            }
                            Area::Settings => self.application_settings_ui(ui, ctx),
                            _ => {}
                        });
                });
            });
    }
    fn application_settings_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.migrate_monitor_preferences();
        let palette = self.themes.active().palette;
        layout::heading(
            ui,
            "App settings",
            "Appearance, navigation and host monitoring",
            palette,
        );
        crate::keyboard::menu_button(ui, "Interface theme", |ui| self.theme_menu(ui, ctx));
        crate::keyboard::menu_button(ui, "Syntax theme", |ui| self.syntax_theme_menu(ui, ctx));
        self.ui_scale_menu(ui, ctx);
        ui.horizontal_wrapped(|ui| {
            if crate::keyboard::button(ui, "Keyboard shortcuts…").clicked() {
                self.set_dialog(Dialog::Shortcuts);
            }
            if crate::keyboard::button(ui, "Extensions…").clicked() {
                self.set_dialog(Dialog::Extensions);
            }
        });
        ui.separator();
        if self.feature(crate::extensions::Feature::Monitor) {
            ui.strong("Host monitor");
            if ui
                .add(crate::keyboard::named(
                    "Monitor refresh interval",
                    egui::Slider::new(&mut self.areas.prefs.monitor_interval, 1..=30)
                        .text("Refresh interval (s)"),
                ))
                .changed()
            {
                self.areas.pending = true;
            }
            let previous = self.areas.prefs.monitor_chart;
            self.areas.prefs.monitor_chart.controls(ui);
            self.areas.pending |= previous != self.areas.prefs.monitor_chart;
            ui.weak(
                "Monitoring starts only when you press Start. Its host is selected in Monitor.",
            );
        }
        ui.separator();
        ui.strong("Project toolbar");
        ui.weak("Pin up to two additional tools. Other tools remain in the Tools menu.");
        for tab in workbench::ToolTab::ALL
            .into_iter()
            .filter(|tab| tab.project_extra())
        {
            if let workbench::ToolTab::Panel(panel) = tab
                && !self.panel_enabled(panel)
            {
                continue;
            }
            let mut pinned = self.areas.prefs.pinned.iter().any(|id| id == tab.label());
            let enabled = pinned || self.areas.prefs.pinned.len() < 2;
            if ui
                .add_enabled_ui(enabled, |ui| {
                    crate::keyboard::checkbox(ui, &mut pinned, tab.label())
                })
                .inner
                .changed()
            {
                self.areas.pin(tab, pinned);
            }
        }
    }
}

#[cfg(test)]
mod tests;

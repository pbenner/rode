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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
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

    /// One existing window owner for each detachable application area. Projects
    /// keeps its editor/tool hosts instead of duplicating the entire workspace.
    pub fn tab(self) -> Option<workbench::ToolTab> {
        match self {
            Self::Projects => None,
            Self::Connections | Self::Settings => Some(workbench::ToolTab::Area(self)),
            _ => self.panel().map(workbench::ToolTab::Panel),
        }
    }

    pub fn for_command(command: Command) -> Option<Self> {
        match command {
            Command::Projects => Some(Self::Projects),
            Command::Connections => Some(Self::Connections),
            Command::AppSettings => Some(Self::Settings),
            Command::Panel(panel) => Self::for_panel(panel),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub monitor_interval: u64,
    pub monitor_chart: workbench::monitoring::ChartView,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
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
        let prefs = Preferences {
            monitor_interval: interval
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(2)
                .clamp(1, 30),
            monitor_chart: chart
                .filter(|v| v.len() <= 1024)
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default(),
        };
        Self {
            saved: prefs.clone(),
            prefs,
            monitor_configured: configured,
            saved_monitor: configured,
            ..Default::default()
        }
    }
    /// Each explicit setting updates its own field, preserving unrelated monitor
    /// edits made by another window.
    pub fn store(&mut self, storage: &mut dyn eframe::Storage) {
        if !self.pending {
            return;
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
}

impl EditorApp {
    pub(super) fn area_detached(&self, area: Area) -> bool {
        area.tab().is_some_and(|tab| self.tab_detached(tab))
    }

    /// Move presentation into the shared viewport registry. Host bindings and
    /// controllers remain in place, and the main window returns to its project.
    pub(super) fn detach_area(&mut self, area: Area, ctx: &egui::Context) {
        let Some(tab) = area.tab().filter(|_| self.command_visible(area.command())) else {
            return;
        };
        if let Some(panel) = area.panel() {
            self.initialize_host_tool(panel, ctx);
        }
        self.detach_tab(tab, ctx);
        if self.areas.active == area {
            self.areas.active = Area::Projects;
            self.navigation.zoom = None;
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    /// Shared rail actions work with the keyboard context-menu shortcut too.
    pub(super) fn area_window_menu(&mut self, ui: &mut egui::Ui, area: Area, ctx: &egui::Context) {
        let Some(tab) = area.tab() else {
            return;
        };
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
                self.detach_area(area, ctx);
                ui.close();
            }
        }
    }

    /// Do not mutate the selected project surface while visiting a global area.
    pub(super) fn open_area(&mut self, area: Area, ctx: &egui::Context) {
        if !self.command_visible(area.command()) {
            return;
        }
        if let Some(tab) = area.tab().filter(|tab| self.tab_detached(*tab)) {
            // Raising a floating area must not replace the root's current view.
            self.focus_tool_tab(tab, ctx);
            return;
        }
        self.areas.active = area;
        self.navigation.zoom = None;
        if let Some(panel) = area.panel() {
            self.initialize_host_tool(panel, ctx);
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
                if self.area_detached(area) {
                    let mut ui = self.pane_ui(ui, navigation::Pane::Tools);
                    ui.label(format!(
                        "{} is open in its own window.",
                        area.command().definition().label
                    ));
                    self.area_window_menu(&mut ui, area, ctx);
                } else {
                    self.area_content(area, ui, ctx);
                }
            });
    }

    /// Render an area in its sole current host, using the same application state.
    pub(in crate::app) fn area_content(
        &mut self,
        area: Area,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        if let Some(panel) = area.panel() {
            self.panel_content(panel, ui, ctx);
            return;
        }
        let mut ui = self.pane_ui(ui, navigation::Pane::Tools);
        layout::page(&mut ui, layout::PAGE_WIDTH, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("application-area", area.command().definition().label))
                .show(ui, |ui| match area {
                    Area::Connections => self.connections_ui(ui, ctx),
                    Area::Settings => self.application_settings_ui(ui, ctx),
                    _ => {}
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
    }
}

#[cfg(test)]
mod tests;

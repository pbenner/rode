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
    Projects,
    Monitor,
    Transfers,
    Stocks,
    Settings,
    #[default]
    Welcome,
}
impl Area {
    pub fn command(self) -> Command {
        match self {
            Self::Welcome | Self::Projects => Command::Projects,
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
    pub fn tab(self) -> Option<ToolTab> {
        match self {
            Self::Welcome | Self::Projects => None,
            Self::Settings => Some(ToolTab::AppSettings),
            _ => self.panel().map(ToolTab::Panel),
        }
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub monitor_interval: u64,
    pub monitor_chart: monitoring::ChartView,
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
    /// Welcome is a presentation choice, never a project close. Keep document,
    /// tool and detached-window owners intact when deselecting Projects.
    pub(super) fn toggle_projects(&mut self, ctx: &egui::Context) {
        self.open_area(
            if self.presentation.area == Area::Projects {
                Area::Welcome
            } else {
                Area::Projects
            },
            ctx,
        );
    }

    pub(super) fn area_detached(&self, area: Area) -> bool {
        area.tab().is_some_and(|tab| self.tab_detached(tab))
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
        self.presentation.enter_area(area);
        self.navigation.zoom = None;
        if let Some(panel) = area.panel() {
            self.initialize_host_tool(panel);
        }
        if area == Area::Projects {
            let pane = match self.presentation.surface {
                tool_windows::Surface::Terminal => navigation::Pane::Terminal,
                tool_windows::Surface::Tools if self.presentation.panel.is_some() => {
                    navigation::Pane::Tools
                }
                _ => navigation::Pane::Editor,
            };
            self.focus_pane(pane, ctx);
            if pane == navigation::Pane::Tools && self.presentation.panel == Some(Panel::Agent) {
                self.agents.focus = true;
            }
        } else {
            self.navigation
                .request_focus(navigation::Pane::Tools, egui::ViewportId::ROOT);
        }
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
    pub(super) fn global_content(&mut self, ctx: &egui::Context) {
        let area = self.presentation.area;
        let palette = self.themes.active().palette;
        egui::CentralPanel::default()
            .frame(layout::pane_frame(if area == Area::Welcome {
                palette.background
            } else {
                palette.panel
            }))
            .show(ctx, |ui| self.area_content(area, ui, ctx));
    }

    /// Render an area in its sole current host, using the same application state.
    pub(in crate::app) fn area_content(
        &mut self,
        area: Area,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        match area {
            Area::Welcome => {
                let mut ui = self.pane_ui(ui, navigation::Pane::Tools);
                self.welcome(&mut ui);
            }
            Area::Settings => {
                let mut ui = self.pane_ui(ui, navigation::Pane::Tools);
                layout::page(&mut ui, layout::PAGE_WIDTH, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("application-area", area.command().definition().label))
                        .show(ui, |ui| self.application_settings_ui(ui, ctx));
                });
            }
            Area::Monitor | Area::Transfers | Area::Stocks => {
                self.panel_content(area.panel().unwrap(), ui, ctx);
            }
            Area::Projects => unreachable!("Projects render through workspace_content"),
        }
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

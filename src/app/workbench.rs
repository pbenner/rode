//! Project-tool state, captured preferences and background-result routing.
//!
//! Each domain submodule renders its own panel. Jobs capture project and tool versions before
//! dispatch; stale formatting/diagnostics cannot replace a newer buffer. Shared preferences
//! contain settings only, while queues, telemetry histories and transcripts remain
//! window-owned.

mod code_tools;
mod environment;
mod git;
mod jobs;
mod monitoring;
mod python;
mod sandbox_access;
mod settings;
mod workspace;
use super::*;
use crate::project::{self, Diagnostic};
pub(super) use sandbox_access::{options_ui as sandbox_options_ui, rules_ui as sandbox_rules_ui};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub(super) use workspace::{Surface, ToolTab};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// Tool-view identity used by tabs, commands and feature gates; it carries no running job.
pub(super) enum Panel {
    Git,
    Monitor,
    Problems,
    Environment,
    Tasks,
    Settings,
    Agent,
    Debug,
}
impl Panel {
    fn title(self) -> &'static str {
        match self {
            Self::Git => "SOURCE CONTROL",
            Self::Monitor => "SYSTEM MONITOR",
            Self::Problems => "PROBLEMS",
            Self::Environment => "ENVIRONMENT",
            Self::Tasks => "TASKS",
            Self::Settings => "PROJECT SETTINGS",
            Self::Agent => "AGENT CHAT",
            Self::Debug => "DEBUGGER",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
/// Serializable settings for one host/project key. `prefs` is applied state;
/// `draft` can change independently until settings are accepted.
pub(super) struct Preferences {
    /// Host interpreter selection: `auto`, an explicit path, or an empty system selection.
    python: String,
    environment: crate::environment::Settings,
    ruff: String,
    format_on_save: bool,
    check_on_save: bool,
    live_checks: bool,
    /// Requested sampling seconds, clamped to 1–30 when scheduling host telemetry.
    monitor_interval: u64,
    /// Requested grants; only the prepared workspace plan establishes actual isolation.
    sandbox: sandbox::Options,
    slurm_queue: String,
    slurm_cancel: String,
    slurm_allocation: crate::slurm::Allocation,
    pub(super) marp: crate::marp::Settings,
    pub(super) latex: crate::latex::Settings,
    pub(super) debugger: crate::debugger::Settings,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            python: "auto".into(),
            environment: Default::default(),
            ruff: String::new(),
            format_on_save: true,
            check_on_save: true,
            live_checks: true,
            monitor_interval: 2,
            sandbox: Default::default(),
            slurm_queue: "squeue".into(),
            slurm_cancel: "scancel".into(),
            slurm_allocation: Default::default(),
            marp: Default::default(),
            latex: Default::default(),
            debugger: Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// Selected source-control view; each view has a separately refreshed cached result.
enum GitTab {
    #[default]
    Changes,
    History,
    Branches,
    Stash,
    Remotes,
    Tags,
    Worktrees,
    Submodules,
    Settings,
}

#[derive(Clone, Debug)]
/// Captured Git completion intent, including the path/staging side of a requested diff.
pub(super) enum GitJob {
    Root,
    Status,
    Tab,
    Diff { path: String, staged: bool },
    Show,
    Mutate { label: String, clear_message: bool },
    Identity(git::identity::Job),
}
#[derive(Clone, Debug)]
/// UI completion metadata for a project-service request. Analysis carries the
/// submitted document/version/epoch so stale replies cannot overwrite newer edits.
pub(super) enum ToolJob {
    Mise {
        action: crate::mise::Action,
        revision: u64,
        settings: crate::environment::Settings,
    },
    Extension {
        revision: u64,
        operation: String,
    },
    Monitor,
    Runtimes {
        settings: crate::environment::Settings,
    },
    Git(GitJob),
    Environment {
        activate: bool,
        selected: String,
    },
    CreateEnvironment {
        path: String,
    },
    Analyze {
        tool: String,
        id: u64,
        version: u64,
        epoch: u64,
        original: String,
        path: String,
        save: Option<Option<String>>,
        manual: bool,
    },
}
#[derive(Clone)]
/// Diagnostics for one source version; later edits make this snapshot obsolete.
struct Issues {
    version: u64,
    items: Vec<Diagnostic>,
}
/// Cached text and visibility of a path-specific staged or working-tree diff.
struct Diff {
    path: String,
    staged: bool,
    text: String,
    open: bool,
}

#[derive(Default)]
/// Window-owned tool state with a persisted settings map and a separate editable draft.
/// Runtime epochs invalidate pending format/check results without rewriting source.
pub(super) struct Workbench {
    pub panel: Option<Panel>,
    pub surface: Surface,
    pub detached: bool,
    pub tools_hidden: bool,
    /// Tabs hosted in their own native windows, independent of the whole sidebar.
    floating_tabs: Vec<workspace::ToolTab>,
    last_tool: Option<workspace::ToolTab>,
    preferences: BTreeMap<String, Preferences>,
    pub preferences_pending: bool,
    key: String,
    /// Applied settings used when capturing a new tool invocation.
    prefs: Preferences,
    /// Editable form state; invalid or unapplied edits must not affect running jobs.
    draft: Preferences,
    /// Tool-setting generation paired with document versions on analysis requests.
    epoch: u64,
    python: python::State,
    environment: environment::State,
    code_tools: code_tools::State,
    monitoring: monitoring::State,
    git: git::State,
}

impl Workbench {
    /// Return the applied literal variables and runtime overrides for newly launched tools.
    pub(super) fn environment_settings(&self) -> &crate::environment::Settings {
        &self.prefs.environment
    }
    /// Return saved allocation defaults; this does not create a scheduler reservation.
    pub(super) fn slurm_allocation(&self) -> &crate::slurm::Allocation {
        &self.prefs.slurm_allocation
    }
    /// Apply allocation defaults to both settings copies and queue a preference-map update.
    pub(super) fn save_slurm_allocation(&mut self, settings: crate::slurm::Allocation) {
        self.prefs.slurm_allocation = settings.clone();
        self.draft.slurm_allocation = settings;
        self.save_preferences();
    }
    /// Return applied debugger launch/attach configurations for this project.
    pub(super) fn debugger_settings(&self) -> &crate::debugger::Settings {
        &self.prefs.debugger
    }
    /// Borrow the live debugger settings for an edit that must subsequently be committed.
    pub(super) fn edit_debugger_settings(&mut self) -> &mut crate::debugger::Settings {
        &mut self.prefs.debugger
    }
    /// Validate edited debugger settings before persistence. Failure restores the last
    /// accepted draft and returns the validation error.
    pub(super) fn commit_debugger_settings(&mut self) -> anyhow::Result<()> {
        if let Err(error) = self.prefs.debugger.validate() {
            self.prefs.debugger = self.draft.debugger.clone();
            return Err(error);
        }
        self.draft.debugger = self.prefs.debugger.clone();
        self.save_preferences();
        Ok(())
    }
    /// Replace both debugger settings copies and mark project preferences for persistence.
    pub(super) fn save_debugger_settings(&mut self, settings: crate::debugger::Settings) {
        self.prefs.debugger = settings.clone();
        self.draft.debugger = settings;
        self.save_preferences();
    }

    /// Return the applied compiler, build and preview settings for LaTeX operations.
    pub(super) fn latex_settings(&self) -> &crate::latex::Settings {
        &self.prefs.latex
    }
    /// Replace a document's diagnostics with issues tied to the supplied source version.
    pub(super) fn set_build_issues(&mut self, id: u64, version: u64, items: Vec<Diagnostic>) {
        self.code_tools.issues.insert(id, Issues { version, items });
    }
    /// Return applied presentation settings without launching the installed renderer.
    pub(super) fn marp_settings(&self) -> &crate::marp::Settings {
        &self.prefs.marp
    }
    /// Advance the tool epoch and clear queued analysis so changed settings or host contexts
    /// cannot accept old diagnostics/formatting.
    pub(super) fn invalidate_tools(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.code_tools.issues.clear();
        self.code_tools.checked.clear();
        self.code_tools.observed = None;
        self.code_tools.unavailable_tools.clear();
    }
    /// Restore the saved per-project settings map; transient tool state starts empty.
    pub(super) fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        Self {
            preferences: storage
                .and_then(|s| s.get_string("project_preferences"))
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            ..Default::default()
        }
    }
    /// Serialize the settings map into shared preference storage without runtime state.
    pub(super) fn store(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(text) = serde_json::to_string(&self.preferences) {
            storage.set_string("project_preferences", text);
        }
    }
    /// Discard project runtime caches while retaining saved preferences and workspace-view choices.
    pub(super) fn reset_project(&mut self) {
        let preferences = std::mem::take(&mut self.preferences);
        let pending = self.preferences_pending;
        let panel = self.panel;
        let surface = self.surface;
        let detached = self.detached;
        let tools_hidden = self.tools_hidden;
        let last_tool = self.last_tool;
        let floating_tabs = std::mem::take(&mut self.floating_tabs);
        *self = Self {
            preferences,
            preferences_pending: pending,
            panel,
            surface,
            detached,
            tools_hidden,
            last_tool,
            floating_tabs,
            ..Default::default()
        };
    }
    /// Clear host telemetry and rate baselines when changing execution hosts.
    pub(super) fn reset_monitor(&mut self) {
        self.monitoring = Default::default();
    }
    /// Snapshot applied settings under the current project key and flag an asynchronous save.
    /// An empty key means no project settings may be persisted yet.
    fn save_preferences(&mut self) {
        if self.key.is_empty() {
            return;
        }
        self.preferences
            .insert(self.key.clone(), self.prefs.clone());
        self.preferences_pending = true;
    }
}

impl EditorApp {
    /// Report a source-control confirmation that must participate in global input guards.
    pub(super) fn workbench_modal(&self) -> bool {
        self.workbench.git.confirm.is_some()
    }

    /// Render the active enabled tool view after checking host/project readiness.
    /// Monitoring needs only a connected host; project tools need a prepared project.
    pub(in crate::app) fn workbench_content(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(panel) = self.workbench.panel else {
            return;
        };
        self.panel_content(panel, ui, ctx);
    }

    /// Render a specific tool using its existing controller, independent of the dock selection.
    fn panel_content(&mut self, panel: Panel, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.panel_enabled(panel) {
            ui.label("This extension is disabled. Enable it in View → Extensions.");
            return;
        }
        let palette = self.themes.active().palette;
        self.pane_ui(ui, navigation::Pane::Tools);
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
                let host = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.remote.as_ref())
                    .map(|r| format!("SSH · {}", r.profile.endpoint()))
                    .unwrap_or_else(|| "LOCAL HOST".into());
                ui.add(egui::Label::new(palette.tag(&host)).truncate())
                    .on_hover_text(&host);
                if panel == Panel::Monitor && !self.monitor_ready() {
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
                egui::ScrollArea::vertical()
                    .id_salt(("workbench-content", panel.title()))
                    .auto_shrink([false, false])
                    .show(ui, |ui| match panel {
                        Panel::Git => self.git_ui(ui, ctx),
                        Panel::Monitor => self.monitor_ui(ui),
                        Panel::Problems => self.problems_ui(ui, ctx),
                        Panel::Environment => self.environment_ui(ui, ctx),
                        Panel::Tasks => self.tasks_ui(ui, ctx),
                        Panel::Settings => self.settings_ui(ui, ctx),
                        Panel::Agent | Panel::Debug => unreachable!(),
                    });
            },
        );
    }

    /// Reveal a tool without toggling it off, including an independently detached tab.
    pub(in crate::app) fn reveal_panel(&mut self, panel: Panel, ctx: &egui::Context) {
        if self.tab_detached(ToolTab::Panel(panel)) {
            self.focus_tool_tab(ToolTab::Panel(panel), ctx);
        } else if self.workbench.panel == Some(panel) && self.workbench.surface == Surface::Tools {
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
        self.workbench.last_tool = Some(workspace::ToolTab::Panel(panel));
        if self.tab_detached(workspace::ToolTab::Panel(panel)) {
            self.focus_tool_tab(workspace::ToolTab::Panel(panel), ctx);
            return;
        }
        if self.workbench.detached
            && self.workbench.panel == Some(panel)
            && self.workbench.surface == Surface::Tools
        {
            self.workbench.tools_hidden = false;
            ctx.send_viewport_cmd_to(Self::tools_viewport(), egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd_to(Self::tools_viewport(), egui::ViewportCommand::Focus);
            return;
        }
        self.workbench.panel =
            if self.workbench.panel == Some(panel) && self.workbench.surface == Surface::Tools {
                None
            } else {
                Some(panel)
            };
        if panel == Panel::Git && self.workbench.panel.is_some() {
            self.refresh_git(ctx);
        }
        if matches!(panel, Panel::Environment | Panel::Tasks) && self.workbench.panel.is_some() {
            self.refresh_environment(false, ctx);
            self.refresh_runtimes(ctx);
        }
        if panel == Panel::Agent && self.workbench.panel.is_some() {
            self.agents.focus = true;
        }
        self.focus_pane(
            if self.workbench.panel.is_some() {
                navigation::Pane::Tools
            } else {
                navigation::Pane::Editor
            },
            ctx,
        );
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_owned()).collect()
}
fn bytes(value: u64) -> String {
    if value >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", value as f64 / 1073741824.)
    } else if value >= 1024 * 1024 {
        format!("{:.1} MiB", value as f64 / 1048576.)
    } else {
        format!("{:.1} KiB", value as f64 / 1024.)
    }
}

/// Compact shared selector for project opening, SSH profiles and saved access rules.
pub(super) fn sandbox_backend_ui(ui: &mut egui::Ui, options: &mut sandbox::Options) {
    ui.checkbox(&mut options.enabled, "Sandbox project when available");
    ui.add_enabled_ui(options.enabled, |ui| {
        egui::ComboBox::from_id_salt(ui.id().with("sandbox-backend"))
            .selected_text(options.backend.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut options.backend, sandbox::Backend::Bwrap, "bwrap");
                ui.selectable_value(&mut options.backend, sandbox::Backend::Landlock, "Landlock");
            }).response.on_hover_text("bwrap isolates namespaces. Landlock restricts file access without root or user namespaces, but does not hide host processes or file metadata. See the sandbox access rules for its limits.");
    });
}

fn diff_view(ui: &mut egui::Ui, text: &str, palette: theme::Palette) {
    let mut end = text.len().min(1024 * 1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end < text.len() {
        ui.label("Preview limited to 1 MiB. Use the Git terminal for the complete output.");
    }
    let mut preview = &text[..end];
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _wrap: f32| {
        let mut job = egui::text::LayoutJob::default();
        for line in buffer.as_str().split_inclusive('\n') {
            let color = if line.starts_with('+') {
                palette.success
            } else if line.starts_with('-') {
                palette.error
            } else if line.starts_with("@@") || line.starts_with("diff ") {
                palette.accent
            } else {
                palette.text
            };
            job.append(
                line,
                0.,
                egui::TextFormat {
                    font_id: FontId::monospace(12.),
                    color,
                    ..Default::default()
                },
            );
        }
        job.wrap.max_width = f32::INFINITY;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    egui::ScrollArea::both()
        .id_salt(ui.id().with("diff-scroll"))
        .max_height(ui.available_height().max(220.))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut preview)
                    .code_editor()
                    .font(FontId::monospace(12.))
                    .desired_width(ui.available_width())
                    .desired_rows(16)
                    .layouter(&mut layouter),
            );
        });
}

#[cfg(test)]
mod tests;

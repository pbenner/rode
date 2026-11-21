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
mod python;
mod sandbox_access;
mod settings;
use super::*;
use crate::project::{self, Diagnostic};
pub(super) use sandbox_access::{options_ui as sandbox_options_ui, rules_ui as sandbox_rules_ui};
use serde::{Deserialize, Serialize};
pub(super) use settings::SettingsSection;
use std::collections::BTreeMap;

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
    /// Legacy project values are imported once into application-wide monitoring preferences.
    monitor_interval: u64,
    /// Legacy chart choice retained for migration from project-scoped monitoring.
    monitor_chart: monitoring::ChartView,
    /// Requested grants; only the prepared workspace plan establishes actual isolation.
    sandbox: sandbox::Options,
    /// User-operated Git defaults to normal host permissions.
    git_sandbox: bool,
    /// Default for explicitly opened terminals; existing shells retain their policy.
    terminal_sandbox: bool,
    slurm_queue: String,
    slurm_cancel: String,
    slurm_allocation: crate::slurm::Allocation,
    pub(super) marp: crate::marp::Settings,
    pub(super) latex: crate::latex::Settings,
    pub(super) debugger: crate::debugger::Settings,
}

#[cfg(test)]
mod execution_preferences_tests {
    use super::*;

    #[test]
    fn existing_projects_default_to_host_git_and_project_terminals() {
        let old: Preferences = serde_json::from_str("{}").unwrap();
        assert!(!old.git_sandbox);
        assert!(old.terminal_sandbox);
        let chosen = Preferences {
            git_sandbox: true,
            terminal_sandbox: false,
            ..old
        };
        let restored: Preferences =
            serde_json::from_str(&serde_json::to_string(&chosen).unwrap()).unwrap();
        assert!(restored.git_sandbox);
        assert!(!restored.terminal_sandbox);
    }
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
            monitor_chart: Default::default(),
            sandbox: Default::default(),
            git_sandbox: false,
            terminal_sandbox: true,
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
/// Project-owned tool caches with a persisted settings map and a separate editable draft.
/// Runtime epochs invalidate pending format/check results without rewriting source;
/// application navigation and tool-window placement have their own lifetime.
pub(super) struct Workbench {
    preferences: BTreeMap<String, Preferences>,
    pub preferences_pending: bool,
    key: String,
    /// Applied settings used when capturing a new tool invocation.
    prefs: Preferences,
    /// Editable form state; invalid or unapplied edits must not affect running jobs.
    draft: Preferences,
    /// Selected settings category; changing it must not replace unsaved drafts.
    settings_section: SettingsSection,
    /// Tool-setting generation paired with document versions on analysis requests.
    epoch: u64,
    python: python::State,
    environment: environment::State,
    code_tools: code_tools::State,
    git: git::State,
}

impl Workbench {
    /// Return the applied sandbox preference for new Git operations.
    pub(super) fn git_sandbox(&self) -> bool {
        self.prefs.git_sandbox
    }

    /// Return the applied sandbox preference for new terminal sessions.
    pub(super) fn terminal_sandbox(&self) -> bool {
        self.prefs.terminal_sandbox
    }

    /// Apply the terminal sandbox choice to both preference copies and queue its storage update.
    pub(super) fn save_terminal_sandbox(&mut self, sandboxed: bool) {
        self.prefs.terminal_sandbox = sandboxed;
        self.draft.terminal_sandbox = sandboxed;
        self.save_preferences();
    }

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
    /// Discard project runtime caches while retaining saved preferences and pending writes.
    /// Tool-window placement has a separate application owner and is untouched.
    pub(super) fn reset_project(&mut self) {
        let preferences = std::mem::take(&mut self.preferences);
        let pending = self.preferences_pending;
        *self = Self {
            preferences,
            preferences_pending: pending,
            ..Default::default()
        };
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
    /// Import the selected project's old monitor preference once. Subsequent
    /// project switches cannot replace application-wide monitor configuration.
    pub(super) fn migrate_monitor_preferences(&mut self) {
        if !self.areas.monitor_configured {
            self.areas.prefs.monitor_interval = self.workbench.prefs.monitor_interval.clamp(1, 30);
            self.areas.prefs.monitor_chart = self.workbench.prefs.monitor_chart;
            self.areas.monitor_configured = true;
            self.areas.pending = true;
        }
    }

    /// Report a source-control confirmation that must participate in global input guards.
    pub(super) fn workbench_modal(&self) -> bool {
        self.workbench.git.confirm.is_some()
    }

    /// Use the same domain refresh on selection and detachment. A settings draft
    /// remains project-owned; moving its view never copies or reapplies it.
    pub(in crate::app) fn refresh_project_panel(&mut self, panel: Panel, ctx: &egui::Context) {
        match panel {
            Panel::Git => self.refresh_git(ctx),
            Panel::Tasks | Panel::Settings
                if panel == Panel::Tasks
                    || self.workbench.settings_section == SettingsSection::Environment =>
            {
                self.refresh_environment(false, ctx);
                self.refresh_runtimes(ctx);
            }
            _ => {}
        }
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_owned()).collect()
}

/// Compact shared selector for project opening, SSH profiles and saved access rules.
pub(super) fn sandbox_backend_ui(ui: &mut egui::Ui, options: &mut sandbox::Options) {
    crate::keyboard::checkbox(ui, &mut options.enabled, "Sandbox project when available");
    ui.add_enabled_ui(options.enabled, |ui| {
        crate::keyboard::ComboBox::new(ui.id().with("sandbox-backend"), "Sandbox backend")
            .selected_text(options.backend.label())
            .show_ui(ui, |ui| {
                crate::keyboard::selectable_value(ui, &mut options.backend, sandbox::Backend::Bwrap, "bwrap");
                crate::keyboard::selectable_value(ui, &mut options.backend, sandbox::Backend::Landlock, "Landlock");
            }).response.on_hover_text("bwrap isolates namespaces. Landlock restricts file access without root or user namespaces, but does not hide host processes or file metadata. See the sandbox access rules for its limits.");
    });
}

fn diff_view(ui: &mut egui::Ui, text: &str, palette: theme::Palette) {
    super::diff_view::show(ui, text, palette, ui.available_height().max(220.));
}

#[cfg(test)]
mod tests;

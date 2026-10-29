//! Project-tool state, captured preferences and background-result routing.
//!
//! Each domain submodule renders its own panel. Jobs capture project and tool versions before
//! dispatch; stale formatting/diagnostics cannot replace a newer buffer. Shared preferences
//! contain settings only, while queues, telemetry histories and transcripts remain
//! window-owned.

mod code_tools;
mod git;
mod monitoring;
mod python;
mod settings;
use super::*;
use crate::project::{self, Diagnostic};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Panel {
    Git,
    Monitor,
    Problems,
    Python,
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
            Self::Python => "PYTHON ENVIRONMENT",
            Self::Settings => "PROJECT SETTINGS",
            Self::Agent => "AGENT CHAT",
            Self::Debug => "DEBUGGER",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    python: String,
    ruff: String,
    format_on_save: bool,
    check_on_save: bool,
    live_checks: bool,
    monitor_interval: u64,
    sandbox: sandbox::Options,
    slurm_queue: String,
    slurm_cancel: String,
    pub(super) marp: crate::marp::Settings,
    pub(super) latex: crate::latex::Settings,
    pub(super) debugger: crate::debugger::Settings,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            python: "auto".into(),
            ruff: String::new(),
            format_on_save: true,
            check_on_save: true,
            live_checks: true,
            monitor_interval: 2,
            sandbox: Default::default(),
            slurm_queue: "squeue".into(),
            slurm_cancel: "scancel".into(),
            marp: Default::default(),
            latex: Default::default(),
            debugger: Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
}

#[derive(Clone, Debug)]
pub(super) enum GitJob {
    Root,
    Status,
    Tab,
    Diff { path: String, staged: bool },
    Show,
    Mutate { label: String, clear_message: bool },
}
#[derive(Clone, Debug)]
pub(super) enum ToolJob {
    Extension {
        revision: u64,
        operation: String,
    },
    Monitor,
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
struct Issues {
    version: u64,
    items: Vec<Diagnostic>,
}
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
    preferences: BTreeMap<String, Preferences>,
    pub preferences_pending: bool,
    key: String,
    prefs: Preferences,
    draft: Preferences,
    epoch: u64,
    python: python::State,
    code_tools: code_tools::State,
    monitoring: monitoring::State,
    git: git::State,
}

impl Workbench {
    pub(super) fn debugger_settings(&self) -> &crate::debugger::Settings {
        &self.prefs.debugger
    }
    pub(super) fn edit_debugger_settings(&mut self) -> &mut crate::debugger::Settings {
        &mut self.prefs.debugger
    }
    pub(super) fn commit_debugger_settings(&mut self) -> anyhow::Result<()> {
        if let Err(error) = self.prefs.debugger.validate() {
            self.prefs.debugger = self.draft.debugger.clone();
            return Err(error);
        }
        self.draft.debugger = self.prefs.debugger.clone();
        self.save_preferences();
        Ok(())
    }
    pub(super) fn save_debugger_settings(&mut self, settings: crate::debugger::Settings) {
        self.prefs.debugger = settings.clone();
        self.draft.debugger = settings;
        self.save_preferences();
    }

    pub(super) fn latex_settings(&self) -> &crate::latex::Settings {
        &self.prefs.latex
    }
    pub(super) fn set_build_issues(&mut self, id: u64, version: u64, items: Vec<Diagnostic>) {
        self.code_tools.issues.insert(id, Issues { version, items });
    }
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
    pub(super) fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        Self {
            preferences: storage
                .and_then(|s| s.get_string("project_preferences"))
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            ..Default::default()
        }
    }
    pub(super) fn store(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(text) = serde_json::to_string(&self.preferences) {
            storage.set_string("project_preferences", text);
        }
    }
    pub(super) fn reset_project(&mut self) {
        let preferences = std::mem::take(&mut self.preferences);
        let pending = self.preferences_pending;
        let panel = self.panel;
        *self = Self {
            preferences,
            preferences_pending: pending,
            panel,
            ..Default::default()
        };
    }
    pub(super) fn reset_monitor(&mut self) {
        self.monitoring = Default::default();
    }
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
    pub(super) fn workbench_modal(&self) -> bool {
        self.workbench.git.confirm.is_some()
    }

    pub(super) fn current_issue_count(&self) -> usize {
        self.documents
            .iter()
            .map(|d| {
                self.workbench
                    .code_tools
                    .issues
                    .get(&d.id)
                    .filter(|i| i.version == d.edit_version)
                    .map_or(0, |i| i.items.len())
            })
            .sum()
    }

    pub(super) fn workbench_panel(&mut self, ctx: &egui::Context) {
        if !self.navigation.visible(navigation::Pane::Tools) {
            return;
        }
        let Some(panel) = self.workbench.panel else {
            return;
        };
        if !self.panel_enabled(panel) {
            self.workbench.panel = None;
            return;
        }
        let palette = self.themes.active().palette;
        let maximum = (ctx.available_rect().width() - 320.).clamp(280., 680.);
        let zoom = self.navigation.zoom == Some(navigation::Pane::Tools);
        let sidebar = egui::SidePanel::right(if zoom {
            "workbench-zoom"
        } else {
            "workbench-panel"
        })
        .default_width(390.)
        .width_range(280.0..=maximum)
        .frame(theme::frame(palette.panel));
        let sidebar = if zoom {
            sidebar.exact_width(ctx.available_rect().width())
        } else if let Some(width) = self.navigation.widths[1].take() {
            sidebar.exact_width(width.clamp(280., maximum))
        } else {
            sidebar
        };
        sidebar.show(ctx, |ui| {
            self.pane_ui(ui, navigation::Pane::Tools);
            ui.horizontal(|ui| {
                ui.label(palette.tag(panel.title()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icons::small(ui, Icon::Close, "Close project tools").clicked() {
                        self.workbench.panel = None;
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.;
                ui.spacing_mut().button_padding.x = 4.;
                let width = (ui.available_width() - 18.) / 7.;
                for (kind, icon) in [
                    (Panel::Git, Icon::Git),
                    (Panel::Monitor, Icon::Monitor),
                    (Panel::Problems, Icon::Problems),
                    (Panel::Python, Icon::Python),
                    (Panel::Settings, Icon::Settings),
                    (Panel::Agent, Icon::Chat),
                    (Panel::Debug, Icon::Debug),
                ] {
                    if !self.panel_enabled(kind) {
                        continue;
                    }
                    if ui
                        .add_sized(
                            [width, 26.],
                            IconButton::icon(icon, kind.title())
                                .icon_size(18.)
                                .selected(panel == kind),
                        )
                        .clicked()
                    {
                        self.workbench.panel = Some(kind);
                        if kind == Panel::Python {
                            self.refresh_environment(false, ctx);
                        }
                        if kind == Panel::Git {
                            self.refresh_git(ctx);
                        }
                    }
                }
            });
            ui.separator();
            if panel == Panel::Debug {
                self.debugger_ui(ui, ctx);
                return;
            }
            if panel == Panel::Agent {
                self.agent_ui(ui, ctx);
                return;
            }
            let host = self
                .workspace
                .as_ref()
                .and_then(|w| w.remote.as_ref())
                .map(|r| format!("SSH · {}", r.profile.endpoint()))
                .unwrap_or_else(|| "LOCAL HOST".into());
            ui.add(egui::Label::new(palette.tag(&host)).truncate())
                .on_hover_text(&host);
            if panel == Panel::Monitor && !self.monitor_ready() {
                ui.label("Connect or reconnect to the SSH host to view its system monitor.");
                return;
            }
            if panel != Panel::Monitor && !self.project_ready() {
                ui.label("Open a project and connect to its host to use these tools.");
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt(("workbench-content", panel.title()))
                .auto_shrink([false, false])
                .show(ui, |ui| match panel {
                    Panel::Git => self.git_ui(ui, ctx),
                    Panel::Monitor => self.monitor_ui(ui),
                    Panel::Problems => self.problems_ui(ui, ctx),
                    Panel::Python => self.python_ui(ui, ctx),
                    Panel::Settings => self.settings_ui(ui, ctx),
                    Panel::Agent | Panel::Debug => unreachable!(),
                });
        });
    }

    pub(super) fn toggle_panel(&mut self, panel: Panel, ctx: &egui::Context) {
        if !self.panel_enabled(panel) {
            self.notify(
                "This extension is disabled. Enable it in View → Extensions.",
                false,
            );
            return;
        }
        self.workbench.panel = if self.workbench.panel == Some(panel) {
            None
        } else {
            Some(panel)
        };
        if panel == Panel::Git && self.workbench.panel.is_some() {
            self.refresh_git(ctx);
        }
        if panel == Panel::Python && self.workbench.panel.is_some() {
            self.refresh_environment(false, ctx);
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

    pub(in crate::app) fn dispatch_tool(
        &mut self,
        request: project::Request,
        job: ToolJob,
        ctx: &egui::Context,
    ) {
        // Telemetry needs a host, while project commands need a prepared project.
        let host = matches!(request, project::Request::Monitor { .. });
        if (host && !self.monitor_ready()) || (!host && !self.project_ready()) {
            return;
        }
        let workspace = self.workspace.as_ref();
        // Telemetry describes the host, not the sandbox's private /proc.
        let plan = if host {
            None
        } else {
            workspace.map(|w| w.plan.clone())
        };
        let remote = workspace.and_then(|w| w.remote.clone());
        let generation = self.generation;
        let cancel = self.extensions.cancel.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let request = Request::Tool { request };
            let invocation = if let Request::Tool {
                request: project::Request::Extension { invocation, .. },
            } = &request
            {
                Some(invocation.clone())
            } else {
                None
            };
            let result = if let Request::Tool {
                request: request @ project::Request::Extension { .. },
            } = &request
            {
                let reply = match &remote {
                    Some(remote) => remote.extension_request(request, plan.as_ref(), &cancel),
                    None => plan
                        .clone()
                        .unwrap_or_default()
                        .extension_request(request, &cancel),
                };
                reply.map(|reply| files::Response {
                    tool: Some(project::Response::Extension(reply)),
                    ..Default::default()
                })
            } else {
                match remote {
                    Some(remote) => remote.request_in(&request, plan.as_ref()),
                    None => match plan {
                        Some(plan) => plan.local_request(&request),
                        None => files::local(&request),
                    },
                }
            }
            .and_then(|r| {
                r.tool
                    .ok_or_else(|| anyhow::anyhow!("Host returned no project service result"))
            })
            .and_then(|response| {
                if let Some(invocation) = &invocation {
                    let project::Response::Extension(reply) = &response else {
                        anyhow::bail!("Missing adapter response");
                    };
                    reply.validate(&invocation.operation)?;
                    if matches!(&job, ToolJob::Analyze { .. }) {
                        return Ok(project::Response::Analysis(project::Analysis {
                            text: reply
                                .text
                                .clone()
                                .unwrap_or_else(|| invocation.text.clone()),
                            diagnostics: reply.diagnostics.clone(),
                            warnings: if reply.output.is_empty() {
                                vec![]
                            } else {
                                vec![reply.output.clone()]
                            },
                        }));
                    }
                }
                Ok(response)
            })
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Tool {
                generation,
                job: Box::new(job),
                result: Box::new(result),
            });
            ctx.request_repaint();
        });
    }
    pub(in crate::app) fn project_ready(&self) -> bool {
        !self.vaults.locking
            && self
                .workspace
                .as_ref()
                .is_some_and(|w| w.ready && w.connected)
    }
    fn monitor_ready(&self) -> bool {
        !self.vaults.locking
            && self
                .workspace
                .as_ref()
                .is_none_or(|w| w.remote.is_none() || w.connected)
    }
    pub(in crate::app) fn root(&self) -> String {
        self.workspace
            .as_ref()
            .map(|w| w.root.clone())
            .unwrap_or_default()
    }

    pub(super) fn workbench_tick(&mut self, ctx: &egui::Context) {
        if self.feature(crate::extensions::Feature::Monitor)
            && self.navigation.visible(navigation::Pane::Tools)
            && self.workbench.panel == Some(Panel::Monitor)
            && self.monitor_ready()
        {
            let interval = Duration::from_secs(self.workbench.prefs.monitor_interval.clamp(1, 30));
            if !self.workbench.monitoring.monitor_busy
                && self
                    .workbench
                    .monitoring
                    .monitor_last
                    .is_none_or(|t| t.elapsed() >= interval)
            {
                self.workbench.monitoring.monitor_busy = true;
                self.dispatch_tool(
                    project::Request::Monitor {
                        root: if self.project_ready() {
                            self.root()
                        } else {
                            "/".into()
                        },
                    },
                    ToolJob::Monitor,
                    ctx,
                );
            }
            ctx.request_repaint_after(interval);
        }
        if !self.project_ready() {
            return;
        }
        if self.feature(crate::extensions::Feature::Git)
            && self.navigation.visible(navigation::Pane::Tools)
            && self.workbench.panel == Some(Panel::Git)
            && self.workbench.git.git_tab == GitTab::Changes
        {
            if !self.workbench.git.git_busy
                && self
                    .workbench
                    .git
                    .git_last
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
            {
                self.refresh_git(ctx);
            }
            ctx.request_repaint_after(Duration::from_secs(5));
        }
        if self.workbench.prefs.live_checks
            && let Some(doc) = self.documents.get(self.active)
            && let Some(path) = doc.path.as_deref()
        {
            let language = crate::languages::detect(Some(path));
            let external = self
                .extensions
                .registry
                .provider(crate::app::language(Some(path)), false, true)
                .is_some();
            if doc.is_preview()
                || doc.text.len() > 512 * 1024
                || (!external
                    && (!self.feature(crate::extensions::Feature::Languages)
                        || language.is_none_or(|l| {
                            !l.check
                                || (l.id == "latex"
                                    && !self.feature(crate::extensions::Feature::Latex))
                                || l.check_tool().is_none_or(|tool| {
                                    self.workbench.code_tools.unavailable_tools.contains(tool)
                                })
                        })))
            {
                return;
            }
            let observed = &mut self.workbench.code_tools.observed;
            if observed
                .as_ref()
                .is_none_or(|(id, version, _)| *id != doc.id || *version != doc.edit_version)
            {
                *observed = Some((doc.id, doc.edit_version, Instant::now()));
            }
            let (_, _, since) = observed.as_ref().unwrap();
            if self.workbench.code_tools.checked.get(&doc.id)
                != Some(&(doc.edit_version, self.workbench.epoch))
                && !self.workbench.code_tools.checking.contains(&doc.id)
                && !doc.saving
            {
                if since.elapsed() >= Duration::from_millis(900) {
                    self.analyze_document(doc.id, path.to_owned(), None, false, false, false, ctx);
                } else {
                    ctx.request_repaint_after(
                        Duration::from_millis(900).saturating_sub(since.elapsed()),
                    );
                }
            }
        }
    }
    pub(super) fn handle_tool(
        &mut self,
        job: ToolJob,
        result: Result<project::Response, String>,
        ctx: &egui::Context,
    ) {
        match job {
            ToolJob::Extension {
                revision,
                operation,
            } => {
                self.extensions.busy = false;
                if revision != self.extensions.registry.revision {
                    return;
                }
                match result {
                    Ok(project::Response::Extension(reply)) => match reply.validate(&operation) {
                        Ok(()) => {
                            self.extensions.output = reply.output;
                            self.extensions.output_open = true;
                        }
                        Err(error) => self.notify(format!("{error:#}"), true),
                    },
                    Err(error) => self.notify(error, true),
                    _ => self.notify("Invalid extension response.", true),
                }
            }
            ToolJob::Monitor => {
                if !self.feature(crate::extensions::Feature::Monitor) {
                    return;
                }
                self.workbench.monitoring.monitor_busy = false;
                self.workbench.monitoring.monitor_error.clear();
                match result {
                    Ok(project::Response::Monitor(sample)) => {
                        let previous = self.workbench.monitoring.monitor.as_ref();
                        self.workbench.monitoring.cpu_usage = sample
                            .cpus
                            .iter()
                            .map(|cpu| {
                                previous
                                    .and_then(|p| p.cpus.iter().find(|c| c.name == cpu.name))
                                    .map(|old| {
                                        let total = cpu.total.saturating_sub(old.total);
                                        let idle = cpu.idle.saturating_sub(old.idle);
                                        if total == 0 {
                                            0.
                                        } else {
                                            100. * total.saturating_sub(idle) as f32 / total as f32
                                        }
                                    })
                                    .unwrap_or(0.)
                            })
                            .collect();
                        if let (Some(old), Some(last)) =
                            (previous, self.workbench.monitoring.monitor_last)
                        {
                            let seconds = last.elapsed().as_secs_f32().max(0.01);
                            self.workbench.monitoring.network_rate = (
                                sample.net_received.saturating_sub(old.net_received) as f32
                                    / seconds,
                                sample.net_sent.saturating_sub(old.net_sent) as f32 / seconds,
                            );
                        }
                        if previous.is_some() {
                            history(
                                &mut self.workbench.monitoring.cpu_history,
                                self.workbench
                                    .monitoring
                                    .cpu_usage
                                    .first()
                                    .copied()
                                    .unwrap_or(0.),
                            );
                        }
                        let used = 100.
                            * (1.
                                - sample.memory_available as f32
                                    / sample.memory_total.max(1) as f32);
                        history(&mut self.workbench.monitoring.memory_history, used);
                        for gpu in &sample.gpus {
                            if let Some(usage) = gpu.usage {
                                history(
                                    self.workbench
                                        .monitoring
                                        .gpu_history
                                        .entry(gpu.name.clone())
                                        .or_default(),
                                    usage,
                                );
                            }
                        }
                        self.workbench.monitoring.monitor = Some(sample);
                    }
                    Err(error) => self.workbench.monitoring.monitor_error = error,
                    _ => {
                        self.workbench.monitoring.monitor_error =
                            "Invalid host monitor response".into()
                    }
                }
                self.workbench.monitoring.monitor_last = Some(Instant::now());
            }
            ToolJob::Environment { activate, selected } => {
                if !self.feature(crate::extensions::Feature::Python) {
                    self.workbench.python.env_busy = false;
                    return;
                }
                self.workbench.python.env_busy = false;
                if selected != self.workbench.prefs.python {
                    self.refresh_environment(activate, ctx);
                    return;
                }
                match result {
                    Ok(project::Response::Environments(environment)) => {
                        self.workbench.python.env = Some(environment);
                        self.workbench.python.env_error.clear();
                        if activate {
                            self.status = "Python interpreter selected · new shells use it".into();
                        }
                    }
                    Err(error) => self.workbench.python.env_error = error,
                    _ => {
                        self.workbench.python.env_error =
                            "Invalid Python environment response".into()
                    }
                }
            }
            ToolJob::CreateEnvironment { path } => {
                if !self.feature(crate::extensions::Feature::Python) {
                    self.workbench.python.venv_busy = false;
                    return;
                }
                self.workbench.python.venv_busy = false;
                match result {
                    Ok(project::Response::Command(output)) if output.success() => {
                        self.workbench.prefs.python = path;
                        self.workbench.draft.python = self.workbench.prefs.python.clone();
                        self.workbench.epoch += 1;
                        self.workbench.code_tools.tool_error.clear();
                        self.workbench.code_tools.unavailable_tools.clear();
                        self.workbench.save_preferences();
                        self.refresh_environment(true, ctx);
                        self.status = "Python environment created and selected".into();
                    }
                    Ok(project::Response::Command(output)) => {
                        self.workbench.python.env_error = output.stderr + &output.stdout
                    }
                    Err(error) => self.workbench.python.env_error = error,
                    _ => {}
                }
            }
            ToolJob::Analyze {
                tool,
                id,
                version,
                epoch,
                original,
                path,
                save,
                manual,
            } => {
                self.workbench.code_tools.checking.remove(&id);
                self.workbench
                    .code_tools
                    .checked
                    .insert(id, (version, epoch));
                let mut text_to_save = original;
                let fresh = self
                    .documents
                    .iter()
                    .any(|d| d.id == id && d.edit_version == version)
                    && epoch == self.workbench.epoch;
                match result {
                    Ok(project::Response::Analysis(analysis)) if fresh => {
                        if let Some(doc) = self.documents.iter_mut().find(|d| d.id == id) {
                            if doc.text != analysis.text {
                                doc.text = analysis.text.clone();
                                doc.edit_version = doc.edit_version.wrapping_add(1);
                                doc.modified = doc.path.is_none() || doc.text != doc.saved_text;
                                doc.lines = doc.text.bytes().filter(|b| *b == b'\n').count() + 1;
                            }
                            self.workbench.code_tools.issues.insert(
                                id,
                                Issues {
                                    version: doc.edit_version,
                                    items: analysis.diagnostics,
                                },
                            );
                            self.workbench
                                .code_tools
                                .checked
                                .insert(id, (doc.edit_version, epoch));
                        }
                        text_to_save = analysis.text;
                        self.workbench.code_tools.tool_error.clear();
                        self.workbench.code_tools.unavailable_tools.remove(&tool);
                        if !analysis.warnings.is_empty() && (manual || save.is_some()) {
                            self.notify(
                                format!("Code tools: {}", analysis.warnings.join("\n")),
                                true,
                            );
                        }
                        if manual {
                            self.status = "Code tools completed".into();
                        }
                    }
                    Ok(_) => {
                        if manual {
                            self.notify("New edits arrived while the tool ran; its outdated result was skipped.", false);
                        }
                    }
                    Err(error) => {
                        self.workbench.code_tools.tool_error = error.clone();
                        if error.contains("is unavailable") || error.contains("Cannot run") {
                            self.workbench
                                .code_tools
                                .unavailable_tools
                                .insert(tool.clone());
                        }
                        if manual || save.is_some() {
                            self.notify(
                                format!(
                                    "Code tool: {error}{}",
                                    if save.is_some() {
                                        " — saving the submitted buffer without formatting."
                                    } else {
                                        ""
                                    }
                                ),
                                true,
                            );
                        }
                    }
                }
                if let Some(expected) = save {
                    self.dispatch(
                        Request::Write {
                            path,
                            text: text_to_save.clone(),
                            expected,
                        },
                        Job::Save {
                            id,
                            text: text_to_save,
                        },
                        ctx,
                    );
                }
            }
            ToolJob::Git(job) => {
                if self.feature(crate::extensions::Feature::Git) {
                    self.handle_git(job, result, ctx);
                } else {
                    self.workbench.git.git_busy = false;
                }
            }
        }
    }

    pub(super) fn reload_clean_documents(&mut self, ctx: &egui::Context) {
        let jobs: Vec<_> = self
            .documents
            .iter()
            .filter(|d| !d.dirty() && !d.saving && !d.preview_loading())
            .filter_map(|d| d.path.clone().map(|p| (p, d.id, d.edit_version)))
            .collect();
        for (path, id, version) in jobs {
            if self
                .documents
                .iter()
                .any(|d| d.id == id && d.notebook.is_some())
            {
                self.dispatch(Request::ReadAuto { path }, Job::Reload { id, version }, ctx);
            } else if self.documents.iter().any(|d| d.id == id && d.is_preview()) {
                if self
                    .documents
                    .iter()
                    .any(|doc| doc.id == id && doc.pdf.is_some())
                {
                    self.reload_pdf(id, ctx);
                } else if self
                    .documents
                    .iter()
                    .any(|doc| doc.id == id && doc.hex.is_some())
                {
                    self.dispatch(Request::ReadHex { path }, Job::Reload { id, version }, ctx);
                } else {
                    self.reload_image(id, ctx);
                }
            } else {
                self.dispatch(Request::Read { path }, Job::Reload { id, version }, ctx);
            }
        }
        if let Some(root) = self.workspace.as_ref().map(|w| w.root.clone()) {
            self.list(root, ctx);
        }
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_owned()).collect()
}
fn history(history: &mut VecDeque<f32>, value: f32) {
    history.push_back(value);
    if history.len() > 120 {
        history.pop_front();
    }
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

fn metric(
    ui: &mut egui::Ui,
    name: &str,
    value: f32,
    detail: String,
    data: &VecDeque<f32>,
    palette: theme::Palette,
) {
    ui.add_space(9.);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(name)
                .monospace()
                .strong()
                .color(palette.accent),
        );
        ui.label(RichText::new(detail).monospace().small());
    });
    ui.add(
        egui::ProgressBar::new((value / 100.).clamp(0., 1.))
            .fill(palette.accent)
            .desired_height(6.),
    );
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 42.), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2., palette.background);
    for i in 1..4 {
        let y = rect.top() + rect.height() * i as f32 / 4.;
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(0.5_f32, palette.border),
        );
    }
    if data.len() > 1 {
        let points: Vec<_> = data
            .iter()
            .enumerate()
            .map(|(index, value)| {
                egui::pos2(
                    rect.left() + rect.width() * index as f32 / (data.len() - 1) as f32,
                    rect.bottom() - rect.height() * value.clamp(0., 100.) / 100.,
                )
            })
            .collect();
        painter.add(egui::Shape::line(
            points,
            Stroke::new(1.5_f32, palette.accent),
        ));
    }
}

pub(super) fn sandbox_options_ui(ui: &mut egui::Ui, options: &mut sandbox::Options) {
    ui.checkbox(
        &mut options.enabled,
        "Sandbox project when bwrap is available",
    );
    ui.add_enabled_ui(options.enabled,|ui| {
        ui.checkbox(&mut options.share_network,"Allow network access");
        ui.checkbox(&mut options.project_read_only,"Project is read only");
        ui.label("Built-in shared paths");
        egui::Grid::new(ui.id().with("sandbox-access")).num_columns(2).striped(true).show(ui,|ui| {
            for (path,default) in sandbox::default_mounts() {
                ui.monospace(&path);
                let mut access = options.overrides.get(&path).copied().unwrap_or(default);
                egui::ComboBox::from_id_salt(("access",&path)).selected_text(access.label()).show_ui(ui,|ui| {
                    for value in [sandbox::Access::Hidden,sandbox::Access::ReadOnly,sandbox::Access::ReadWrite] { ui.selectable_value(&mut access,value,value.label()); }
                });
                if access == default { options.overrides.remove(&path); } else { options.overrides.insert(path,access); }
                ui.end_row();
            }
        });
        ui.add_space(8.); ui.label("Additional shared paths (absolute or ~/)");
        let mut rows: Vec<_> = options.read_only.iter().map(|p|(p.clone(),sandbox::Access::ReadOnly)).chain(options.read_write.iter().map(|p|(p.clone(),sandbox::Access::ReadWrite))).collect();
        let mut remove = None;
        for (index,(path,access)) in rows.iter_mut().enumerate() { ui.push_id(("extra-mount",index),|ui| { ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(path).desired_width((ui.available_width()-155.).max(80.)).hint_text("/scratch/data"));
            egui::ComboBox::from_id_salt("mode").width(90.).selected_text(access.label()).show_ui(ui,|ui| {
                ui.selectable_value(access,sandbox::Access::ReadOnly,"Read only"); ui.selectable_value(access,sandbox::Access::ReadWrite,"Read + write");
            });
            if icons::small(ui, Icon::Close, "Remove path grant").clicked() { remove = Some(index); }
        }); }); }
        if let Some(index) = remove { rows.remove(index); }
        if ui.button("+ Add folder or file").clicked() { rows.push((String::new(),sandbox::Access::ReadOnly)); }
        options.read_only = rows.iter().filter(|(_,a)|*a == sandbox::Access::ReadOnly).map(|(p,_)|p.clone()).collect();
        options.read_write = rows.into_iter().filter(|(_,a)|*a == sandbox::Access::ReadWrite).map(|(p,_)|p).collect();
        ui.label(RichText::new("Private /tmp and hidden home remain enforced. Unavailable bwrap is visibly normal mode. Shared writable caches retain their contents and access.").small());
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
mod tests {
    use super::*;

    fn panel_text(app: &mut EditorApp, ctx: &egui::Context) -> String {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1380., 900.),
                )),
                ..Default::default()
            },
            |ctx| app.workbench_panel(ctx),
        )
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
    }

    fn app(root: &std::path::Path) -> (EditorApp, egui::Context) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.workspace = Some(Workspace {
            root: root.to_str().unwrap().into(),
            remote: None,
            connected: true,
            ready: true,
            plan: sandbox::Plan {
                root: root.to_str().unwrap().into(),
                ..Default::default()
            },
            sandbox_options: Default::default(),
        });
        (app, ctx)
    }
    #[test]
    fn opening_a_non_python_project_does_not_probe_python_and_disable_invalidates_tools() {
        use crate::extensions::Feature;
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx) = app(root.path());
        app.load_project_tools(&ctx);
        assert!(!app.workbench.python.env_busy);
        app.new_file();
        app.documents[0].text = "keep dirty".into();
        let cancel = app.extensions.cancel.clone();
        let epoch = app.workbench.epoch;
        app.set_extension(Feature::Monitor.id(), false);
        assert!(cancel.load(Ordering::Acquire));
        assert!(app.workbench.epoch > epoch);
        app.toggle_panel(Panel::Monitor, &ctx);
        assert_ne!(app.workbench.panel, Some(Panel::Monitor));
        assert_eq!(app.documents[0].text, "keep dirty");
        assert!(app.documents[0].dirty());
        app.set_extension(Feature::Python.id(), false);
        app.refresh_environment(false, &ctx);
        assert!(!app.workbench.python.env_busy);
        assert_eq!(app.terminal_python_selection(), "off");
    }
    #[test]
    fn monitor_samples_without_a_project_and_stops_scheduling_when_hidden() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        assert!(app.workspace.is_none());
        assert!(app.monitor_ready());
        assert!(!app.project_ready());
        app.workbench_tick(&ctx);
        assert!(!app.workbench.monitoring.monitor_busy);
        app.toggle_panel(Panel::Monitor, &ctx);
        app.workbench_tick(&ctx);
        assert!(app.workbench.monitoring.monitor_busy);
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.workbench.monitoring.monitor_busy && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            app.poll(&ctx);
        }
        assert!(
            !app.workbench.monitoring.monitor_busy,
            "Host sampling did not complete"
        );
        assert!(
            app.workbench.monitoring.monitor_error.is_empty(),
            "{}",
            app.workbench.monitoring.monitor_error
        );
        let sample = app.workbench.monitoring.monitor.as_ref().unwrap();
        assert!(!sample.cpus.is_empty());
        assert!(sample.memory_total > 0);
        assert!(sample.disk_total > 0);
        assert!(app.workspace.is_none());
        assert!(app.terminal.is_none());
        assert!(app.workbench.preferences.is_empty());
        let labels = panel_text(&mut app, &ctx);
        assert!(labels.contains("SYS // LIVE"), "{labels}");
        assert!(labels.contains("LOCAL HOST"), "{labels}");
        assert!(!labels.contains("Open a project"), "{labels}");

        // Make another sample due, then close the panel before scheduling it.
        let due = Instant::now() - Duration::from_secs(31);
        app.workbench.monitoring.monitor_last = Some(due);
        app.navigation.zoom = Some(navigation::Pane::Editor);
        app.workbench_tick(&ctx);
        assert!(!app.workbench.monitoring.monitor_busy);
        assert_eq!(app.workbench.monitoring.monitor_last, Some(due));
        app.navigation.zoom = None;
        app.toggle_panel(Panel::Monitor, &ctx);
        app.workbench_tick(&ctx);
        assert!(!app.workbench.monitoring.monitor_busy);
        assert_eq!(app.workbench.monitoring.monitor_last, Some(due));
    }
    #[test]
    fn remote_monitor_waits_for_authentication_and_ignores_stale_host_results() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let remote = Remote::new(Profile {
            host: "monitor.invalid".into(),
            ..Default::default()
        })
        .unwrap();
        app.workspace = Some(Workspace {
            root: "/not-yet-open".into(),
            remote: Some(remote),
            connected: false,
            ready: false,
            plan: Default::default(),
            sandbox_options: Default::default(),
        });
        app.workbench.panel = Some(Panel::Monitor);
        assert!(!app.monitor_ready());
        app.workbench_tick(&ctx);
        assert!(!app.workbench.monitoring.monitor_busy);
        assert!(app.workbench.monitoring.monitor.is_none());

        let labels = panel_text(&mut app, &ctx);
        assert!(labels.contains("Connect or reconnect"), "{labels}");
        assert!(labels.contains("monitor.invalid"), "{labels}");
        assert!(!labels.contains("SYS // LIVE"), "{labels}");

        app.workspace.as_mut().unwrap().connected = true;
        assert!(app.monitor_ready());
        assert!(!app.project_ready());
        app.workbench.monitoring.monitor_busy = true;
        app.workbench.monitoring.monitor = Some(project::Monitor {
            hostname: "old-host".into(),
            ..Default::default()
        });
        app.workbench.monitoring.cpu_history.push_back(42.);
        app.workbench.reset_monitor();
        let old_generation = app.generation;
        app.generation += 1;
        app.tx
            .send(Event::Tool {
                generation: old_generation,
                job: Box::new(ToolJob::Monitor),
                result: Box::new(Ok(project::Response::Monitor(project::Monitor {
                    hostname: "old-host".into(),
                    ..Default::default()
                }))),
            })
            .unwrap();
        app.poll(&ctx);
        assert!(!app.workbench.monitoring.monitor_busy);
        assert!(app.workbench.monitoring.monitor.is_none());
        assert!(app.workbench.monitoring.cpu_history.is_empty());
        assert!(app.workbench.monitoring.monitor_last.is_none());
    }
    #[test]
    fn live_checks_skip_unbound_buffers_even_with_a_plain_text_adapter() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx) = app(root.path());
        let extensions = root.path().join("extensions");
        std::fs::create_dir(&extensions).unwrap();
        std::fs::write(
            extensions.join("text.json"),
            serde_json::json!({
                "api":1,"id":"test.text","name":"Text checker","version":"1.0","description":"Unbound buffer fixture",
                "tool":{"program":"false","languages":["text"],"check":true}
            })
            .to_string(),
        )
        .unwrap();
        app.extensions.registry =
            crate::extensions::Registry::load(&extensions, Default::default());
        app.extensions.registry.set_enabled("test.text", true);
        assert!(
            app.extensions
                .registry
                .provider("text", false, true)
                .is_some()
        );
        app.workbench.prefs.live_checks = true;
        app.new_file();
        app.workbench.code_tools.observed = Some((
            app.documents[0].id,
            0,
            Instant::now() - Duration::from_secs(1),
        ));
        app.workbench_tick(&ctx);
        assert!(app.workbench.code_tools.checking.is_empty());
        assert!(!app.documents[0].saving);
    }
    #[test]
    fn format_save_result_cannot_replace_newer_edits_or_publish_stale_diagnostics() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.py");
        std::fs::write(&path, "value=0\n").unwrap();
        let (mut app, ctx) = app(root.path());
        app.new_file();
        let id = app.documents[0].id;
        let document = &mut app.documents[0];
        document.path = Some(path.to_str().unwrap().into());
        document.saved_text = "value=0\n".into();
        document.text = "value=2\n".into();
        document.modified = true;
        document.edit_version = 2;
        document.saving = true;
        app.handle_tool(
            ToolJob::Analyze {
                tool: "test-tool".into(),
                id,
                version: 1,
                epoch: 0,
                original: "value=1\n".into(),
                path: path.to_str().unwrap().into(),
                save: Some(Some(files::digest(b"value=0\n"))),
                manual: false,
            },
            Ok(project::Response::Analysis(project::Analysis {
                text: "value = 1\n".into(),
                diagnostics: vec![Diagnostic {
                    code: "stale".into(),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            &ctx,
        );
        let started = Instant::now();
        while app.documents[0].saving && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(10));
            app.poll(&ctx);
        }
        assert!(!app.documents[0].saving);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "value=1\n");
        assert_eq!(app.documents[0].text, "value=2\n");
        assert!(app.documents[0].dirty());
        assert!(app.document_issues(&app.documents[0]).is_empty());
    }
    #[test]
    fn fresh_formatter_result_updates_buffer_and_diagnostics_without_saving_it() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx) = app(root.path());
        app.new_file();
        app.documents[0].text = "value=1\n".into();
        let id = app.documents[0].id;
        app.handle_tool(
            ToolJob::Analyze {
                tool: "test-tool".into(),
                id,
                version: 0,
                epoch: 0,
                original: "value=1\n".into(),
                path: "main.py".into(),
                save: None,
                manual: true,
            },
            Ok(project::Response::Analysis(project::Analysis {
                text: "value = 1\n".into(),
                diagnostics: vec![Diagnostic {
                    code: "test".into(),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            &ctx,
        );
        assert_eq!(app.documents[0].text, "value = 1\n");
        assert_eq!(app.documents[0].edit_version, 1);
        assert!(app.documents[0].dirty());
        assert_eq!(app.document_issues(&app.documents[0]).len(), 1);
        assert!(!root.path().join("main.py").exists());
        app.documents[0].edit_version += 1;
        assert!(app.document_issues(&app.documents[0]).is_empty());
    }
    #[test]
    fn applying_a_local_project_policy_keeps_unsaved_buffers() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx) = app(root.path());
        app.new_file();
        app.documents[0].text = "keep unsaved work".into();
        let id = app.documents[0].id;
        app.commit_project(
            files::Response {
                path: root.path().to_str().unwrap().into(),
                project: Some(sandbox::Plan {
                    root: root.path().to_str().unwrap().into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            false,
            Default::default(),
            true,
            &ctx,
        );
        assert_eq!(app.documents[0].id, id);
        assert_eq!(app.documents[0].text, "keep unsaved work");
        assert!(app.documents[0].dirty());
    }
    #[test]
    fn missing_one_formatter_does_not_disable_other_languages() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx) = app(root.path());
        app.new_file();
        let id = app.documents[0].id;
        let job = |path: &str, epoch| ToolJob::Analyze {
            tool: crate::languages::detect(Some(path))
                .unwrap()
                .check_tool()
                .unwrap()
                .into(),
            id,
            version: 0,
            epoch,
            original: String::new(),
            path: path.into(),
            save: None,
            manual: false,
        };
        app.handle_tool(
            job("main.py", app.workbench.epoch),
            Err("Ruff is unavailable".into()),
            &ctx,
        );
        assert!(app.workbench.code_tools.unavailable_tools.contains("ruff"));
        assert!(!app.workbench.code_tools.unavailable_tools.contains("gofmt"));
        app.handle_tool(
            job("main.go", app.workbench.epoch),
            Ok(project::Response::Analysis(Default::default())),
            &ctx,
        );
        assert!(app.workbench.code_tools.unavailable_tools.contains("ruff"));
        app.handle_tool(
            job("main.py", app.workbench.epoch),
            Ok(project::Response::Analysis(Default::default())),
            &ctx,
        );
        assert!(!app.workbench.code_tools.unavailable_tools.contains("ruff"));
    }
    #[derive(Default)]
    struct Storage {
        data: HashMap<String, String>,
        flushes: usize,
    }
    impl eframe::Storage for Storage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.data.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.data.insert(key.into(), value);
        }
        fn flush(&mut self) {
            self.flushes += 1;
        }
    }
    #[test]
    fn project_settings_flush_immediately_and_restore_per_project_values() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, _) = app(root.path());
        app.workbench.key = "ssh:test:/project".into();
        app.workbench.prefs.python = "/project/.venv/bin/python".into();
        app.workbench.prefs.sandbox.project_read_only = true;
        app.workbench
            .prefs
            .sandbox
            .overrides
            .insert("~/.cargo".into(), sandbox::Access::Hidden);
        app.workbench.save_preferences();
        let mut storage = Storage::default();
        app.persist_project_preferences(Some(&mut storage));
        assert_eq!(storage.flushes, 1);
        app.persist_project_preferences(Some(&mut storage));
        assert_eq!(storage.flushes, 1);
        let restored = Workbench::load(Some(&storage));
        let prefs = &restored.preferences["ssh:test:/project"];
        assert_eq!(prefs.python, "/project/.venv/bin/python");
        assert!(prefs.sandbox.project_read_only);
        assert_eq!(prefs.sandbox.overrides["~/.cargo"], sandbox::Access::Hidden);
    }
}

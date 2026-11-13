//! Project-tool dispatch, refresh scheduling and version-checked completion routing.
//!
//! Capture host policy, generation and document/tool epochs before leaving the UI
//! thread. Shared readiness checks separate host telemetry from project commands;
//! accepting a reply never widens its captured policy or replaces newer edits.

use super::*;
use crate::sandbox::PlanExt as _;

impl EditorApp {
    /// Capture host, policy, generation, cancellation and encrypted-mount ownership,
    /// then run a blocking project request off the UI thread. Telemetry and explicitly
    /// host-mode UI Git use separate host execution; other tools retain the project policy.
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
        let host_git = !self.workbench.git_sandbox()
            && matches!(job, ToolJob::Git(_))
            && matches!(
                request,
                project::Request::Git { .. }
                    | project::Request::GitAuthenticated { .. }
                    | project::Request::GitStash { .. }
            );
        let git_host_plan = self.workbench.git.host_plan.clone();
        let cancel = self.extensions.cancel.clone();
        let replies = self.workers.workspace(ctx);
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let invocation = if let project::Request::Extension { invocation, .. } = &request {
                Some(invocation.clone())
            } else {
                None
            };
            let result = (|| -> anyhow::Result<files::Response> {
                let plan = if host_git {
                    let project = plan
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Git requires a prepared project"))?;
                    Some(git_host_plan.prepare(project, remote.as_ref())?)
                } else {
                    plan
                };
                let request = Request::Tool { request };
                if let Request::Tool {
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
            })()
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
            let _ = replies.send(WorkspaceEvent::Tool {
                job: Box::new(job),
                result: Box::new(result),
            });
        });
    }
    /// Require a prepared, connected project and no encrypted-project lock in progress.
    pub(in crate::app) fn project_ready(&self) -> bool {
        !self.vaults.locking
            && self
                .workspace
                .as_ref()
                .is_some_and(|w| w.ready && w.connected)
    }
    /// Allow local host sampling without a project; SSH sampling requires its connection.
    pub(super) fn monitor_ready(&self) -> bool {
        !self.vaults.locking
            && self
                .workspace
                .as_ref()
                .is_none_or(|w| w.remote.is_none() || w.connected)
    }
    /// Copy the selected project root, or an empty string when no workspace exists.
    pub(in crate::app) fn root(&self) -> String {
        self.workspace
            .as_ref()
            .map(|w| w.root.clone())
            .unwrap_or_default()
    }

    /// Schedule visible host/Git refreshes and debounced source diagnostics.
    /// Use cached timestamps and in-flight flags to avoid launching work every frame.
    pub(in crate::app) fn workbench_tick(&mut self, ctx: &egui::Context) {
        if self.feature(crate::extensions::Feature::Monitor)
            && self.panel_visible(Panel::Monitor)
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
            && self.panel_visible(Panel::Git)
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
            && let Some(doc) = self.documents.active()
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
    /// Apply a current-generation tool reply after its domain-specific identity checks;
    /// format/check replies additionally validate the submitted source and tool epoch.
    pub(in crate::app) fn handle_tool(
        &mut self,
        job: ToolJob,
        result: Result<project::Response, String>,
        ctx: &egui::Context,
    ) {
        match job {
            ToolJob::Mise {
                action,
                revision,
                settings,
            } => {
                self.workbench.environment.mise_busy = false;
                if revision != self.extensions.registry.revision
                    || settings != self.workbench.prefs.environment
                {
                    return;
                }
                match result {
                    Ok(project::Response::Mise(snapshot)) => {
                        self.workbench.environment.mise_error.clear();
                        self.workbench.environment.mise = Some(snapshot);
                        if !matches!(action, crate::mise::Action::Inspect) {
                            self.request_mise(crate::mise::Action::Inspect, ctx);
                        }
                    }
                    Err(error) => self.workbench.environment.mise_error = error,
                    _ => self.workbench.environment.mise_error = "Invalid mise response".into(),
                }
            }
            ToolJob::Runtimes { settings } => {
                self.workbench.environment.busy = false;
                if settings != self.workbench.prefs.environment {
                    self.refresh_runtimes(ctx);
                    return;
                }
                match result {
                    Ok(project::Response::Runtimes(runtimes)) => {
                        self.workbench.environment.runtimes = runtimes;
                        self.workbench.environment.error.clear();
                    }
                    Err(error) => self.workbench.environment.error = error,
                    _ => self.workbench.environment.error = "Invalid runtime response".into(),
                }
            }
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
                        self.workbench.monitoring.record(
                            sample,
                            Instant::now(),
                            Duration::from_secs(self.workbench.prefs.monitor_interval.clamp(1, 30)),
                        );
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
                    self.dispatch_text_save(id, path, text_to_save, expected, ctx);
                }
            }
            ToolJob::Git(job) => {
                if self.feature(crate::extensions::Feature::Git) {
                    self.handle_git(job, result, ctx);
                } else {
                    self.workbench.git.git_busy = false;
                    self.workbench.git.committing = false;
                    self.workbench.git.clear_auth();
                }
            }
        }
    }

    /// Reload only bound, clean text documents after project mutations; dirty or
    /// saving buffers retain their local contents and preview tabs keep their own reload paths.
    pub(in crate::app) fn reload_clean_documents(&mut self, ctx: &egui::Context) {
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

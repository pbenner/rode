//! Terminal view and acknowledgement controller with explicit resource inputs.
//! It can update its attachments, cache and queue, but cannot select documents,
//! change workspace authority or manipulate another domain through `EditorApp`.
use super::{
    Attached, Job, Meta, QueuedCommand, Sessions, Terminal, execution, project, sandbox, service,
    slurm,
};
use crate::{app::workers::WorkspaceEvent, environment, files::Request};
use anyhow::Context;
use std::time::Instant;

/// Captured creation inputs; the UI contributes readiness and interpreter preferences.
/// The session owner controls limits, identities and pending acknowledgements.
pub(super) struct Create {
    pub plan: sandbox::Plan,
    pub persistent: bool,
    pub allocation: Option<slurm::Allocation>,
    pub reserve: bool,
    pub ready: bool,
    pub python: String,
    pub environment: environment::Settings,
}

/// UI/lifecycle consequences of an acknowledged terminal or scheduler operation.
#[derive(Default)]
pub(super) struct Completion {
    pub info: Option<rode_session::Info>,
    pub created_persistence: Option<bool>,
    pub ended: Option<String>,
    pub notice: Option<String>,
    pub allocation_cancelled: bool,
}

impl Sessions {
    /// Admit at most eight captured commands; failure leaves earlier commands intact.
    pub(super) fn enqueue(&mut self, command: String, plan: sandbox::Plan) -> Result<(), String> {
        if self.queued.len() >= 8 {
            return Err("Terminal startup command queue is full; command was not run.".into());
        }
        self.queued.push_back(QueuedCommand { command, plan });
        Ok(())
    }

    /// Send commands only to the exact captured policy. Return a requested replacement plan
    /// instead of creating a terminal through the application. A failed creation is never retried.
    pub(super) fn drain_commands(
        &mut self,
        terminal: Option<&Terminal>,
        created_policy: Option<&str>,
    ) -> Result<Option<sandbox::Plan>, String> {
        if self.busy() {
            return Ok(None);
        }
        while let Some(next) = self.queued.front() {
            if !self.matches(terminal, &next.plan) {
                if created_policy == Some(service::fingerprint(&next.plan).as_str()) {
                    self.queued.clear();
                    return Err(
                        "The new terminal exited. Remaining queued commands were not sent.".into(),
                    );
                }
                return Ok(Some(next.plan.clone()));
            }
            let next = self.queued.pop_front().unwrap();
            if let Some(terminal) = terminal
                && let Err(error) = terminal.send((next.command + "\n").into_bytes())
            {
                self.queued.clear();
                return Err(error.to_string());
            }
        }
        Ok(None)
    }

    /// A rejected application creation must not leave commands waiting indefinitely.
    pub(super) fn discard_unstarted_commands(&mut self) {
        if !self.starting {
            self.queued.clear();
        }
    }

    /// Creation and control mutations share one serial admission boundary.
    pub(super) fn busy(&self) -> bool {
        self.starting || self.busy
    }

    /// Retain approved runtime shutdowns while clearing project discovery and view state.
    pub(super) fn reset_context(&mut self) {
        let runtimes = self.runtimes.take_shutdowns();
        let cleanups = std::mem::take(&mut self.cleanups);
        *self = Self {
            runtimes,
            cleanups,
            ..Self::default()
        };
    }

    /// Compatibility admits only the project policy or its explicit normal-mode terminal variant.
    pub(super) fn compatible(info: &rode_session::Info, project: &sandbox::Plan) -> bool {
        info.root == project.root
            && (info.policy == service::fingerprint(project)
                || info.policy == service::fingerprint(&execution::outside_sandbox(project)))
    }

    /// A live view must match the queued command's exact policy before receiving input.
    pub(super) fn matches(&self, terminal: Option<&Terminal>, plan: &sandbox::Plan) -> bool {
        terminal.is_some_and(Terminal::is_alive)
            && self
                .active
                .as_ref()
                .is_some_and(|meta| meta.policy == service::fingerprint(plan))
    }

    /// Release this window's attachments according to their acknowledged host
    /// lifetime policy. Retain process completion flags across project reset.
    pub(in crate::app) fn release_views(&mut self, terminal: &mut Option<Terminal>) {
        let views = terminal
            .take()
            .into_iter()
            .chain(self.other.drain(..).map(|slot| slot.terminal));
        for view in views {
            if view.owns_shell() && view.is_alive() {
                self.cleanups.push(view.alive.clone());
            }
            drop(view);
        }
        self.active = None;
        self.queued.clear();
    }

    /// Locking waits for lifetime edits before releasing temporary attachments.
    /// Persistent shells need explicit End; shared jobs keep their other views.
    pub(in crate::app) fn close_for_lock(
        &mut self,
        terminal: &mut Option<Terminal>,
    ) -> Result<bool, String> {
        if self.starting || self.persistence_pending {
            return Ok(false);
        }
        let persistent = terminal
            .iter()
            .chain(self.other.iter().map(|slot| &slot.terminal))
            .any(|view| view.is_alive() && view.owns_shell() && view.persistent());
        if persistent {
            return Err("End persistent terminal sessions in Sessions before locking this encrypted project. Shared or detached jobs must release the project too.".into());
        }
        self.release_views(terminal);
        Ok(true)
    }

    /// Return presentation snapshots in stable creation order without exposing attachment mutation.
    pub(in crate::app) fn tabs(&self) -> Vec<Meta> {
        let mut tabs: Vec<_> = self
            .other
            .iter()
            .map(|slot| slot.meta.clone())
            .chain(self.active.iter().cloned())
            .collect();
        tabs.sort_by_key(|meta| meta.order);
        tabs
    }

    /// Select an existing hidden view while preserving the previous active attachment.
    pub(super) fn select(&mut self, terminal: &mut Option<Terminal>, key: &str) -> bool {
        let Some(index) = self.other.iter().position(|slot| slot.meta.key == key) else {
            return false;
        };
        let slot = self.other.remove(index);
        self.adopt(terminal, slot.terminal, slot.meta);
        true
    }

    /// Cycle existing attachments; presentation code decides whether to raise native focus.
    pub(super) fn cycle(&mut self, terminal: &mut Option<Terminal>, direction: isize) -> bool {
        let tabs = self.tabs();
        if tabs.len() < 2 {
            return false;
        }
        let index = tabs
            .iter()
            .position(|meta| {
                self.active
                    .as_ref()
                    .is_some_and(|active| active.key == meta.key)
            })
            .unwrap_or(0);
        let target = (index as isize + direction).rem_euclid(tabs.len() as isize) as usize;
        self.select(terminal, &tabs[target].key)
    }

    /// Validate and reserve one creation; rejected allocation drafts retain their local error.
    pub(super) fn prepare_create(
        &mut self,
        request: Create,
    ) -> Result<Option<(service::Operation, Job)>, String> {
        if self.busy() {
            return Ok(None);
        }
        if self.other.len() >= 15 {
            return Err(
                "At most 16 terminal views per window. Detach an existing view first.".into(),
            );
        }
        if !request.ready {
            return Err("Open a connected project before starting a terminal.".into());
        }
        let id = rode_session::new_id().map_err(|error| error.to_string())?;
        self.next += 1;
        let allocation = match request
            .allocation
            .map(|settings| {
                if request.reserve {
                    settings.reserve(&request.plan, &id)
                } else {
                    settings.launch(&request.plan, &id)
                }
            })
            .transpose()
        {
            Ok(launch) => launch,
            Err(error) => {
                self.allocation_error = error.to_string();
                return Ok(None);
            }
        };
        let policy = service::fingerprint(&request.plan);
        self.starting = true;
        Ok(Some((
            service::Operation::Create {
                id,
                name: format!(
                    "{} {}",
                    if request.reserve {
                        "Notebook allocation"
                    } else if allocation.is_some() {
                        "Slurm"
                    } else {
                        "Terminal"
                    },
                    self.next
                ),
                plan: Box::new(request.plan),
                policy: policy.clone(),
                python: request.python,
                environment: Box::new(request.environment),
                persistent: request.persistent,
                allocation,
            },
            Job::Create {
                persistent: request.persistent,
                policy,
            },
        )))
    }

    /// Run a serialized native service request using only its captured host/reply capability.
    pub(super) fn request(
        &mut self,
        operation: service::Operation,
        job: Job,
        root: String,
        work: execution::Work,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<project::Response> {
                if work.target.remote().is_some() {
                    work.target
                        .request(&Request::Tool {
                            request: project::Request::Sessions {
                                root,
                                operation,
                                directory: std::env::var("RODE_SESSION_DIR").ok(),
                            },
                        })?
                        .tool
                        .context("Missing terminal service response")
                } else {
                    Ok(project::Response::Sessions(service::local(&operation)?))
                }
            })()
            .map_err(|error| format!("{error:#}"));
            work.send(WorkspaceEvent::Session { job, result });
        });
    }

    /// Submit a scheduler read or confirmed cancellation with captured executable choices.
    pub(super) fn refresh_slurm(
        &mut self,
        cancel: Option<String>,
        root: String,
        queue: String,
        cancel_program: String,
        work: execution::Work,
    ) {
        if self.slurm_busy {
            return;
        }
        self.slurm_busy = true;
        self.slurm_last = Some(Instant::now());
        let job = if cancel.is_some() {
            Job::Release
        } else {
            Job::Slurm
        };
        let request = Request::Tool {
            request: project::Request::Slurm {
                root,
                queue,
                cancel_program,
                cancel,
            },
        };
        std::thread::spawn(move || {
            let result = work
                .target
                .request(&request)
                .and_then(|response| response.tool.context("Missing Slurm response"))
                .map_err(|error| format!("{error:#}"));
            work.send(WorkspaceEvent::Session { job, result });
        });
    }

    /// Acknowledge matching state and validate creation policy before returning UI effects.
    /// Errors clear the failed domain's queue/busy markers and remain with that domain.
    pub(super) fn complete(
        &mut self,
        job: &Job,
        result: Result<project::Response, String>,
        plan: &sandbox::Plan,
    ) -> Result<Completion, String> {
        if matches!(job, Job::Slurm | Job::Release) {
            self.slurm_busy = false;
        } else {
            self.busy = false;
        }
        if matches!(job, Job::Create { .. }) {
            self.starting = false;
        }
        if matches!(job, Job::Persistence) {
            self.persistence_pending = false;
        }
        let result = result.and_then(|response| {
            if let Job::Create { policy, .. } = job {
                let valid = matches!(&response, project::Response::Sessions(reply)
                    if matches!(&reply.response, rode_session::Response::Session(info)
                        if info.policy == *policy && Self::compatible(info, plan)));
                if !valid { return Err("The terminal service did not return the requested project and execution policy. No commands were sent.".into()); }
            }
            Ok(response)
        });
        let response = result.inspect_err(|error| {
            if matches!(job, Job::Slurm | Job::Release) {
                self.slurm_error = error.clone();
            } else {
                self.error = error.clone();
                self.queued.clear();
            }
        })?;
        let mut effects = Completion::default();
        match response {
            project::Response::Sessions(reply) => {
                self.error.clear();
                self.backend = Some(reply.clone());
                match reply.response {
                    rode_session::Response::Sessions(mut infos) => {
                        infos.retain(|info| info.alive);
                        self.available = infos;
                    }
                    rode_session::Response::Session(info) => {
                        if info.allocation.is_some() && matches!(job, Job::Create { .. }) {
                            self.slurm_last = None;
                        }
                        if let Job::Create { persistent, .. } = job {
                            effects.created_persistence = Some(*persistent);
                        }
                        effects.info = Some(info);
                    }
                    rode_session::Response::Ok => {}
                    rode_session::Response::Error(error) => effects.notice = Some(error),
                    _ => {
                        effects.notice = Some("Unexpected agent reply in terminal controls".into())
                    }
                }
                if let Job::End(id) = job {
                    effects.ended = Some(id.clone());
                }
            }
            project::Response::Slurm(snapshot) => {
                self.slurm = Some(snapshot);
                self.slurm_error.clear();
            }
            project::Response::Command(output) if matches!(job, Job::Release) => {
                if output.success() {
                    self.slurm_error.clear();
                    effects.allocation_cancelled = true;
                } else {
                    self.slurm_error = output.stderr + &output.stdout;
                }
            }
            _ => effects.notice = Some("Unexpected session response".into()),
        }
        Ok(effects)
    }

    /// Keep commands queued only if their created attachment is selected and still alive.
    pub(super) fn finish_creation(&mut self, info: &rode_session::Info) -> Option<String> {
        if !self.active.as_ref().is_some_and(|meta| meta.key == info.id) {
            self.queued.clear();
        }
        if !info.alive {
            self.queued.clear();
            return Some(
                "The new terminal exited. Remaining queued commands were not sent.".into(),
            );
        }
        None
    }

    /// Update metadata and labels for active/hidden views after any requested UI attachment.
    pub(super) fn update_info(
        &mut self,
        active_terminal: &mut Option<Terminal>,
        info: rode_session::Info,
    ) {
        if let Some(meta) = &mut self.active
            && meta.key == info.id
        {
            meta.name = info.name.clone();
            meta.native = Some(info.clone());
            if let Some(terminal) = active_terminal {
                terminal.title = format!("NATIVE · {} / {}", info.name, info.mode);
            }
        }
        for slot in &mut self.other {
            if slot.meta.key == info.id {
                slot.meta.name = info.name.clone();
                slot.meta.native = Some(info.clone());
                slot.terminal.title = format!("NATIVE · {} / {}", info.name, info.mode);
            }
        }
        self.available.retain(|old| old.id != info.id);
        if info.alive {
            self.available.push(info);
        }
    }

    /// Finish an explicit end only for its named view and discovery entry.
    pub(super) fn finish_end(&mut self, terminal: &mut Option<Terminal>, id: &str) {
        self.remove(terminal, id);
        self.available.retain(|info| info.id != id);
    }

    /// Select an existing terminal while retaining the previously active view for later switching.
    pub(super) fn adopt(
        &mut self,
        active_terminal: &mut Option<Terminal>,
        terminal: Terminal,
        meta: Meta,
    ) {
        if let Some(old) = active_terminal.take() {
            old.set_visible(false);
            let old_meta = self.active.take().unwrap_or(Meta {
                key: "temporary-old".into(),
                order: 0,
                name: "Temporary terminal".into(),
                native: None,
                policy: String::new(),
                mode: "Unknown".into(),
            });
            if old.is_alive() {
                self.other.push(Attached {
                    meta: old_meta,
                    terminal: old,
                });
            }
        }
        *active_terminal = Some(terminal);
        self.active = Some(meta);
    }

    /// Release the matching window attachment and select another retained view if available;
    /// the host session follows its acknowledged persistence policy.
    pub(super) fn remove(&mut self, active_terminal: &mut Option<Terminal>, key: &str) {
        self.other.retain(|slot| slot.meta.key != key);
        if self.active.as_ref().is_some_and(|active| active.key == key) {
            *active_terminal = None;
            self.active = None;
            if let Some(slot) = self.other.pop() {
                *active_terminal = Some(slot.terminal);
                self.active = Some(slot.meta);
            }
        }
    }
}

//! Multiple terminal views, native reattachment, Slurm allocation controls and
//! per-session persistence. Session daemons own their PTYs independently.
//!
//! Views belong to this window; shells belong to the host service and obey their explicit
//! persistence flags. Capture policy fingerprints for attachment, serialize pending lifetime
//! edits with close/disconnect, and keep scheduler allocations separate from observed GPU
//! utilization.
use super::*;
use crate::project;
use crate::{sessions as service, slurm};
use std::collections::VecDeque;
mod allocation;
mod controller;
mod runtimes;
#[derive(Clone)]
/// Stable window view identity/order and optional host-owned terminal metadata;
/// view keys and native service IDs describe different lifetimes.
pub(super) struct Meta {
    pub key: String,
    pub(super) order: usize,
    pub name: String,
    pub native: Option<rode_session::Info>,
    pub policy: String,
    pub mode: String,
}

/// Terminal input waiting for the captured project execution plan to become ready.
pub(super) struct QueuedCommand {
    command: String,
    plan: sandbox::Plan,
}

#[cfg(test)]
mod tests;

/// A hidden terminal view retained in this window while another terminal is selected.
struct Attached {
    meta: Meta,
    terminal: Terminal,
}
#[derive(Clone)]
/// Captured terminal/scheduler completion intent carried by generation-tagged worker events.
pub(super) enum Job {
    List,
    Create { persistent: bool, policy: String },
    Rename,
    End(String),
    Slurm,
    Release,
    Persistence,
}
/// Captured identity for a destructive shell or scheduler action awaiting explicit confirmation.
enum Confirm {
    End {
        id: String,
        policy: String,
        name: String,
        clients: usize,
    },
    Temporary(String),
    Release(slurm::Job),
}
#[derive(Default)]
/// Window terminal views, host discovery caches and serialized lifetime mutations.
/// Pending creation/persistence/shutdown work participates in normal exit guards.
pub(super) struct Sessions {
    runtimes: runtimes::Browser,
    pub active: Option<Meta>,
    other: Vec<Attached>,
    pub available: Vec<rode_session::Info>,
    backend: Option<service::Reply>,
    busy: bool,
    starting: bool,
    error: String,
    next: usize,
    view_next: usize,
    pub queued: VecDeque<QueuedCommand>,
    last: Option<Instant>,
    pub(super) slurm: Option<slurm::Snapshot>,
    pub(super) slurm_error: String,
    pub(super) slurm_busy: bool,
    slurm_last: Option<Instant>,
    confirm: Option<Confirm>,
    rename: Option<(String, String)>,
    persistence_pending: bool,
    allocation_draft: slurm::Allocation,
    allocation_error: String,
}
impl Sessions {
    /// Inject persistence acknowledgement state for lifecycle fixtures without starting a worker.
    #[cfg(test)]
    pub(in crate::app) fn persistence_pending_fixture(&mut self, pending: bool) {
        self.persistence_pending = pending;
    }

    /// Creation and persistence mutations must be acknowledged before leaving this window.
    pub(super) fn lifetime_pending(&self) -> bool {
        self.starting || self.persistence_pending
    }
    /// Report whether runtime cleanup still awaits acknowledgement from its background owner.
    pub(super) fn runtime_shutdown_pending(&self) -> bool {
        self.runtimes.shutdown_pending()
    }
    /// Check active and hidden views by stable window-local view key.
    pub(super) fn has_view(&self, key: &str) -> bool {
        self.active.as_ref().is_some_and(|m| m.key == key)
            || self.other.iter().any(|slot| slot.meta.key == key)
    }

    /// Count active and retained hidden terminal views, excluding unattached host sessions.
    pub(super) fn view_count(&self) -> usize {
        self.other.len() + usize::from(self.active.is_some())
    }
}
impl EditorApp {
    /// Cycle retained views through the session owner, then restore native terminal focus.
    pub(in crate::app) fn cycle_terminal(&mut self, direction: isize, ctx: &egui::Context) {
        if self.sessions.cycle(&mut self.terminal, direction) {
            self.show_terminal = true;
            Terminal::focus(ctx);
        }
    }
    /// Include active and hidden attachments without creating another session owner.
    pub(super) fn has_live_terminal_views(&self) -> bool {
        self.sessions.has_live_views(self.terminal.as_ref())
    }
    /// Capture the active project sandbox plan, or the default plan rooted at the current
    /// directory.
    /// Terminal sandbox selection is applied separately when constructing a terminal request.
    pub(in crate::app) fn session_plan(&self) -> sandbox::Plan {
        self.workspace
            .as_ref()
            .map(|workspace| workspace.plan.clone())
            .unwrap_or_else(|| sandbox::Plan {
                root: std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                ..Default::default()
            })
    }
    /// Interactive mode selection never changes the workspace's tool/agent plan.
    pub(in crate::app) fn terminal_plan(&self, sandboxed: bool) -> sandbox::Plan {
        let project = self.session_plan();
        if sandboxed {
            project
        } else {
            execution::outside_sandbox(&project)
        }
    }

    fn compatible_session(&self, info: &rode_session::Info) -> bool {
        Sessions::compatible(info, &self.session_plan())
    }

    fn session_remote(&self) -> Option<Remote> {
        self.workspace
            .as_ref()
            .and_then(|workspace| workspace.remote.clone())
    }
    /// Serialized creation/control admission owned by the terminal controller.
    pub(super) fn session_busy(&self) -> bool {
        self.sessions.busy()
    }
    /// Retire project discovery while preserving acknowledged runtime-shutdown ownership.
    pub(super) fn session_context_reset(&mut self) {
        self.sessions.reset_context();
    }
    /// Register a new window-local terminal identity and make its view active.
    pub(super) fn add_terminal_view(&mut self, terminal: Terminal, key: String, name: String) {
        self.sessions.view_next += 1;
        let plan = self.session_plan();
        self.adopt_terminal(
            terminal,
            Meta {
                key,
                order: self.sessions.view_next,
                name,
                native: None,
                policy: service::fingerprint(&plan),
                mode: plan.label().into(),
            },
        );
    }
    /// Reveal a terminal after its attachment owner has selected the retained view.
    pub(super) fn adopt_terminal(&mut self, terminal: Terminal, meta: Meta) {
        self.sessions.adopt(&mut self.terminal, terminal, meta);
        self.show_terminal = true;
    }
    /// Create a window-owned fallback shell subject to the view bound and captured project policy.
    pub(super) fn new_temporary_terminal(&mut self, ctx: &egui::Context) {
        self.terminal_intent();
        if self.sessions.other.len() >= 15 {
            self.notify(
                "At most 16 terminal views per window. Detach an existing view first.",
                true,
            );
            return;
        }
        let plan = self.terminal_plan(self.workbench.terminal_sandbox());
        let result = match self.session_remote() {
            Some(remote) => Terminal::remote_project(ctx, &remote, &plan),
            None => Terminal::local_project(ctx, &plan),
        };
        match result {
            Ok(terminal) => {
                self.sessions.next += 1;
                self.sessions.view_next += 1;
                let name = format!("Temporary {}", self.sessions.next);
                self.adopt_terminal(
                    terminal,
                    Meta {
                        key: format!("temporary-{}", self.sessions.next),
                        order: self.sessions.view_next,
                        name,
                        native: None,
                        policy: service::fingerprint(&plan),
                        mode: plan.label().into(),
                    },
                );
                self.new_terminal_environment();
                self.activate_python(ctx);
                if let Ok(exports) = self.workbench.environment_settings().exports()
                    && !exports.is_empty()
                    && let Some(terminal) = &self.terminal
                    && let Err(error) = terminal.send((exports + "\n").into_bytes())
                {
                    self.notify(error.to_string(), true);
                }
                self.dialog = Dialog::None;
                Terminal::focus(ctx);
            }
            Err(error) => self.notify(format!("{error:#}"), true),
        }
    }
    /// Request a native host shell explicitly allowed to survive its last view closing.
    pub(super) fn new_persistent_terminal(&mut self, ctx: &egui::Context) {
        self.create_native_terminal(true, ctx);
    }
    /// Request a native host shell whose default lifetime ends after its last view closes.
    pub(super) fn new_terminal(&mut self, ctx: &egui::Context) {
        self.create_native_terminal(false, ctx);
    }
    fn create_native_terminal(&mut self, persistent: bool, ctx: &egui::Context) {
        let plan = self.terminal_plan(self.workbench.terminal_sandbox());
        self.create_terminal(persistent, None, false, plan, ctx);
    }
    fn create_terminal(
        &mut self,
        persistent: bool,
        allocation: Option<slurm::Allocation>,
        reserve: bool,
        plan: sandbox::Plan,
        ctx: &egui::Context,
    ) {
        self.terminal_intent();
        let ready = self
            .workspace
            .as_ref()
            .is_none_or(|w| w.ready && w.connected);
        let request = self.sessions.prepare_create(controller::Create {
            plan,
            persistent,
            allocation,
            reserve,
            ready,
            python: self.terminal_python_selection(),
            environment: self.workbench.environment_settings().clone(),
        });
        match request {
            Ok(Some((operation, job))) => {
                self.show_terminal = true;
                self.session_request(operation, job, ctx);
            }
            Ok(None) => {}
            Err(error) => self.notify(error, true),
        }
    }
    /// Capture one host/reply/lease capability before handing execution to the session owner.
    fn session_request(&mut self, operation: service::Operation, job: Job, ctx: &egui::Context) {
        if self.sessions.busy {
            return;
        }
        let root = self.session_plan().root;
        let work = self.capture_work(execution::Scope::Host, ctx);
        self.sessions.request(operation, job, root, work);
    }
    /// Open terminal/runtime/scheduler discovery and queue independent metadata refreshes.
    pub(super) fn open_sessions(&mut self, ctx: &egui::Context) {
        self.set_dialog(Dialog::Sessions);
        self.refresh_sessions(ctx);
        self.refresh_slurm(None, ctx);
        self.refresh_background_runtimes(ctx);
    }
    fn refresh_sessions(&mut self, ctx: &egui::Context) {
        self.sessions.last = Some(Instant::now());
        self.session_request(service::Operation::List, Job::List, ctx);
    }
    /// Capture scheduler configuration and project authority for the session owner.
    pub(super) fn refresh_slurm(&mut self, cancel: Option<String>, ctx: &egui::Context) {
        if self.sessions.slurm_busy {
            return;
        }
        let root = self.session_plan().root;
        let (queue, cancel_program) = self.slurm_programs();
        let work = self.capture_work(execution::Scope::Project, ctx);
        self.sessions
            .refresh_slurm(cancel, root, queue, cancel_program, work);
    }
    /// Drain runtime completions, advance waiting exits and refresh visible session/scheduler caches.
    pub(super) fn session_tick(&mut self, ctx: &egui::Context) {
        self.poll_runtimes(ctx);
        if self.advance_exit(ctx) {
            return;
        }
        for terminal in self.terminal.iter().chain(self.ssh_master.iter()) {
            terminal.set_visible(self.terminal_any_visible());
        }
        let manager = self.dialog == Dialog::Sessions;
        let allocation_visible = self.terminal_any_visible()
            && self
                .sessions
                .active
                .as_ref()
                .and_then(|m| m.native.as_ref())
                .is_some_and(|info| info.allocation.is_some())
            && self.terminal.as_ref().is_some_and(Terminal::is_alive);
        if manager
            && self
                .sessions
                .last
                .is_none_or(|time| time.elapsed() >= Duration::from_secs(2))
        {
            self.refresh_sessions(ctx);
        }
        let notebook_allocation = self
            .documents
            .iter()
            .any(|doc| doc.notebook.as_ref().is_some_and(|v| v.uses_slurm()));
        if (manager || allocation_visible || notebook_allocation || self.dialog == Dialog::Slurm)
            && self
                .sessions
                .slurm_last
                .is_none_or(|time| time.elapsed() >= Duration::from_secs(5))
        {
            self.refresh_slurm(None, ctx);
        }
        if manager || allocation_visible || notebook_allocation || self.dialog == Dialog::Slurm {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
    }
    fn attach_info(&mut self, info: rode_session::Info, ctx: &egui::Context) {
        if !self.compatible_session(&info) {
            self.notify(
                "Open this terminal's project and sandbox policy before attaching.",
                true,
            );
            return;
        }
        self.terminal_intent();
        if self
            .sessions
            .active
            .as_ref()
            .is_some_and(|active| active.key == info.id)
            && self.terminal.as_ref().is_some_and(Terminal::is_alive)
        {
            self.dialog = Dialog::None;
            self.show_terminal = true;
            Terminal::focus(ctx);
            return;
        }
        if let Some(index) = self
            .sessions
            .other
            .iter()
            .position(|slot| slot.meta.key == info.id)
        {
            let slot = self.sessions.other.remove(index);
            self.adopt_terminal(slot.terminal, slot.meta);
            self.dialog = Dialog::None;
            Terminal::focus(ctx);
            return;
        }
        if self.sessions.other.len() >= 15 {
            self.notify(
                "At most 16 terminal views per window. Detach an existing view first.",
                true,
            );
            return;
        }
        let Some(backend) = self.sessions.backend.clone() else {
            return;
        };
        match Terminal::native_session(ctx, self.session_remote().as_ref(), &backend, &info) {
            Ok(terminal) => {
                let key = info.id.clone();
                let name = info.name.clone();
                self.sessions.view_next += 1;
                self.adopt_terminal(
                    terminal,
                    Meta {
                        key,
                        order: self.sessions.view_next,
                        name,
                        policy: info.policy.clone(),
                        mode: info.mode.clone(),
                        native: Some(info),
                    },
                );
                if self.dialog == Dialog::Sessions {
                    self.dialog = Dialog::None;
                }
                Terminal::focus(ctx);
            }
            Err(error) => self.notify(format!("{error:#}"), true),
        }
    }
    /// Release only the named attachment; the host applies its acknowledged lifetime policy.
    pub(super) fn remove_view(&mut self, key: &str) {
        self.sessions.remove(&mut self.terminal, key);
    }
    /// Release an explicitly persistent attachment; temporary views require their confirmation path.
    pub(super) fn detach_terminal(&mut self, ctx: &egui::Context) {
        if let Some(active) = self.sessions.active.clone() {
            if self.terminal.as_ref().is_some_and(Terminal::persistent) {
                self.remove_view(&active.key);
                self.refresh_sessions(ctx);
            } else if active.native.is_none() {
                self.sessions.confirm = Some(Confirm::Temporary(active.key));
                self.set_dialog(Dialog::Sessions);
            }
        }
    }
    /// Request an in-place lifetime change only for a live capable native terminal,
    /// retaining the pending acknowledgement before close/disconnect can proceed.
    pub(super) fn toggle_terminal_persistence(&mut self, ctx: &egui::Context) {
        if self.session_busy() {
            return;
        }
        let Some(terminal) = self
            .terminal
            .as_ref()
            .filter(|t| t.is_native() && t.is_alive())
        else {
            return;
        };
        let Some(info) = self
            .sessions
            .active
            .as_ref()
            .and_then(|meta| meta.native.as_ref())
        else {
            return;
        };
        let operation = service::Operation::SetPersistence {
            id: info.id.clone(),
            policy: info.policy.clone(),
            persistent: !terminal.persistent(),
        };
        self.sessions.persistence_pending = true;
        self.session_request(operation, Job::Persistence, ctx);
        Terminal::focus(ctx);
    }
    /// Apply only cross-domain presentation/lifecycle effects of an admitted session reply.
    pub(super) fn handle_session(
        &mut self,
        job: Job,
        result: Result<project::Response, String>,
        ctx: &egui::Context,
    ) {
        let effects = match self.sessions.complete(&job, result, &self.session_plan()) {
            Ok(effects) => effects,
            Err(error) => {
                self.abort_lifecycle(error, ctx);
                return;
            }
        };
        if let Some(error) = effects.notice {
            self.notify(error, true);
        }
        if let Some(info) = effects.info {
            if let Some(persistent) = effects.created_persistence {
                if info.persistent != persistent {
                    self.notify("The running terminal service predates persistence controls. This session remains persistent; existing jobs were preserved. Finish old sessions before restarting the service.", true);
                }
                let restore = (self.navigation.active != navigation::Pane::Terminal)
                    .then_some(self.navigation.active);
                let restore_composer = restore == Some(navigation::Pane::Tools)
                    && self.tool_windows.panel == Some(Panel::Agent)
                    && ctx.memory(|memory| memory.has_focus(egui::Id::new("agent-composer")));
                self.attach_info(info.clone(), ctx);
                if let Some(error) = self.sessions.finish_creation(&info) {
                    self.notify(error, true);
                }
                if let Some(pane) = restore {
                    self.focus_pane(pane, ctx);
                    self.agents.focus |= restore_composer;
                }
            }
            self.sessions.update_info(&mut self.terminal, info);
        }
        if let Some(id) = effects.ended {
            self.sessions.finish_end(&mut self.terminal, &id);
        }
        if effects.allocation_cancelled {
            self.status = "Allocation cancellation requested; GPUs remain allocated until Slurm completes cleanup.".into();
            self.refresh_slurm(None, ctx);
        }
        let created_policy = if let Job::Create { policy, .. } = &job {
            Some(policy.as_str())
        } else {
            None
        };
        self.drain_terminal_commands(ctx, created_policy);
        self.advance_exit(ctx);
    }
    /// Render cached terminals, runtimes and scheduler jobs with explicit attach/end/release actions.
    pub(super) fn sessions_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        layout::heading(
            ui,
            "Terminals & Slurm",
            "Manage terminal sessions, allocations and running kernels on this host",
            palette,
        );
        let host = self
            .session_remote()
            .map(|remote| format!("SSH · {}", remote.profile.endpoint()))
            .unwrap_or_else(|| "LOCAL HOST".into());
        ui.add(egui::Label::new(palette.tag(&host)).truncate())
            .on_hover_text(host);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.sessions.busy,
                    crate::keyboard::Button::new("New terminal"),
                )
                .clicked()
            {
                self.new_terminal(ctx);
            }
            if ui
                .add_enabled(
                    !self.sessions.busy,
                    crate::keyboard::Button::new("New persistent terminal"),
                )
                .clicked()
            {
                self.new_persistent_terminal(ctx);
            }
            if crate::keyboard::button(ui, "New temporary terminal")
                .on_hover_text("Temporary terminals stop when this window or project closes.")
                .clicked()
            {
                self.new_temporary_terminal(ctx);
            }
            if ui
                .add_enabled(
                    !self.sessions.busy,
                    crate::keyboard::Button::new("Refresh sessions"),
                )
                .clicked()
            {
                self.refresh_sessions(ctx);
            }
            if self.sessions.busy {
                ui.spinner();
            }
        });
        if !self.sessions.error.is_empty() {
            ui.colored_label(palette.error, &self.sessions.error);
        }
        let available = self.sessions.available.clone();
        super::ui_probe::session_list(ui, available.iter().map(|info| info.id.as_str()));
        // Destructive confirmations remain visible even when the list scrolls.
        self.session_confirmation(ui, ctx);
        let body_height = (ctx.content_rect().height() - 300.).clamp(150., 520.);
        egui::ScrollArea::vertical()
            .id_salt("sessions-body")
            .auto_shrink([false, false])
            .min_scrolled_height(body_height)
            .max_height(body_height)
            .show(ui, |ui| {
                ui.label("Persist: on keeps a terminal running after its last view closes. Persist: off ends it then. Shared attachments control the same shell and persistence setting.");
                if available.is_empty() && !self.sessions.busy {
                    layout::empty(ui, "No terminals on this host", "Create a terminal to start a new session.", palette);
                }
                for info in available {
                    layout::card(ui, palette, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(&info.name);
                            ui.small(if info.persistent { "PERSISTENT" } else { "UNTIL LAST VIEW CLOSES" });
                            ui.label(if info.alive {
                                format!("RUNNING · {} attached", info.clients)
                            } else {
                                "ENDED".into()
                            });
                            let compatible = self.compatible_session(&info);
                            let attach = ui.add_enabled(compatible, crate::keyboard::Button::new("Attach"))
                                .on_hover_text("Open this session's original project and sandbox policy to attach.");
                            super::ui_probe::button(ui, &format!("session-attach:{}", info.id), &attach);
                            if attach.clicked() {
                                self.attach_info(info.clone(), ctx);
                            }
                            if ui.add_enabled(compatible && !self.sessions.busy, crate::keyboard::Button::new("Rename")).clicked() {
                                self.sessions.rename = Some((info.id.clone(), info.name.clone()));
                            }
                            if ui.add_enabled(compatible && !self.sessions.busy, crate::keyboard::Button::new(if info.alive { "End" } else { "Remove" })).clicked() {
                                self.sessions.confirm = Some(Confirm::End {
                                    id: info.id.clone(), policy: info.policy.clone(),
                                    name: info.name.clone(), clients: info.clients,
                                });
                            }
                        });
                        ui.add(egui::Label::new(palette.tag(format!("{} · {}", info.mode, info.root))).truncate())
                            .on_hover_text(&info.root);
                        if let Some((id, mut name)) = self.sessions.rename.clone()
                            && id == info.id
                        {
                            ui.horizontal_wrapped(|ui| {
                                ui.add(crate::keyboard::named("Name", egui::TextEdit::singleline(&mut name).char_limit(64)));
                                if ui.add_enabled(!self.sessions.busy, crate::keyboard::Button::new("Save name")).clicked() {
                                    self.session_request(service::Operation::Rename {
                                        id: id.clone(), policy: info.policy.clone(), name: name.clone(),
                                    }, Job::Rename, ctx);
                                    self.sessions.rename = None;
                                } else {
                                    self.sessions.rename = Some((id, name));
                                }
                                if crate::keyboard::button(ui, "Cancel rename").clicked() {
                                    self.sessions.rename = None;
                                }
                            });
                        }
                    });
                    ui.add_space(8.);
                }
                if !self.sessions.other.is_empty() {
                    ui.separator();
                    ui.label("Other views in this window");
                }
                let local: Vec<_> = self.sessions.other.iter()
                    .map(|slot| (slot.meta.clone(), slot.terminal.is_alive())).collect();
                for (meta, alive) in local {
                    ui.horizontal(|ui| {
                        ui.label(format!("{}{}", meta.name, if alive { "" } else { " · view ended" }));
                        if crate::keyboard::button(ui, "Show").clicked() {
                            if let Some(info) = meta.native.clone() {
                                self.attach_info(info, ctx);
                            } else if let Some(index) = self.sessions.other.iter().position(|slot| slot.meta.key == meta.key) {
                                let slot = self.sessions.other.remove(index);
                                self.adopt_terminal(slot.terminal, slot.meta);
                                self.dialog = Dialog::None;
                                self.terminal_intent();
                                Terminal::focus(ctx);
                            }
                        }
                    });
                }
                ui.add_space(12.);
                layout::section(ui, "Your Slurm allocations", palette, |ui| self.slurm_allocations_ui(ui, ctx));
                layout::section(ui, "Notebook kernels & debuggers", palette, |ui| self.runtimes_ui(ui, ctx));
                self.slurm_settings_ui(ui);
            });
        if crate::keyboard::button(ui, "Close").clicked() {
            self.dialog = Dialog::None;
        }
    }
    fn session_confirmation(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(confirm) = &self.sessions.confirm else {
            return;
        };
        ui.separator();
        let releasing = matches!(confirm, Confirm::Release(_));
        let message = match confirm {
            Confirm::End { name, clients, .. } => format!(
                "End '{name}'? Its shell/programs will stop, affecting {clients} attached views. Release Slurm allocations separately to free their GPUs."
            ),
            Confirm::Temporary(_) => {
                "Close this temporary terminal? Its running commands will stop.".into()
            }
            Confirm::Release(job) => {
                let scope = if job.id.contains('+') {
                    "Slurm cancels this heterogeneous component and its steps, or the entire job for component zero. Pending component cancellation may be refused."
                } else if job.id.contains('_') {
                    "Slurm cancels this array element and its steps. If it has not been split from the array, other array elements can also be cancelled."
                } else {
                    "This cancels the entire job and all its steps."
                };
                let action = if job.state == "PENDING" {
                    "Cancel queued"
                } else {
                    "Release"
                };
                format!(
                    "{action} Slurm allocation {} ({})? {scope} This also stops affected work started outside RODE.",
                    job.id, job.name
                )
            }
        };
        ui.colored_label(self.themes.active().palette.error, message);
        ui.horizontal(|ui| {
            if crate::keyboard::button(ui, "Cancel").clicked() {
                self.sessions.confirm = None;
            }
            // Native session polling and Slurm run through independent workers.
            // A terminal-list refresh must not disable allocation cancellation.
            let ready = if releasing {
                !self.sessions.slurm_busy
            } else {
                !self.sessions.busy
            };
            let response =
                ui.add_enabled(ready, crate::keyboard::Button::new("Confirm · Alt+Enter"));
            super::ui_probe::button(ui, "session-confirm", &response);
            let confirm = response.clicked()
                || ui.input_mut(|input| input.consume_key(egui::Modifiers::ALT, Key::Enter));
            if ready
                && confirm
                && let Some(confirm) = self.sessions.confirm.take()
            {
                match confirm {
                    Confirm::End { id, policy, .. } => self.session_request(
                        service::Operation::End {
                            id: id.clone(),
                            policy,
                        },
                        Job::End(id),
                        ctx,
                    ),
                    Confirm::Temporary(id) => self.remove_view(&id),
                    Confirm::Release(job) => self.refresh_slurm(Some(job.id), ctx),
                }
            }
        });
        ui.separator();
    }
    fn slurm_allocations_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.sessions.slurm_busy,
                    crate::keyboard::Button::new("Refresh allocations"),
                )
                .clicked()
            {
                self.refresh_slurm(None, ctx);
            }
            if self.sessions.slurm_busy {
                ui.spinner();
            }
        });
        if !self.sessions.slurm_error.is_empty() {
            ui.colored_label(palette.muted, &self.sessions.slurm_error);
        }
        let Some(snapshot) = self.sessions.slurm.clone() else {
            return;
        };
        let allocated = snapshot
            .jobs
            .iter()
            .filter(|job| job.allocated())
            .filter_map(|job| job.gpus)
            .fold(0u64, u64::saturating_add);
        let unknown = snapshot
            .jobs
            .iter()
            .filter(|job| job.allocated() && job.gpus.is_none())
            .count();
        ui.strong(format!(
            "{allocated} GPUs allocated{}",
            if unknown > 0 {
                format!(" · {unknown} allocations have unknown GPU counts")
            } else {
                String::new()
            }
        ));
        ui.label(
            palette.tag(
                "Scheduler allocations, not GPU utilization. Pending GPU requests are separate.",
            ),
        );
        if !snapshot.warning.is_empty() {
            ui.colored_label(palette.muted, snapshot.warning);
        }
        for job in snapshot.jobs {
            layout::card(ui, palette, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("Job {} · {}", job.id, job.state));
                    ui.label(format!(
                        "{} GPUs {}",
                        job.gpus
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "?".into()),
                        if job.allocated() {
                            "allocated"
                        } else {
                            "requested"
                        }
                    ));
                    let action = if job.state == "PENDING" {
                        "Cancel queued allocation"
                    } else {
                        "Release allocation"
                    };
                    let release = ui.add_enabled(
                        !self.sessions.slurm_busy,
                        crate::keyboard::Button::new(action),
                    );
                    super::ui_probe::button(ui, action, &release);
                    if release.clicked() {
                        self.sessions.confirm = Some(Confirm::Release(job.clone()));
                    }
                });
                ui.label(format!(
                    "{} · {} · {} elapsed · {} left",
                    job.name, job.nodes, job.elapsed, job.remaining
                ));
                ui.add(egui::Label::new(palette.tag(&job.tres)).truncate());
            });
            ui.add_space(8.);
        }
    }
    /// Render window-owned terminal views using their stable keys and retained creation order.
    pub(super) fn terminal_tabs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let slots = self.sessions.tabs();
        if slots.len() < 2 {
            return;
        }
        egui::ScrollArea::horizontal()
            .id_salt("terminal-tabs")
            .max_height(25.)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for meta in slots {
                        let selected = self
                            .sessions
                            .active
                            .as_ref()
                            .is_some_and(|active| active.key == meta.key);
                        if crate::keyboard::selectable_label(ui, selected, &meta.name).clicked()
                            && self.sessions.select(&mut self.terminal, &meta.key)
                        {
                            self.show_terminal = true;
                            Terminal::focus(ctx);
                        }
                    }
                });
            });
    }
    /// Admit a bounded captured command through its owner before requesting a suitable view.
    pub(super) fn queue_terminal_command(
        &mut self,
        command: String,
        plan: sandbox::Plan,
        ctx: &egui::Context,
    ) {
        match self.sessions.enqueue(command, plan) {
            Ok(()) => self.drain_terminal_commands(ctx, None),
            Err(error) => self.notify(error, true),
        }
    }

    /// Apply a session owner's request for a matching terminal while respecting application exit.
    fn drain_terminal_commands(&mut self, ctx: &egui::Context, created_policy: Option<&str>) {
        if self.exit.waiting() {
            return;
        }
        match self
            .sessions
            .drain_commands(self.terminal.as_ref(), created_policy)
        {
            Ok(Some(plan)) => {
                self.create_terminal(false, None, false, plan, ctx);
                self.sessions.discard_unstarted_commands();
            }
            Ok(None) => {}
            Err(error) => self.notify(error, true),
        }
    }

    /// One creation menu always requires an explicit execution mode. The current
    /// shell's policy stays read-only in its title and in the menu's details.
    pub(in crate::app) fn terminal_new_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mode = self.sessions.active.as_ref().map(|meta| meta.mode.as_str());
        let detail = mode.unwrap_or("No project terminal attached").to_owned();
        let project = self.session_plan();
        let ready = !self.session_busy()
            && !self.workspace.preparing()
            && !self.vaults.locking()
            && self
                .workspace
                .as_ref()
                .is_none_or(|w| w.ready && w.connected);
        let height = layout::compact_control_height(ui);
        let palette = self.themes.active().palette;
        ui.add_enabled_ui(ready, |ui| {
            let arrow_size = 12.;
            let arrow = egui::Atom::custom(
                ui.next_auto_id().with("terminal-new-arrow"),
                egui::Vec2::splat(arrow_size),
            );
            let menu = crate::keyboard::MenuButton::from_button(
                egui::Button::new(("New terminal", arrow)).small().min_size(egui::vec2(0., height)), "New terminal").ui(ui, |ui| {
                ui.set_width(240.);
                ui.label(RichText::new("Current terminal").small().color(palette.muted));
                ui.add(egui::Label::new(RichText::new(&detail).strong()).wrap());
                ui.separator();
                ui.label(RichText::new("Open new terminal").small().color(palette.muted));
                let unavailable = if project.requested {
                    project.reason.as_str()
                } else {
                    "Open a project and enable its sandbox in Settings to open sandboxed terminals."
                };
                for (sandboxed, title) in [(true, "Sandboxed"), (false, "Outside sandbox")] {
                    let response = ui.add_enabled(
                        !sandboxed || project.active,
                        icons::IconButton::new(Icon::Terminal, title)
                            .min_size(egui::vec2(ui.available_width(), 28.)),
                    ).on_disabled_hover_text(unavailable);
                    super::ui_probe::button(ui, if sandboxed { "terminal-new-sandbox" } else { "terminal-new-host" }, &response);
                    if response.clicked() {
                        self.workbench.save_terminal_sandbox(sandboxed);
                        self.create_native_terminal(false, ctx);
                        ui.close();
                    }
                }
                ui.separator();
                ui.add(egui::Label::new(RichText::new("Existing terminals keep running.").small().color(palette.muted)).wrap());
                if !project.active {
                    let hint = if project.requested {
                        "Project sandbox unavailable."
                    } else {
                        "Enable sandbox in Settings."
                    };
                    ui.add(egui::Label::new(RichText::new(hint).small().color(palette.muted)).wrap())
                        .on_hover_text(unavailable);
                }
            });
            Icon::ChevronDown.paint(
                ui.painter(),
                egui::Rect::from_center_size(
                    egui::pos2(
                        menu.0.rect.right() - ui.spacing().button_padding.x - arrow_size / 2.,
                        menu.0.rect.center().y,
                    ),
                    egui::Vec2::splat(arrow_size),
                ),
                ui.style().interact(&menu.0).text_color(),
            );
            super::ui_probe::button(ui, "terminal-new-menu", &menu.0);
            menu.0.on_hover_text("Create a terminal on this host. Choose sandboxed or outside the sandbox; existing terminals keep running.");
        });
    }
}

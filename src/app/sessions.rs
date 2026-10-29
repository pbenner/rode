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
use anyhow::Context;
use std::collections::VecDeque;
#[derive(Clone)]
pub(super) struct Meta {
    pub key: String,
    pub(super) order: usize,
    pub name: String,
    pub native: Option<rode_session::Info>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quitting_waits_for_a_toggle_and_keeps_the_window_open_if_it_fails() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.sessions.persistence_pending = true;
        app.quit_terminals(&ctx);
        assert!(!app.quitting);
        assert!(app.dialog == Dialog::TerminalExit);
        app.handle_session(Job::Persistence, Err("Host connection lost".into()), &ctx);
        assert!(!app.quitting);
        assert!(!app.sessions.quit_waiting);
        assert!(app.dialog == Dialog::None);
        assert!(
            app.notice.as_ref().is_some_and(
                |(message, error, _)| *error && message.contains("Host connection lost")
            )
        );
    }

    #[test]
    fn quit_still_guards_unsaved_source_before_any_terminal_cleanup() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].text = "keep these edits".into();
        app.documents[0].modified = true;
        app.guarded(Action::Quit, &ctx);
        assert!(matches!(app.pending, Some(Action::Quit)));
        assert!(!app.quitting);
        assert!(!app.sessions.quit_waiting);
        assert_eq!(app.documents[0].text, "keep these edits");
    }

    #[test]
    fn disconnect_waits_for_terminal_startup_and_persistence_changes() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.workspace = Some(crate::app::tests::remote_workspace());
        for starting in [true, false] {
            app.sessions.starting = starting;
            app.sessions.persistence_pending = !starting;
            let generation = app.generation;
            app.disconnect_ssh(&ctx);
            assert!(app.workspace.as_ref().unwrap().connected);
            assert_eq!(app.generation, generation);
            assert!(!app.connection_cancel.load(Ordering::Acquire));
        }
    }
}
struct Attached {
    meta: Meta,
    terminal: Terminal,
}
#[derive(Clone)]
pub(super) enum Job {
    List,
    Create { persistent: bool },
    Rename,
    End(String),
    Slurm,
    Release,
    Persistence,
}
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
pub(super) struct Sessions {
    pub active: Option<Meta>,
    other: Vec<Attached>,
    pub available: Vec<rode_session::Info>,
    backend: Option<service::Reply>,
    busy: bool,
    starting: bool,
    helper_ready: bool,
    error: String,
    next: usize,
    view_next: usize,
    pub queued: VecDeque<String>,
    last: Option<Instant>,
    slurm: Option<slurm::Snapshot>,
    slurm_error: String,
    slurm_busy: bool,
    slurm_last: Option<Instant>,
    confirm: Option<Confirm>,
    rename: Option<(String, String)>,
    persistence_pending: bool,
    quit_waiting: bool,
}
impl Sessions {
    pub(super) fn has_view(&self, key: &str) -> bool {
        self.active.as_ref().is_some_and(|m| m.key == key)
            || self.other.iter().any(|slot| slot.meta.key == key)
    }

    pub(super) fn view_count(&self) -> usize {
        self.other.len() + usize::from(self.active.is_some())
    }
}
impl EditorApp {
    pub(in crate::app) fn cycle_terminal(&mut self, direction: isize, ctx: &egui::Context) {
        let mut views: Vec<_> = self
            .sessions
            .other
            .iter()
            .map(|s| s.meta.clone())
            .chain(self.sessions.active.iter().cloned())
            .collect();
        views.sort_by_key(|m| m.order);
        if views.len() < 2 {
            return;
        }
        let index = views
            .iter()
            .position(|m| {
                self.sessions
                    .active
                    .as_ref()
                    .is_some_and(|a| a.key == m.key)
            })
            .unwrap_or(0);
        let target = (index as isize + direction).rem_euclid(views.len() as isize) as usize;
        if let Some(index) = self
            .sessions
            .other
            .iter()
            .position(|s| s.meta.key == views[target].key)
        {
            let slot = self.sessions.other.remove(index);
            self.adopt_terminal(slot.terminal, slot.meta);
            Terminal::focus(ctx);
        }
    }
    pub(super) fn has_live_terminal_views(&self) -> bool {
        self.terminal.as_ref().is_some_and(Terminal::is_alive)
            || self
                .sessions
                .other
                .iter()
                .any(|slot| slot.terminal.is_alive())
    }
    fn session_plan(&self) -> sandbox::Plan {
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
    fn session_remote(&self) -> Option<Remote> {
        self.workspace
            .as_ref()
            .and_then(|workspace| workspace.remote.clone())
    }
    pub(super) fn session_exit_pending(&self) -> bool {
        self.sessions.quit_waiting
    }
    pub(super) fn session_busy(&self) -> bool {
        self.sessions.starting || self.sessions.busy
    }
    pub(super) fn terminal_persistence_supported(&self) -> bool {
        self.sessions
            .active
            .as_ref()
            .and_then(|meta| meta.native.as_ref())
            .is_some_and(|info| info.persistence_supported)
    }
    pub(super) fn sessions_need_exit_check(&self) -> bool {
        self.sessions.starting || self.sessions.persistence_pending || self.debugger_stopping()
    }
    pub(super) fn session_context_reset(&mut self) {
        self.sessions = Sessions::default();
    }
    pub(super) fn add_terminal_view(&mut self, terminal: Terminal, key: String, name: String) {
        self.sessions.view_next += 1;
        self.adopt_terminal(
            terminal,
            Meta {
                key,
                order: self.sessions.view_next,
                name,
                native: None,
            },
        );
    }
    pub(super) fn adopt_terminal(&mut self, terminal: Terminal, meta: Meta) {
        if let Some(old) = self.terminal.take() {
            old.set_visible(false);
            let old_meta = self.sessions.active.take().unwrap_or(Meta {
                key: "temporary-old".into(),
                order: 0,
                name: "Temporary terminal".into(),
                native: None,
            });
            if old.is_alive() {
                self.sessions.other.push(Attached {
                    meta: old_meta,
                    terminal: old,
                });
            }
        }
        self.terminal = Some(terminal);
        self.sessions.active = Some(meta);
        self.show_terminal = true;
    }
    pub(super) fn new_temporary_terminal(&mut self, ctx: &egui::Context) {
        self.navigation.terminal_intent();
        if self.sessions.other.len() >= 15 {
            self.notify(
                "At most 16 terminal views per window. Detach an existing view first.",
                true,
            );
            return;
        }
        let plan = self.session_plan();
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
                    },
                );
                self.new_terminal_environment();
                self.activate_python(ctx);
                self.dialog = Dialog::None;
                Terminal::focus(ctx);
            }
            Err(error) => self.notify(format!("{error:#}"), true),
        }
    }
    pub(super) fn new_persistent_terminal(&mut self, ctx: &egui::Context) {
        self.create_native_terminal(true, ctx);
    }
    pub(super) fn new_terminal(&mut self, ctx: &egui::Context) {
        self.create_native_terminal(false, ctx);
    }
    fn create_native_terminal(&mut self, persistent: bool, ctx: &egui::Context) {
        self.navigation.terminal_intent();
        if self.sessions.busy {
            return;
        }
        if self.sessions.other.len() >= 15 {
            self.notify(
                "At most 16 terminal views per window. Detach an existing view first.",
                true,
            );
            return;
        }
        if self
            .workspace
            .as_ref()
            .is_some_and(|workspace| !workspace.ready || !workspace.connected)
        {
            self.notify("Open a connected project before starting a terminal.", true);
            return;
        }
        let id = match rode_session::new_id() {
            Ok(id) => id,
            Err(error) => {
                self.notify(error.to_string(), true);
                return;
            }
        };
        self.sessions.next += 1;
        let plan = self.session_plan();
        let policy = service::fingerprint(&plan);
        let python = self.terminal_python_selection();
        self.sessions.starting = true;
        self.show_terminal = true;
        self.session_request(
            service::Operation::Create {
                id,
                name: format!("Terminal {}", self.sessions.next),
                plan: Box::new(plan),
                policy,
                python,
                persistent,
            },
            Job::Create { persistent },
            ctx,
        );
    }
    fn session_request(&mut self, operation: service::Operation, job: Job, ctx: &egui::Context) {
        if self.sessions.busy {
            return;
        }
        self.sessions.busy = true;
        let remote = self.session_remote();
        let root = self.session_plan().root;
        let generation = self.generation;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let upload = !self.sessions.helper_ready;
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let result = (|| -> anyhow::Result<project::Response> {
                if let Some(remote) = remote {
                    let helper = Some(service::helper(upload)?);
                    remote
                        .request_in(
                            &Request::Tool {
                                request: project::Request::Sessions {
                                    root,
                                    operation,
                                    helper,
                                    directory: std::env::var("RODE_SESSION_DIR").ok(),
                                },
                            },
                            None,
                        )?
                        .tool
                        .context("Missing terminal service response")
                } else {
                    Ok(project::Response::Sessions(service::local(&operation)?))
                }
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::Session {
                generation,
                job,
                result,
            });
            ctx.request_repaint();
        });
    }
    pub(super) fn open_sessions(&mut self, ctx: &egui::Context) {
        self.set_dialog(Dialog::Sessions);
        self.refresh_sessions(ctx);
        self.refresh_slurm(None, ctx);
    }
    fn refresh_sessions(&mut self, ctx: &egui::Context) {
        self.sessions.last = Some(Instant::now());
        self.session_request(service::Operation::List, Job::List, ctx);
    }
    fn refresh_slurm(&mut self, cancel: Option<String>, ctx: &egui::Context) {
        if self.sessions.slurm_busy {
            return;
        }
        self.sessions.slurm_busy = true;
        self.sessions.slurm_last = Some(Instant::now());
        let job = if cancel.is_some() {
            Job::Release
        } else {
            Job::Slurm
        };
        let root = self.session_plan().root;
        let (queue, cancel_program) = self.slurm_programs();
        let remote = self.session_remote();
        let plan = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.plan.clone());
        let request = Request::Tool {
            request: project::Request::Slurm {
                root,
                queue,
                cancel_program,
                cancel,
            },
        };
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let generation = self.generation;
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let result = (match remote {
                Some(remote) => remote.request_in(&request, plan.as_ref()),
                None => match plan {
                    Some(plan) => plan.local_request(&request),
                    None => files::local(&request),
                },
            })
            .and_then(|response| response.tool.context("Missing Slurm response"))
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::Session {
                generation,
                job,
                result,
            });
            ctx.request_repaint();
        });
    }
    pub(super) fn session_tick(&mut self, ctx: &egui::Context) {
        if self.sessions.quit_waiting && !self.sessions_need_exit_check() {
            self.final_quit(ctx);
            return;
        }
        for terminal in self.terminal.iter().chain(self.ssh_master.iter()) {
            terminal.set_visible(
                self.show_terminal && self.navigation.visible(navigation::Pane::Terminal),
            );
        }
        if self.dialog != Dialog::Sessions {
            return;
        }
        if self
            .sessions
            .last
            .is_none_or(|time| time.elapsed() >= Duration::from_secs(2))
        {
            self.refresh_sessions(ctx);
        }
        if self
            .sessions
            .slurm_last
            .is_none_or(|time| time.elapsed() >= Duration::from_secs(15))
        {
            self.refresh_slurm(None, ctx);
        }
        ctx.request_repaint_after(Duration::from_secs(2));
    }
    fn attach_info(&mut self, info: rode_session::Info, ctx: &egui::Context) {
        self.navigation.terminal_intent();
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
    pub(super) fn remove_view(&mut self, key: &str) {
        self.sessions.other.retain(|slot| slot.meta.key != key);
        if self
            .sessions
            .active
            .as_ref()
            .is_some_and(|active| active.key == key)
        {
            self.terminal = None;
            self.sessions.active = None;
            if let Some(slot) = self.sessions.other.pop() {
                self.terminal = Some(slot.terminal);
                self.sessions.active = Some(slot.meta);
            }
        }
    }
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
    pub(super) fn toggle_terminal_persistence(&mut self, ctx: &egui::Context) {
        if self.session_busy() {
            return;
        }
        let Some(terminal) = self.terminal.as_ref().filter(|t| t.native && t.is_alive()) else {
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
        if !info.persistence_supported {
            self.notify(rode_session::LEGACY_PERSISTENCE_HELP, true);
            return;
        }
        let operation = service::Operation::SetPersistence {
            id: info.id.clone(),
            policy: info.policy.clone(),
            persistent: !terminal.persistent(),
        };
        self.sessions.persistence_pending = true;
        self.session_request(operation, Job::Persistence, ctx);
        Terminal::focus(ctx);
    }
    pub(super) fn handle_session(
        &mut self,
        job: Job,
        result: Result<project::Response, String>,
        ctx: &egui::Context,
    ) {
        if matches!(job, Job::Slurm | Job::Release) {
            self.sessions.slurm_busy = false;
        } else {
            self.sessions.busy = false;
        }
        if matches!(job, Job::Create { .. }) {
            self.sessions.starting = false;
        }
        if matches!(job, Job::Persistence) {
            self.sessions.persistence_pending = false;
        }
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                if matches!(job, Job::Slurm | Job::Release) {
                    self.sessions.slurm_error = error;
                } else {
                    self.sessions.error = error.clone();
                    self.sessions.helper_ready = false;
                    self.sessions.queued.clear();
                    self.notify(error, true);
                }
                if matches!(job, Job::Persistence) && self.sessions.quit_waiting {
                    self.sessions.quit_waiting = false;
                    self.dialog = Dialog::None;
                    self.focus_editor(ctx);
                } else if self.sessions.quit_waiting && !self.sessions_need_exit_check() {
                    self.final_quit(ctx);
                }
                return;
            }
        };
        match response {
            project::Response::Sessions(reply) => {
                self.sessions.error.clear();
                self.sessions.helper_ready = true;
                self.sessions.backend = Some(reply.clone());
                match reply.response {
                    rode_session::Response::Sessions(mut infos) => {
                        infos.retain(|info| info.alive);
                        self.sessions.available = infos;
                        if !self.sessions.queued.is_empty()
                            && self
                                .terminal
                                .as_ref()
                                .is_none_or(|terminal| !terminal.is_alive())
                        {
                            self.new_terminal(ctx);
                        }
                    }
                    rode_session::Response::Session(info) => {
                        if let Job::Create { persistent } = &job {
                            if info.persistent != *persistent {
                                self.notify("The running terminal service predates persistence controls. This session remains persistent; existing jobs were preserved. Finish old sessions before restarting the service.", true);
                            }
                            let restore = if self.navigation.used
                                && self.navigation.active != navigation::Pane::Terminal
                            {
                                Some(self.navigation.active)
                            } else {
                                None
                            };
                            self.attach_info(info.clone(), ctx);
                            if let Some(pane) = restore {
                                self.focus_pane(pane, ctx);
                            }
                            while let Some(command) = self.sessions.queued.pop_front() {
                                if let Some(terminal) = &self.terminal
                                    && let Err(error) = terminal.send((command + "\n").into_bytes())
                                {
                                    self.notify(error.to_string(), true);
                                }
                            }
                        }
                        if let Some(meta) = &mut self.sessions.active
                            && meta.key == info.id
                        {
                            meta.name = info.name.clone();
                            meta.native = Some(info.clone());
                            if let Some(terminal) = &mut self.terminal {
                                terminal.title = format!("NATIVE · {} / {}", info.name, info.mode);
                            }
                        }
                        for slot in &mut self.sessions.other {
                            if slot.meta.key == info.id {
                                slot.meta.name = info.name.clone();
                                slot.meta.native = Some(info.clone());
                                slot.terminal.title =
                                    format!("NATIVE · {} / {}", info.name, info.mode);
                            }
                        }
                        self.sessions.available.retain(|old| old.id != info.id);
                        if info.alive {
                            self.sessions.available.push(info);
                        }
                    }
                    rode_session::Response::Ok => {}
                    rode_session::Response::Error(error) => self.notify(error, true),
                }
                if let Job::End(id) = &job {
                    self.remove_view(id);
                    self.sessions.available.retain(|info| info.id != *id);
                }
            }
            project::Response::Slurm(snapshot) => {
                self.sessions.slurm = Some(snapshot);
                self.sessions.slurm_error.clear();
            }
            project::Response::Command(output) if matches!(job, Job::Release) => {
                if output.success() {
                    self.sessions.slurm_error.clear();
                    self.status="Allocation cancellation requested; GPUs remain allocated until Slurm completes cleanup.".into();
                    self.refresh_slurm(None, ctx);
                } else {
                    self.sessions.slurm_error = output.stderr + &output.stdout;
                }
            }
            _ => self.notify("Unexpected session response", true),
        }
        if self.sessions.quit_waiting && !self.sessions_need_exit_check() {
            self.final_quit(ctx);
        }
    }
    pub(super) fn sessions_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.heading("Terminals & Slurm");
        let host = self
            .session_remote()
            .map(|remote| format!("SSH · {}", remote.profile.endpoint()))
            .unwrap_or_else(|| "LOCAL HOST".into());
        ui.add(egui::Label::new(palette.tag(&host)).truncate())
            .on_hover_text(host);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.sessions.busy, egui::Button::new("New terminal"))
                .clicked()
            {
                self.new_terminal(ctx);
            }
            if ui
                .add_enabled(
                    !self.sessions.busy,
                    egui::Button::new("New persistent terminal"),
                )
                .clicked()
            {
                self.new_persistent_terminal(ctx);
            }
            if ui
                .button("New temporary terminal")
                .on_hover_text("Temporary terminals stop when this window or project closes.")
                .clicked()
            {
                self.new_temporary_terminal(ctx);
            }
            if ui
                .add_enabled(!self.sessions.busy, egui::Button::new("Refresh sessions"))
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
        let fingerprint = service::fingerprint(&self.session_plan());
        let available = self.sessions.available.clone();
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
                    ui.label(palette.tag("No native terminals on this host yet."));
                }
                for info in available {
                    ui.group(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(&info.name);
                            ui.small(if info.persistent { "PERSISTENT" } else { "UNTIL LAST VIEW CLOSES" });
                            ui.label(if info.alive {
                                format!("RUNNING · {} attached", info.clients)
                            } else {
                                "ENDED".into()
                            });
                            let compatible = info.policy == fingerprint;
                            if ui.add_enabled(compatible, egui::Button::new("Attach"))
                                .on_hover_text("Open this session's original project and sandbox policy to attach.")
                                .clicked()
                            {
                                self.attach_info(info.clone(), ctx);
                            }
                            if ui.add_enabled(compatible && !self.sessions.busy, egui::Button::new("Rename")).clicked() {
                                self.sessions.rename = Some((info.id.clone(), info.name.clone()));
                            }
                            if ui.add_enabled(compatible && !self.sessions.busy, egui::Button::new(if info.alive { "End" } else { "Remove" })).clicked() {
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
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut name).char_limit(64));
                                if ui.add_enabled(!self.sessions.busy, egui::Button::new("Save name")).clicked() {
                                    self.session_request(service::Operation::Rename {
                                        id: id.clone(), policy: fingerprint.clone(), name: name.clone(),
                                    }, Job::Rename, ctx);
                                    self.sessions.rename = None;
                                } else {
                                    self.sessions.rename = Some((id, name));
                                }
                                if ui.button("Cancel rename").clicked() {
                                    self.sessions.rename = None;
                                }
                            });
                        }
                    });
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
                        if ui.button("Show").clicked() {
                            if let Some(info) = meta.native.clone() {
                                self.attach_info(info, ctx);
                            } else if let Some(index) = self.sessions.other.iter().position(|slot| slot.meta.key == meta.key) {
                                let slot = self.sessions.other.remove(index);
                                self.adopt_terminal(slot.terminal, slot.meta);
                                self.dialog = Dialog::None;
                                self.navigation.terminal_intent();
                                Terminal::focus(ctx);
                            }
                        }
                    });
                }
                self.slurm_allocations_ui(ui, ctx);
                self.slurm_settings_ui(ui);
            });
        if ui.button("Close").clicked() {
            self.dialog = Dialog::None;
        }
    }
    fn session_confirmation(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(confirm) = &self.sessions.confirm else {
            return;
        };
        ui.separator();
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
                format!(
                    "Release Slurm allocation {} ({})? {scope} This also stops affected work started outside RODE.",
                    job.id, job.name
                )
            }
        };
        ui.colored_label(self.themes.active().palette.error, message);
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                self.sessions.confirm = None;
            }
            let ready = !self.sessions.busy && !self.sessions.slurm_busy;
            let confirm = ui
                .add_enabled(ready, egui::Button::new("Confirm · Alt+Enter"))
                .clicked()
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
        ui.separator();
        ui.heading("Your Slurm allocations");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.sessions.slurm_busy,
                    egui::Button::new("Refresh allocations"),
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
            ui.group(|ui| {
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
                    if ui
                        .add_enabled(
                            !self.sessions.slurm_busy,
                            egui::Button::new("Release allocation"),
                        )
                        .clicked()
                    {
                        self.sessions.confirm = Some(Confirm::Release(job.clone()));
                    }
                });
                ui.label(format!(
                    "{} · {} · {} elapsed · {} left",
                    job.name, job.nodes, job.elapsed, job.remaining
                ));
                ui.add(egui::Label::new(palette.tag(&job.tres)).truncate());
            });
        }
    }
    pub(super) fn terminal_tabs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut slots: Vec<_> = self
            .sessions
            .other
            .iter()
            .map(|slot| slot.meta.clone())
            .collect();
        if let Some(active) = &self.sessions.active {
            slots.push(active.clone());
        }
        slots.sort_by_key(|meta| meta.order);
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
                        if ui.selectable_label(selected, &meta.name).clicked()
                            && let Some(index) = self
                                .sessions
                                .other
                                .iter()
                                .position(|slot| slot.meta.key == meta.key)
                        {
                            let slot = self.sessions.other.remove(index);
                            self.adopt_terminal(slot.terminal, slot.meta);
                            Terminal::focus(ctx);
                        }
                    }
                });
            });
    }
    pub(super) fn queue_terminal_command(&mut self, command: String, ctx: &egui::Context) {
        if self.sessions.queued.len() >= 8 {
            self.notify(
                "Terminal startup command queue is full; command was not run.",
                true,
            );
            return;
        }
        self.sessions.queued.push_back(command);
        if !self.sessions.starting {
            self.new_terminal(ctx);
        }
    }
    fn final_quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    pub(super) fn quit_terminals(&mut self, ctx: &egui::Context) {
        self.stop_debugger();
        if self.sessions_need_exit_check() {
            self.sessions.quit_waiting = true;
            self.set_dialog(Dialog::TerminalExit);
        } else {
            self.final_quit(ctx);
        }
    }
    pub(super) fn terminal_exit_dialog(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        self.sessions.quit_waiting = true;
        ui.heading("Closing RODE");
        ui.spinner();
        ui.label("Finishing terminal changes and stopping the debugger…");
        ui.small("RODE will close automatically when it completes.");
    }
}

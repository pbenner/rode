//! Explicit discovery and reattachment of host-owned notebook/debugger state.
mod ending;
use super::*;
use rode_runtime_core::service::{Info, Kind};
#[derive(Default)]
/// Cached host runtime list, one discovery request and independent confirmed termination work.
pub(super) struct Browser {
    items: Vec<Info>,
    pending: Option<mpsc::Receiver<Result<rode_session::Response, String>>>,
    last: Option<Instant>,
    error: String,
    confirm: Option<ending::Request>,
    endings: Option<ending::Queue>,
    attaching: Option<Info>,
    helper_ready: bool,
    document: Option<String>,
    debugger: bool,
}
impl Browser {
    /// Report confirmed runtime mutations that must survive context reset and delay normal exit.
    pub(super) fn shutdown_pending(&self) -> bool {
        self.endings.as_ref().is_some_and(ending::Queue::busy)
    }
    /// Approved mutations retain their original host context across a project switch.
    pub(super) fn take_shutdowns(&mut self) -> Self {
        Self {
            endings: self.endings.take(),
            ..Self::default()
        }
    }
}
impl EditorApp {
    /// Discover detached debuggers for the current policy, attaching directly only when unambiguous.
    pub(in crate::app) fn find_debugger_runtime(&mut self, ctx: &egui::Context) {
        self.sessions.runtimes.debugger = true;
        self.refresh_background_runtimes(ctx);
    }
    /// Discover detached kernels for a captured notebook path and current project policy.
    pub(in crate::app) fn find_notebook_runtime(&mut self, path: String, ctx: &egui::Context) {
        self.sessions.runtimes.document = Some(path);
        self.refresh_background_runtimes(ctx);
    }
    /// Queue read-only metadata discovery without creating a notebook kernel or debugger.
    pub(in crate::app) fn refresh_background_runtimes(&mut self, ctx: &egui::Context) {
        if self.sessions.runtimes.pending.is_some() {
            return;
        }
        let plan = self.session_plan();
        let remote = self.session_remote();
        let (tx, rx) = mpsc::channel();
        self.sessions.runtimes.pending = Some(rx);
        self.sessions.runtimes.last = Some(Instant::now());
        let wake = ctx.clone();
        let upload = !self.sessions.runtimes.helper_ready;
        std::thread::spawn(move || {
            let request = rode_session::Request::Runtimes;
            let result =
                crate::runtime::request_with_upload(&plan, remote.as_ref(), request, upload)
                    .map(|r| r.response)
                    .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            wake.request_repaint();
        });
    }
    /// Drain discovery and independent shutdown acknowledgements; only matching runtime
    /// identities attach, and failed termination cancels a waiting application exit.
    pub(super) fn poll_runtimes(&mut self, ctx: &egui::Context) {
        if let Some(endings) = &mut self.sessions.runtimes.endings {
            let result = endings.poll();
            if !matches!(result, Ok(false)) {
                self.sessions.runtimes.last = None;
            }
            if endings.busy() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            if let Err(error) = result {
                self.abort_session_quit(error, ctx);
            }
        }
        if let Some(result) = self
            .sessions
            .runtimes
            .pending
            .as_ref()
            .and_then(|r| r.try_recv().ok())
        {
            self.sessions.runtimes.pending = None;
            match result {
                Ok(rode_session::Response::Runtimes(items)) => {
                    self.sessions.runtimes.helper_ready = true;
                    self.sessions.runtimes.items = items;
                    self.sessions.runtimes.error.clear();
                    if std::mem::take(&mut self.sessions.runtimes.debugger) {
                        let policy = service::fingerprint(&self.session_plan());
                        let found: Vec<_> = self
                            .sessions
                            .runtimes
                            .items
                            .iter()
                            .filter(|i| {
                                i.kind == Kind::Debugger && i.policy == policy && !i.attached
                            })
                            .cloned()
                            .collect();
                        if found.len() == 1 {
                            if let Err(error) =
                                self.attach_debugger_runtime(found.into_iter().next().unwrap(), ctx)
                            {
                                self.sessions.runtimes.error = error.to_string();
                            }
                        } else {
                            self.set_dialog(Dialog::Sessions);
                            if found.is_empty() {
                                self.sessions.runtimes.error =
                                    "No detached debugger matches this project policy".into();
                            }
                        }
                    }
                    if let Some(path) = self.sessions.runtimes.document.take() {
                        let policy = service::fingerprint(&self.session_plan());
                        let found: Vec<_> = self
                            .sessions
                            .runtimes
                            .items
                            .iter()
                            .filter(|i| {
                                i.kind == Kind::Notebook
                                    && i.document == path
                                    && i.policy == policy
                                    && !i.attached
                            })
                            .cloned()
                            .collect();
                        if found.len() == 1 {
                            self.sessions.runtimes.attaching = found.into_iter().next();
                        } else {
                            self.set_dialog(Dialog::Sessions);
                            if found.is_empty() {
                                self.sessions.runtimes.error =
                                    "No detached kernel matches this notebook and project policy"
                                        .into();
                            }
                        }
                    }
                }
                Ok(rode_session::Response::Ok) => {
                    self.sessions.runtimes.last = None;
                }
                Err(error) | Ok(rode_session::Response::Error(error)) => {
                    self.sessions.runtimes.error = error
                }
                _ => self.sessions.runtimes.error = "Unexpected runtime discovery reply".into(),
            }
        }
        if self.dialog == Dialog::Sessions
            && self
                .sessions
                .runtimes
                .last
                .is_none_or(|t| t.elapsed() > Duration::from_secs(3))
        {
            self.refresh_background_runtimes(ctx);
        }
        if let Some(info) = self.sessions.runtimes.attaching.clone() {
            if let Some(index) = self.documents.iter().position(|doc| {
                doc.path.as_deref() == Some(info.document.as_str()) && doc.notebook.is_some()
            }) {
                self.sessions.runtimes.attaching = None;
                match self.attach_notebook_runtime(index, info, ctx) {
                    Ok(()) => self.dialog = Dialog::None,
                    Err(error) => self.sessions.runtimes.error = error.to_string(),
                }
            } else if !self.opening.contains(&info.document) {
                self.sessions.runtimes.attaching = None;
                self.sessions.runtimes.error =
                    "Notebook could not be opened; retained kernel state was left untouched".into();
            }
        }
    }
    /// Render cached retained runtimes and capture explicit attachment/confirmed-end requests.
    pub(super) fn runtimes_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.separator();
        ui.heading("Notebook kernels & debuggers");
        ui.label("Persistent runtimes retain variables, queued cells, outputs and debugger state on this host. Attach from the original project and sandbox. End stops the runtime and discards its retained state.");
        if ui
            .add_enabled(
                self.sessions.runtimes.pending.is_none(),
                egui::Button::new("Refresh kernels and debuggers"),
            )
            .clicked()
        {
            self.refresh_background_runtimes(ctx);
        }
        if !self.sessions.runtimes.error.is_empty() {
            ui.colored_label(
                self.themes.active().palette.error,
                &self.sessions.runtimes.error,
            );
        }
        if let Some(endings) = &self.sessions.runtimes.endings
            && !endings.error.is_empty()
        {
            ui.colored_label(self.themes.active().palette.error, &endings.error);
        }
        if let Some(request) = &self.sessions.runtimes.confirm {
            let info = request.info.clone();
            ui.colored_label(self.themes.active().palette.error,format!("End '{}' and discard its retained state? Its kernel or launched debuggee will stop. Its Slurm allocation is released separately.",info.name));
            ui.horizontal(|ui| {
                if ui.button("Cancel end").clicked() {
                    self.sessions.runtimes.confirm = None;
                }
                if ui.button("Confirm end runtime").clicked() {
                    let request = self.sessions.runtimes.confirm.as_ref().unwrap().clone();
                    let wake = ctx.clone();
                    let endings =
                        self.sessions.runtimes.endings.get_or_insert_with(|| {
                            ending::Queue::new(move || wake.request_repaint())
                        });
                    // Keep the concrete confirmation available if the bounded
                    // queue cannot accept this approved mutation yet.
                    if let Err(error) = endings.push(request) {
                        endings.error = error.to_string();
                    } else {
                        self.sessions.runtimes.confirm = None;
                    }
                }
            });
        }
        let plan = self.session_plan();
        let policy = service::fingerprint(&plan);
        for info in self.sessions.runtimes.items.clone() {
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&info.name);
                    ui.small(if info.persistent { "PERSISTENT" } else { "UNTIL VIEW CLOSES" });
                    ui.label(&info.status);
                    let current = match info.kind {
                        Kind::Notebook => self.documents.iter().any(|d| {
                            d.notebook.as_ref().and_then(|v| v.native_id()) == Some(info.id.as_str())
                        }),
                        Kind::Debugger => self.debugger.client.as_ref().and_then(|c| c.native_id())
                            == Some(info.id.as_str()),
                    };
                    let compatible = info.root == plan.root && info.policy == policy;
                    let ending = self.sessions.runtimes.endings.as_ref().is_some_and(|q| q.contains(&info.id));
                    if ui.add_enabled(
                        !ending && compatible && (!info.attached || current),
                        egui::Button::new(if current { "Show" } else { "Attach runtime" }),
                    ).on_disabled_hover_text("Open the original project and policy; detach the controlling view in another window first").clicked() {
                        self.show_background_runtime(info.clone(), current, ctx);
                    }
                    if ui.add_enabled(compatible && !ending, egui::Button::new(if ending { "Ending…" } else { "End runtime" })).clicked() {
                        self.sessions.runtimes.confirm = Some(ending::Request { info: info.clone(), plan: plan.clone(), remote: self.session_remote() });
                    }
                });
                ui.small(format!("{} · {} · {}", info.mode, info.root, info.document));
            });
        }
    }
    fn show_background_runtime(&mut self, info: Info, current: bool, ctx: &egui::Context) {
        if info.kind == Kind::Notebook {
            self.open_file(info.document.clone(), ctx);
            if let Some(index) = self
                .documents
                .iter()
                .position(|d| d.path.as_deref() == Some(&info.document) && d.notebook.is_some())
            {
                if current {
                    self.active = index;
                    self.focus_editor(ctx);
                } else if let Err(error) = self.attach_notebook_runtime(index, info, ctx) {
                    self.sessions.runtimes.error = error.to_string();
                    return;
                }
                self.dialog = Dialog::None;
            } else {
                self.sessions.runtimes.attaching = Some(info);
            }
        } else if current {
            self.dialog = Dialog::None;
            self.workbench.panel = Some(Panel::Debug);
        } else {
            match self.attach_debugger_runtime(info, ctx) {
                Ok(()) => self.dialog = Dialog::None,
                Err(error) => self.sessions.runtimes.error = error.to_string(),
            }
        }
    }
}

//! Window-owned host selections and completions. Project changes cannot retarget
//! telemetry or widen a transfer policy. SSH cleanup leaves a visibly offline selection.
use super::*;

struct TransferHost {
    endpoint: crate::transfers::Endpoint,
    /// Project policies expire with project authority; explicit host policies do not.
    project_generation: Option<u64>,
}
enum Reply {
    Monitor {
        generation: u64,
        result: Result<crate::project::Response, String>,
    },
    Prepared {
        generation: u64,
        remote: Box<Remote>,
        result: Result<sandbox::Plan, String>,
    },
}
pub(super) struct State {
    /// Live host telemetry follows this window's host selection, never project resets.
    pub monitoring: monitoring::State,
    monitor_initialized: bool,
    monitor_remote: Option<Remote>,
    monitor_generation: u64,
    transfer_initialized: bool,
    transfer: Option<TransferHost>,
    transfer_remote: Option<Remote>,
    transfer_generation: u64,
    pub preparing: bool,
    error: Option<String>,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
}
impl Default for State {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            monitoring: Default::default(),
            monitor_initialized: false,
            monitor_remote: None,
            monitor_generation: 0,
            transfer_initialized: false,
            transfer: None,
            transfer_remote: None,
            transfer_generation: 0,
            preparing: false,
            error: None,
            tx,
            rx,
        }
    }
}
impl State {
    /// Stop collection while retaining cached readings and ownership of an in-flight reply.
    pub fn stop_monitor(&mut self) {
        self.monitoring.set_active(false);
    }
    /// Reset host readings and Start intent without losing an outstanding reply.
    pub fn change_monitor_host(&mut self) {
        self.monitoring.set_active(false);
        self.monitoring.reset_history();
    }
    /// Release the old request slot without admitting stale telemetry.
    pub fn discard_monitor_completion(&mut self) {
        self.monitoring.monitor_busy = false;
        self.monitoring.discard_reply();
    }
}

impl EditorApp {
    fn selected_host_connected(&self, remote: &Remote) -> bool {
        self.workspace
            .as_ref()
            .filter(|workspace| {
                workspace
                    .remote
                    .as_ref()
                    .is_some_and(|current| current.socket == remote.socket)
            })
            .map_or_else(|| remote.connected(), |workspace| workspace.connected)
    }
    /// Capture each tool’s initial host once; later project changes cannot retarget it.
    pub(super) fn initialize_host_tool(&mut self, panel: Panel) {
        match panel {
            Panel::Monitor if !self.host_tools.monitor_initialized => {
                self.migrate_monitor_preferences();
                self.host_tools.monitor_remote =
                    self.workspace.as_ref().and_then(|w| w.remote.clone());
                self.host_tools.monitor_initialized = true;
            }
            Panel::Transfers if !self.host_tools.transfer_initialized => {
                self.host_tools.transfer_initialized = true;
                self.bind_project_transfers();
            }
            _ => {}
        }
    }
    pub(super) fn bind_project_transfers(&mut self) {
        if !self.allow_shared_folder_change(transfers::ContextChange::Access) {
            return;
        }
        self.host_tools.transfer_initialized = true;
        if let Some(workspace) = self.workspace.as_ref().filter(|w| w.ready && w.connected)
            && let Some(remote) = &workspace.remote
        {
            let host = TransferHost {
                endpoint: crate::transfers::Endpoint::ssh(remote.clone(), workspace.plan.clone()),
                project_generation: Some(self.workers.generation()),
            };
            self.transfers.reset();
            self.host_tools.transfer_remote = host.endpoint.remote().cloned();
            self.host_tools.transfer = Some(host);
            self.host_tools.error = None;
            self.host_tools.transfer_generation =
                self.host_tools.transfer_generation.wrapping_add(1);
        }
    }
    /// A project-scoped snapshot must be explicitly replaced after its authority expires.
    pub(super) fn invalidate_project_transfers(&mut self) {
        if self
            .host_tools
            .transfer
            .as_ref()
            .is_none_or(|host| host.project_generation.is_some())
        {
            self.transfers.reset();
        }
    }
    pub(super) fn transfer_host_endpoint(&self) -> Option<crate::transfers::Endpoint> {
        let host = self.host_tools.transfer.as_ref()?;
        if self.vaults.locking()
            || host
                .project_generation
                .is_some_and(|generation| generation != self.workers.generation())
        {
            return None;
        }
        host.endpoint
            .remote()
            .filter(|remote| self.selected_host_connected(remote))?;
        Some(host.endpoint.clone())
    }
    pub(super) fn transfer_host_root(&self) -> Option<String> {
        self.transfer_host_endpoint()?
            .plan()
            .map(|plan| plan.root.clone())
    }
    pub(super) fn transfer_host_label(&self) -> String {
        self.host_tools
            .transfer_remote
            .as_ref()
            .map(|remote| remote.profile.endpoint())
            .unwrap_or_else(|| "Choose an SSH connection".into())
    }
    pub(super) fn monitor_host_label(&self) -> String {
        self.host_tools
            .monitor_remote
            .as_ref()
            .map(|remote| remote.profile.endpoint())
            .unwrap_or_else(|| "This computer".into())
    }
    pub(super) fn monitor_host_ready(&self) -> bool {
        if !self.host_tools.monitor_initialized {
            return self
                .workspace
                .as_ref()
                .is_none_or(|w| w.remote.is_none() || w.connected);
        }
        self.host_tools
            .monitor_remote
            .as_ref()
            .is_none_or(|remote| self.selected_host_connected(remote))
    }
    fn select_monitor_host(&mut self, remote: Option<Remote>) {
        self.host_tools.monitor_initialized = true;
        self.host_tools.monitor_remote = remote;
        self.host_tools.monitor_generation = self.host_tools.monitor_generation.wrapping_add(1);
        self.host_tools.change_monitor_host();
    }
    /// Capture one read-only host request independently of the project reply generation.
    pub(super) fn dispatch_host_monitor(&self, ctx: &egui::Context) {
        let remote = self.host_tools.monitor_remote.clone();
        let generation = self.host_tools.monitor_generation;
        let tx = self.host_tools.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let request = Request::Tool {
                request: crate::project::Request::Monitor { root: "/".into() },
            };
            let result = match remote {
                Some(remote) => remote.request_in(&request, None),
                None => files::local(&request),
            }
            .and_then(|reply| {
                reply
                    .tool
                    .ok_or_else(|| anyhow::anyhow!("Host returned no monitor result"))
            })
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Reply::Monitor { generation, result });
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        });
    }
    fn prepare_transfer_host(&mut self, remote: Remote, ctx: &egui::Context) {
        if self.host_tools.preparing
            || !self.allow_shared_folder_change(transfers::ContextChange::Access)
        {
            return;
        }
        self.transfers.reset();
        self.host_tools.transfer_remote = Some(remote.clone());
        self.host_tools.transfer = None;
        self.host_tools.transfer_generation = self.host_tools.transfer_generation.wrapping_add(1);
        let generation = self.host_tools.transfer_generation;
        self.host_tools.preparing = true;
        self.host_tools.error = None;
        let tx = self.host_tools.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = remote
                .request_in(
                    &Request::Prepare {
                        path: "~".into(),
                        sandbox: Default::default(),
                    },
                    None,
                )
                .and_then(|reply| {
                    reply
                        .project
                        .ok_or_else(|| anyhow::anyhow!("Host returned no file access context"))
                })
                .and_then(|plan| {
                    anyhow::ensure!(
                        !plan.active && !plan.requested && plan.prefix.is_empty(),
                        "Host access context was not applied"
                    );
                    Ok(plan)
                })
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Reply::Prepared {
                generation,
                remote: Box::new(remote),
                result,
            });
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        });
    }
    pub(super) fn poll_host_tools(&mut self, ctx: &egui::Context) {
        while let Ok(reply) = self.host_tools.rx.try_recv() {
            match reply {
                Reply::Monitor { generation, result } => {
                    if generation == self.host_tools.monitor_generation {
                        self.complete_host_monitor(result, ctx);
                    } else {
                        self.host_tools.discard_monitor_completion();
                    }
                }
                Reply::Prepared {
                    generation,
                    remote,
                    result,
                } => {
                    self.host_tools.preparing = false;
                    if generation != self.host_tools.transfer_generation {
                        continue;
                    }
                    match result {
                        Ok(plan) => {
                            self.host_tools.transfer = Some(TransferHost {
                                endpoint: crate::transfers::Endpoint::ssh(*remote, plan),
                                project_generation: None,
                            })
                        }
                        Err(error) => self.host_tools.error = Some(error),
                    }
                }
            }
        }
        if !self.monitor_host_ready() {
            self.host_tools.stop_monitor();
        }
        if self
            .host_tools
            .transfer
            .as_ref()
            .and_then(|h| h.endpoint.remote())
            .is_some_and(|r| !r.connected())
            && self.transfers.pending()
        {
            self.transfers.reset();
        }
    }
    /// Host choices are explicit. A disconnected or expired selection never falls back locally.
    pub(super) fn host_tool_header(
        &mut self,
        panel: Panel,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        self.initialize_host_tool(panel);
        let remote = self
            .workspace
            .as_ref()
            .filter(|w| w.connected)
            .and_then(|w| w.remote.clone());
        ui.horizontal_wrapped(|ui| {
            ui.weak("Host");
            let label = if panel == Panel::Monitor {
                self.monitor_host_label()
            } else {
                self.transfer_host_label()
            };
            let selector = crate::keyboard::ComboBox::new(("host-tool", panel), "Host selection")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    if panel == Panel::Monitor {
                        let local = crate::keyboard::button(ui, "This computer");
                        ui_probe::button(ui, "monitor-host-local", &local);
                        if local.clicked() {
                            self.select_monitor_host(None);
                            ui.close();
                        }
                    }
                    if let Some(remote) = &remote {
                        let choose = ui
                            .add_enabled(
                                panel != Panel::Transfers
                                    || self
                                        .transfers
                                        .sharing
                                        .admit(transfers::ContextChange::Host)
                                        .is_ok(),
                                crate::keyboard::Button::new(format!(
                                    "SSH · {}",
                                    remote.profile.endpoint()
                                )),
                            )
                            .on_disabled_hover_text(
                                "Unmount shared folders before changing the host.",
                            );
                        ui_probe::button(
                            ui,
                            if panel == Panel::Monitor {
                                "monitor-host-ssh"
                            } else {
                                "transfer-host-ssh"
                            },
                            &choose,
                        );
                        if choose.clicked() {
                            if panel == Panel::Monitor {
                                self.select_monitor_host(Some(remote.clone()));
                            } else if self
                                .allow_shared_folder_change(transfers::ContextChange::Host)
                            {
                                self.transfers.reset();
                                self.host_tools.transfer_remote = Some(remote.clone());
                                self.host_tools.transfer = None;
                                self.host_tools.transfer_generation =
                                    self.host_tools.transfer_generation.wrapping_add(1);
                                self.bind_project_transfers();
                            }
                            ui.close();
                        }
                    }
                    if crate::keyboard::button(ui, "Manage connections…").clicked() {
                        self.open_area(areas::Area::Connections, ctx);
                        ui.close();
                    }
                });
            ui_probe::button(ui, &format!("host-selector:{panel:?}"), &selector.response);
            if panel == Panel::Transfers {
                let selected = self.host_tools.transfer_remote.clone();
                let same_project_host = selected
                    .as_ref()
                    .zip(remote.as_ref())
                    .is_some_and(|(selected, current)| selected.socket == current.socket);
                let access = crate::keyboard::menu_button(ui, "Access", |ui| {
                    let project = ui.add_enabled(
                        self.project_ready()
                            && same_project_host
                            && self
                                .transfers
                                .sharing
                                .admit(transfers::ContextChange::Access)
                                .is_ok(),
                        crate::keyboard::Button::new("Use current project permissions"),
                    );
                    ui_probe::button(ui, "transfer-use-project-access", &project);
                    if project.clicked() {
                        self.bind_project_transfers();
                        ui.close();
                    }
                    let host = ui.add_enabled(
                        selected
                            .as_ref()
                            .is_some_and(|remote| self.selected_host_connected(remote))
                            && !self.host_tools.preparing
                            && self
                                .transfers
                                .sharing
                                .admit(transfers::ContextChange::Access)
                                .is_ok(),
                        crate::keyboard::Button::new("Use host access (outside project sandbox)"),
                    );
                    ui_probe::button(ui, "transfer-use-host-access", &host);
                    if host.clicked()
                        && let Some(remote) = &selected
                    {
                        self.prepare_transfer_host(remote.clone(), ctx);
                        ui.close();
                    }
                });
                ui_probe::button(ui, "transfer-access-menu", &access.response);
                if let Some(host) = &self.host_tools.transfer {
                    let scope = ui.weak(if host.project_generation.is_some() {
                        "Project permissions"
                    } else {
                        "Host access"
                    });
                    ui_probe::button(
                        ui,
                        if host.project_generation.is_some() {
                            "transfer-scope-project"
                        } else {
                            "transfer-scope-host"
                        },
                        &scope,
                    );
                    if host
                        .project_generation
                        .is_some_and(|generation| generation != self.workers.generation())
                    {
                        ui.colored_label(
                            self.themes.active().palette.error,
                            "Project permissions expired; choose an access scope.",
                        );
                    }
                }
                if self.host_tools.preparing {
                    let spinner = ui.spinner();
                    ui_probe::button(ui, "transfer-host-preparing", &spinner);
                } else if selected
                    .as_ref()
                    .is_some_and(|r| !self.selected_host_connected(r))
                {
                    ui.colored_label(
                        self.themes.active().palette.accent,
                        "Disconnected · reconnect or choose another host.",
                    );
                } else if selected.is_some() && self.host_tools.transfer.is_none() {
                    ui.weak("Choose an access scope to browse this host.");
                }
                if let Some(error) = &self.host_tools.error {
                    ui.colored_label(self.themes.active().palette.error, error);
                }
            }
        });
        ui.add_space(6.);
    }
}

#[cfg(test)]
mod tests;

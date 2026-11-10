//! Explicit host administration for daemon-owned reverse SSH tunnels.
//! Workers use the local service or the workspace's authenticated host connection,
//! outside the project sandbox. Remote failures never fall back to the local host.
//! Closing this dialog or window never stops a tunnel.
use super::*;
use crate::project;
use anyhow::Context;
use rode_session::{
    Request as Control,
    tunnels::{Info, ListenOn, Spec, Status},
};

mod saved;
mod ui;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Hash)]
enum Page {
    #[default]
    Tunnels,
    Saved,
    New,
}

enum Outcome {
    Listed(Vec<Info>),
    Route(Box<Profile>),
}

/// Capture the selected host before dispatching work. An offline remote workspace
/// is an error, not permission to administer the desktop's service instead.
enum Host {
    Local,
    Remote { remote: Box<Remote>, root: String },
}
impl Host {
    fn for_workspace(workspace: Option<&Workspace>) -> anyhow::Result<Self> {
        if let Some(workspace) = workspace
            && let Some(remote) = &workspace.remote
        {
            anyhow::ensure!(
                workspace.connected,
                "Reconnect SSH to manage this host's tunnels"
            );
            return Ok(Self::Remote {
                remote: Box::new(remote.clone()),
                root: workspace.root.clone(),
            });
        }
        Ok(Self::Local)
    }

    fn request(&self, request: Control) -> anyhow::Result<rode_session::Response> {
        let scope = match &request {
            Control::StartTunnel { spec, .. } => Some(spec.listen_on),
            _ => None,
        };
        let action = match &request {
            Control::StartTunnel { .. } => "start SSH tunnel",
            Control::StopTunnel { .. } => "stop SSH tunnel",
            _ => "list SSH tunnels",
        };
        self.dispatch(request)
            .and_then(|response| {
                if let Some(scope) = scope {
                    check_started_scope(&response, scope)?;
                }
                Ok(response)
            })
            .with_context(|| match self {
                Self::Local => format!("Cannot {action} on this computer"),
                Self::Remote { remote, .. } => {
                    format!("Cannot {action} on SSH host {}", remote.profile.endpoint())
                }
            })
    }

    fn dispatch(&self, request: Control) -> anyhow::Result<rode_session::Response> {
        let operation = crate::sessions::Operation::Tunnels {
            request: Box::new(request),
        };
        let reply = match self {
            Self::Local => crate::sessions::local(&operation)?,
            Self::Remote { remote, root } => {
                let response = remote
                    .request_in(
                        &Request::Tool {
                            request: project::Request::Sessions {
                                root: root.clone(),
                                operation,
                                directory: std::env::var("RODE_SESSION_DIR").ok(),
                            },
                        },
                        None,
                    )?
                    .tool
                    .context("Missing tunnel response")?;
                let project::Response::Sessions(reply) = response else {
                    anyhow::bail!("Unexpected tunnel response");
                };
                reply
            }
        };
        reply.response.checked()
    }
}

fn check_started_scope(
    response: &rode_session::Response,
    requested: ListenOn,
) -> anyhow::Result<()> {
    let rode_session::Response::Tunnel(info) = response else {
        anyhow::bail!("Unexpected tunnel start response");
    };
    anyhow::ensure!(
        info.spec.listen_on == requested,
        "The running tunnel service did not apply the requested relay access. Update/restart that service before retrying; its existing jobs were not stopped."
    );
    Ok(())
}
/// Window-owned tunnel manager with captured-host generations and separate discovery/action workers.
/// Saved configurations outlive a workspace reset; stale observations do not imply a live connection.
#[derive(Default)]
pub(super) struct Tunnels {
    saved: saved::Library,
    saved_id: Option<String>,
    delete_saved: Option<String>,
    generation: Option<u64>,
    page: Page,
    draft_epoch: u64,
    spec: Spec,
    available: Vec<Info>,
    pending: Option<mpsc::Receiver<Result<Outcome, String>>>,
    /// Read-only background discovery never occupies the manager's action slot.
    background: Option<mpsc::Receiver<Result<Vec<Info>, String>>>,
    /// None until checked; a failed check must not advertise a stale connected count.
    observation: Option<Result<(), String>>,
    last: Option<Instant>,
    error: String,
    relay_jump: String,
    stop: Option<String>,
}
impl Tunnels {
    fn relay_jump(&self, info: &Info) -> String {
        if !self.relay_jump.trim().is_empty() {
            return self.relay_jump.trim().to_owned();
        }
        let host = &info.spec.relay_host;
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.clone()
        };
        format!("{}@{}:{}", info.spec.relay_user, host, info.spec.relay_port)
    }

    fn connection_details(&self, info: &Info) -> String {
        let via_relay = format!(
            "In Connect SSH on the other computer:\nHost: 127.0.0.1\nPort: {}\nJump / ProxyJump: {}\nUser: your login on the computer running the tunnel\n\nUse that computer's SSH credentials and verify its host key before accepting it.\n",
            info.spec.listen_port,
            self.relay_jump(info)
        );
        if info.spec.listen_on == ListenOn::AllInterfaces {
            format!(
                "Direct connection, if the relay's SSH policy and firewall allow access:\nHost: {}\nPort: {}\nJump / ProxyJump: leave blank\nUser: your login on the computer running the tunnel\n\nVerify the tunnel host's SSH key, independently of the relay's SSH key.\n\nAlternatively, through the relay's SSH connection:\n{via_relay}",
                info.spec.relay_host, info.spec.listen_port
            )
        } else {
            via_relay
        }
    }
}
impl EditorApp {
    fn tunnel_status_blocked(&self) -> Option<&'static str> {
        if self.vaults.locking {
            return Some("Encrypted project is locking. Tunnel status checks are paused.");
        }
        let workspace = self.workspace.as_ref().filter(|w| w.remote.is_some())?;
        if !workspace.connected {
            Some("SSH is offline. Reconnect to check this host's tunnels.")
        } else if !workspace.ready {
            Some("Waiting for SSH host preparation before checking tunnels.")
        } else {
            None
        }
    }

    /// Poll tunnel workers and schedule bounded discovery without blocking the UI.
    /// A workspace generation change resets live state while retaining saved configurations.
    pub(super) fn tunnel_tick(&mut self, ctx: &egui::Context) {
        if self.tunnels.generation != Some(self.generation) {
            self.tunnels = Tunnels {
                generation: Some(self.generation),
                saved: std::mem::take(&mut self.tunnels.saved),
                ..Default::default()
            };
        }
        if let Some(result) = self
            .tunnels
            .background
            .as_ref()
            .and_then(|rx| match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Tunnel status worker ended without a response".into()))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            })
        {
            self.tunnels.background = None;
            self.tunnels.last = Some(Instant::now());
            self.tunnels.observation = Some(result.map(|list| self.tunnels.available = list));
        }
        if let Some(result) = self
            .tunnels
            .pending
            .as_ref()
            .and_then(|rx| match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Tunnel worker ended without a response".into()))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            })
        {
            self.tunnels.pending = None;
            self.tunnels.last = Some(Instant::now());
            match result {
                Ok(Outcome::Listed(list)) => {
                    self.tunnels.available = list;
                    self.tunnels.observation = Some(Ok(()));
                }
                Ok(Outcome::Route(profile))
                    if self.dialog == Dialog::Tunnels && self.tunnels.page == Page::Tunnels =>
                {
                    self.profile = *profile;
                    self.profile_baseline = None;
                    self.profile_form += 1;
                    self.set_dialog(Dialog::Connect);
                }
                Ok(Outcome::Route(_)) => {}
                Err(error) => {
                    self.tunnels.observation = Some(Err(error.clone()));
                    self.tunnels.error = error;
                }
            }
        }
        if self.tunnel_status_blocked().is_some() {
            self.tunnels.background = None;
            self.tunnels.observation = None;
            self.tunnels.last = None;
            return;
        }
        let manager = self.dialog == Dialog::Tunnels && self.tunnels.page == Page::Tunnels;
        let interval = Duration::from_secs(if manager { 2 } else { 15 });
        if self.tunnels.pending.is_none()
            && self.tunnels.background.is_none()
            && self
                .tunnels
                .last
                .is_none_or(|last| last.elapsed() >= interval)
        {
            if manager {
                self.tunnel_request(Control::Tunnels, ctx);
            } else {
                self.refresh_tunnel_status(ctx);
            }
        }
        if self.tunnels.pending.is_some() || self.tunnels.background.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        } else {
            ctx.request_repaint_after(
                interval.saturating_sub(
                    self.tunnels
                        .last
                        .map_or(Duration::ZERO, |last| last.elapsed()),
                ),
            );
        }
    }

    /// Discover retained workers on the current host without starting a service,
    /// opening a new SSH connection or disabling setup and lifetime actions.
    fn refresh_tunnel_status(&mut self, ctx: &egui::Context) {
        let host = match Host::for_workspace(self.workspace.as_ref()) {
            Ok(host) => host,
            Err(error) => {
                self.tunnels.observation = Some(Err(error.to_string()));
                self.tunnels.last = Some(Instant::now());
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        self.tunnels.background = Some(rx);
        self.tunnels.last = Some(Instant::now());
        let ctx = ctx.clone();
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let _ = tx.send(list_tunnels(&host).map_err(|error| format!("{error:#}")));
            ctx.request_repaint();
        });
    }

    fn tunnel_request(&mut self, request: Control, ctx: &egui::Context) {
        if self.tunnels.pending.is_some() {
            return;
        }
        let host = match Host::for_workspace(self.workspace.as_ref()) {
            Ok(host) => host,
            Err(error) => {
                self.tunnels.error = error.to_string();
                self.tunnels.last = Some(Instant::now());
                return;
            }
        };
        // A status reply captured before Start/Stop must not overwrite its result.
        self.tunnels.background = None;
        let (tx, rx) = mpsc::channel();
        self.tunnels.pending = Some(rx);
        self.tunnels.last = Some(Instant::now());
        if !matches!(request, Control::Tunnels) {
            self.tunnels.error.clear();
            self.tunnels.observation = None;
        }
        let ctx = ctx.clone();
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let result = (|| -> anyhow::Result<Outcome> {
                if !matches!(request, Control::Tunnels) {
                    host.request(request)?;
                }
                list_tunnels(&host).map(Outcome::Listed)
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn tunnel_route(&mut self, info: &Info, ctx: &egui::Context) {
        let Some(remote) = self.workspace.as_ref().and_then(|w| w.remote.as_ref()) else {
            return;
        };
        let mut profile = remote.profile.clone();
        // Retain the current folder and explicit project policy in the new profile.
        if let Some(workspace) = &self.workspace {
            profile.root = workspace.root.clone();
            profile.sandbox = workspace.sandbox_options.clone();
        }
        let jump = self.tunnels.relay_jump(info);
        let port = info.spec.listen_port;
        let (tx, rx) = mpsc::channel();
        self.tunnels.pending = Some(rx);
        let ctx = ctx.clone();
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let result = profile
                .through_tunnel(jump, port)
                .map(|profile| Outcome::Route(Box::new(profile)))
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}

fn list_tunnels(host: &Host) -> anyhow::Result<Vec<Info>> {
    match host.request(Control::Tunnels)? {
        rode_session::Response::Tunnels(list) => Ok(list),
        _ => anyhow::bail!("Unexpected tunnel listing"),
    }
}

#[cfg(test)]
mod tests;

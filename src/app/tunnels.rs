//! Explicit host administration for daemon-owned reverse SSH tunnels.
//! Workers use the local service or the workspace's authenticated host connection,
//! outside the project sandbox. Remote failures never fall back to the local host.
//! Closing this dialog or window never stops a tunnel.
use super::*;
use crate::project;
use anyhow::Context;
use rode_session::{
    Request as Control,
    tunnels::{Info, Spec, Status},
};

mod ui;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Hash)]
enum Page {
    #[default]
    Tunnels,
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
        let action = match &request {
            Control::StartTunnel { .. } => "start SSH tunnel",
            Control::StopTunnel { .. } => "stop SSH tunnel",
            _ => "list SSH tunnels",
        };
        self.dispatch(request).with_context(|| match self {
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
#[derive(Default)]
pub(super) struct Tunnels {
    generation: Option<u64>,
    page: Page,
    draft_epoch: u64,
    spec: Spec,
    available: Vec<Info>,
    pending: Option<mpsc::Receiver<Result<Outcome, String>>>,
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
        format!(
            "In Connect SSH on the other computer:\nHost: 127.0.0.1\nPort: {}\nJump / ProxyJump: {}\nUser: your login on the computer running the tunnel\n\nUse that computer's SSH credentials and verify its host key before accepting it.\n",
            info.spec.listen_port,
            self.relay_jump(info)
        )
    }
}
impl EditorApp {
    pub(super) fn tunnel_tick(&mut self, ctx: &egui::Context) {
        if self.tunnels.generation != Some(self.generation) {
            self.tunnels = Tunnels {
                generation: Some(self.generation),
                ..Default::default()
            };
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
                Ok(Outcome::Listed(list)) => self.tunnels.available = list,
                Ok(Outcome::Route(profile))
                    if self.dialog == Dialog::Tunnels && self.tunnels.page == Page::Tunnels =>
                {
                    self.profile = *profile;
                    self.profile_baseline = None;
                    self.profile_form += 1;
                    self.set_dialog(Dialog::Connect);
                }
                Ok(Outcome::Route(_)) => {}
                Err(error) => self.tunnels.error = error,
            }
        }
        if self.dialog == Dialog::Tunnels
            && self.tunnels.page == Page::Tunnels
            && self.tunnels.pending.is_none()
            && self
                .tunnels
                .last
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(2))
        {
            self.tunnel_request(Control::Tunnels, ctx);
        }
        if self.dialog == Dialog::Tunnels && self.tunnels.page == Page::Tunnels {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
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
        let (tx, rx) = mpsc::channel();
        self.tunnels.pending = Some(rx);
        self.tunnels.last = Some(Instant::now());
        if !matches!(request, Control::Tunnels) {
            self.tunnels.error.clear();
        }
        let ctx = ctx.clone();
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let result = (|| -> anyhow::Result<Outcome> {
                if !matches!(request, Control::Tunnels) {
                    host.request(request)?;
                }
                match host.request(Control::Tunnels)? {
                    rode_session::Response::Tunnels(list) => Ok(Outcome::Listed(list)),
                    _ => anyhow::bail!("Unexpected tunnel listing"),
                }
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

#[cfg(test)]
mod tests;

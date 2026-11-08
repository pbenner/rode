//! Explicit host administration for daemon-owned reverse SSH tunnels.
//! Workers use the authenticated host connection; no project sandbox or local
//! fallback is allowed. Closing this dialog or window never stops a tunnel.
use super::*;
use crate::project;
use anyhow::Context;
use rode_session::{
    Request as Control,
    tunnels::{Info, Spec, Status},
};

enum Outcome {
    Listed(Vec<Info>),
    Route(Box<Profile>),
}
#[derive(Default)]
pub(super) struct Tunnels {
    generation: Option<u64>,
    spec: Spec,
    available: Vec<Info>,
    pending: Option<mpsc::Receiver<Result<Outcome, String>>>,
    last: Option<Instant>,
    error: String,
    relay_jump: String,
    stop: Option<String>,
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
                Ok(Outcome::Route(profile)) if self.dialog == Dialog::Tunnels => {
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
            && self.tunnels.pending.is_none()
            && self
                .tunnels
                .last
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(2))
        {
            self.tunnel_request(Control::Tunnels, ctx);
        }
        if self.dialog == Dialog::Tunnels {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
    fn tunnel_request(&mut self, request: Control, ctx: &egui::Context) {
        if self.tunnels.pending.is_some() {
            return;
        }
        let Some(workspace) = self.workspace.as_ref().filter(|w| w.connected) else {
            return;
        };
        let Some(remote) = workspace.remote.clone() else {
            return;
        };
        let root = workspace.root.clone();
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
                let dispatch = |request: Control| -> anyhow::Result<rode_session::Response> {
                    let response = remote
                        .request_in(
                            &Request::Tool {
                                request: project::Request::Sessions {
                                    root: root.clone(),
                                    operation: crate::sessions::Operation::Tunnels {
                                        request: Box::new(request),
                                    },
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
                    reply.response.checked()
                };
                if !matches!(request, Control::Tunnels) {
                    dispatch(request)?;
                }
                match dispatch(Control::Tunnels)? {
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
        let jump = if self.tunnels.relay_jump.trim().is_empty() {
            let host = &info.spec.relay_host;
            let host = if host.contains(':') {
                format!("[{host}]")
            } else {
                host.clone()
            };
            format!("{}@{}:{}", info.spec.relay_user, host, info.spec.relay_port)
        } else {
            self.tunnels.relay_jump.trim().to_owned()
        };
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
    pub(super) fn tunnels_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("SSH tunnels");
        ui.label("The compute host’s session service maintains these tunnels after RODE disconnects. They end when stopped or when the service/node ends.");
        let connected = self
            .workspace
            .as_ref()
            .is_some_and(|w| w.connected && w.remote.is_some());
        if !connected {
            ui.label("Connect to the compute host first to manage its tunnels.");
            return;
        }
        ui.label("Host administration · outside the project sandbox");
        if !self.tunnels.error.is_empty() {
            ui.colored_label(self.themes.active().palette.error, &self.tunnels.error);
        }
        let busy = self.tunnels.pending.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            let mut action = None;
            let mut route = None;
            for info in self.tunnels.available.clone() {
                ui.push_id(&info.id, |ui| {
                    ui.separator();
                    ui.strong(format!("{} — {}", info.spec.name, info.status.label()));
                    ui.label(format!("{}@{}:{} · relay 127.0.0.1:{} → compute 127.0.0.1:{}",
                        info.spec.relay_user, info.spec.relay_host, info.spec.relay_port, info.spec.listen_port, info.spec.ssh_port));
                    if !info.error.is_empty() { ui.label(&info.error); }
                    if info.status == Status::HostKeyRequired {
                        ui.label("Verify this fingerprint through a trusted source before accepting it:");
                        ui.monospace(&info.fingerprint);
                        if ui.button(if info.spec.relay_key.is_empty() { "Trust relay key and start" } else { "Replace trusted relay key and restart" }).clicked() {
                            let mut spec = info.spec.clone();
                            spec.relay_key = info.observed_key.clone();
                            action = Some(Control::StartTunnel { id: info.id.clone(), spec });
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(info.status == Status::Connected, egui::Button::new("Configure return route")).clicked() { route = Some(info.clone()); }
                        if info.status == Status::Attention && ui.button("Retry").clicked() {
                            action = Some(Control::StartTunnel { id: info.id.clone(), spec: info.spec.clone() });
                        }
                        if ui.button("Copy settings").clicked() {
                            self.tunnels.spec = info.spec.clone();
                        }
                        if ui.button("Stop…").clicked() { self.tunnels.stop = Some(info.id.clone()); }
                    });
                    if self.tunnels.stop.as_deref() == Some(&info.id) {
                        ui.label("Stopping closes all connections through this tunnel, including this workspace if it uses this route.");
                        ui.horizontal(|ui| {
                            if ui.button("Stop tunnel").clicked() {
                                action = Some(Control::StopTunnel { id: info.id.clone() });
                                self.tunnels.stop = None;
                            }
                            if ui.button("Cancel").clicked() { self.tunnels.stop = None; }
                        });
                    }
                });
            }
            if let Some(action) = action { self.tunnel_request(action, ctx); }
            else if let Some(info) = route { self.tunnel_route(&info, ctx); }
        });
        ui.separator();
        ui.label("Desktop relay alias / jump chain (optional)");
        ui.add(
            egui::TextEdit::singleline(&mut self.tunnels.relay_jump)
                .hint_text("Default: relay username, hostname and SSH port above"),
        );
        ui.small("Configure return route opens Connect SSH. Save that profile before leaving the direct connection. Desktop relay authentication uses your desktop SSH configuration.");
        ui.collapsing("Start a new tunnel", |ui| {
            ui.add_enabled_ui(self.tunnels.pending.is_none(), |ui| {
                ui.label("These paths and credentials belong to the compute host. Use an unencrypted identity file or an explicitly selected persistent agent. Forwarded desktop agents cannot provide unattended recovery.");
                let spec = &mut self.tunnels.spec;
                for (label, value) in [("Name", &mut spec.name), ("Relay hostname / IP", &mut spec.relay_host), ("Relay username", &mut spec.relay_user), ("Compute identity file", &mut spec.identity), ("Compute persistent agent socket (optional)", &mut spec.agent_socket)] {
                    ui.label(label);
                    ui.add(egui::TextEdit::singleline(value).id_salt(("tunnel-field", label)).char_limit(4096).desired_width(f32::INFINITY));
                }
                for (label, value) in [("Relay SSH port", &mut spec.relay_port), ("Relay loopback port", &mut spec.listen_port), ("Compute SSH port", &mut spec.ssh_port)] {
                    ui.horizontal(|ui| { ui.label(label); ui.add(egui::DragValue::new(value).range(1..=65535)); });
                }
                if !spec.relay_key.is_empty() { ui.small("Copied settings retain the verified relay key."); }
                let validation = spec.validate();
                if let Err(error) = &validation { ui.small(error.to_string()); }
                if ui.add_enabled(validation.is_ok(), egui::Button::new("Start tunnel")).clicked() {
                    let spec = spec.clone();
                    match rode_session::new_id() {
                        Ok(id) => self.tunnel_request(Control::StartTunnel { id, spec }, ctx),
                        Err(error) => self.tunnels.error = error.to_string(),
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_route_result_cannot_reopen_a_dismissed_dialog() {
        for dialog in [Dialog::None, Dialog::Folder] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            let (tx, rx) = mpsc::channel();
            app.profile.host = "current-compute".into();
            app.tunnels.generation = Some(app.generation);
            app.tunnels.pending = Some(rx);
            app.dialog = dialog;
            let expected = std::mem::discriminant(&app.dialog);
            tx.send(Ok(Outcome::Route(Box::new(Profile {
                host: "late-route".into(),
                ..Default::default()
            }))))
            .unwrap();
            app.tunnel_tick(&ctx);
            assert_eq!(app.profile.host, "current-compute");
            assert_eq!(std::mem::discriminant(&app.dialog), expected);
            assert!(app.tunnels.pending.is_none());
        }
    }

    #[test]
    fn tunnel_results_from_an_old_workspace_cannot_replace_the_new_profile() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let (tx, rx) = mpsc::channel();
        app.profile.host = "current-compute".into();
        app.tunnels.generation = Some(app.generation);
        app.tunnels.pending = Some(rx);
        tx.send(Ok(Outcome::Route(Box::new(Profile {
            host: "old-compute".into(),
            ..Default::default()
        }))))
        .unwrap();
        app.generation += 1;
        app.tunnel_tick(&ctx);
        assert_eq!(app.profile.host, "current-compute");
        assert!(app.tunnels.pending.is_none());
        assert!(app.dialog == Dialog::None);
    }

    #[test]
    fn tunnel_controls_render_without_starting_workers_and_hidden_manager_does_not_poll() {
        for width in [330., 500., 680.] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.workspace = Some(crate::app::tests::remote_workspace());
            app.tunnel_tick(&ctx);
            assert!(
                app.tunnels.pending.is_none(),
                "Hidden manager must not poll"
            );
            app.tunnels.available = vec![Info {
                id: "retained".into(),
                spec: Spec {
                    relay_host: "relay.example".into(),
                    relay_user: "compute".into(),
                    ..Default::default()
                },
                status: Status::HostKeyRequired,
                error: "Verify the relay before trusting its host key".into(),
                observed_key: String::new(),
                fingerprint: "SHA256:abcdefghijklmnopqrstuvABCDEFGHIJKLMNOPQRSTU".into(),
                attempts: 1,
            }];
            for frame in 0..2 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 1400.),
                        )),
                        time: Some(frame as f64),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                            app.tunnels_dialog(ui, ctx);
                            assert!(
                                ui.min_rect().width() <= width,
                                "Tunnel dialog overflows at {width}"
                            );
                        });
                    },
                );
            }
            assert!(
                app.tunnels.pending.is_none(),
                "Painting must not start a tunnel"
            );
            app.workspace
                .as_ref()
                .unwrap()
                .remote
                .as_ref()
                .unwrap()
                .cleanup();
        }
    }
}

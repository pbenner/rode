//! Native live-agent discovery is separate from backend saved history. Listing
//! starts no agent or model; an attachment keeps the original host and policy.
use super::*;
use rode_agent_core::service::Info;
use std::sync::mpsc;

mod rows;

#[derive(Default)]
/// Cached live-agent discovery and at most one captured worker; confirmed End
/// requests keep their original host identity across project changes.
pub(super) struct Running {
    items: Vec<Info>,
    pending: Option<mpsc::Receiver<Result<rode_session::Response, String>>>,
    pub(super) mutating: bool,
    ready: bool,
    last: Option<Instant>,
    error: String,
    confirm: Option<Info>,
}
impl Running {
    /// Discovery belongs to the selected project. An approved mutation keeps its
    /// captured worker and acknowledgement even after that project is closed.
    pub(super) fn reset_context(&mut self) {
        let pending = if self.mutating {
            self.pending.take()
        } else {
            None
        };
        *self = Self {
            mutating: pending.is_some(),
            pending,
            ..Self::default()
        };
    }
}
/// Render the acknowledged lifetime toggle and submit an explicit in-place policy change.
pub(super) fn persistence(
    ui: &mut egui::Ui,
    chat: &mut Chat,
    palette: theme::Palette,
    enabled: bool,
) {
    let on = chat.persistent();
    let mut button = egui::Button::new(
        RichText::new(if on { "Persist: on" } else { "Persist: off" }).color(if on {
            palette.background
        } else {
            palette.text
        }),
    )
    .min_size(egui::vec2(84., 0.));
    if on {
        button = button
            .fill(palette.accent)
            .stroke(egui::Stroke::new(1.0_f32, palette.accent));
    }
    if ui.add_enabled(enabled && chat.persistence_ready(), button)
        .on_hover_text("Keep this agent running after closing RODE, changing projects or losing SSH. Permissions still apply; questions wait until you reattach from Running. New chats default off.")
        .on_disabled_hover_text("Waiting for native agent startup or persistence acknowledgement.")
        .clicked()
        && let Err(error) = chat.set_persistent(!on)
    { chat.notice(&error.to_string()); }
}
impl EditorApp {
    /// Open live-session discovery and request metadata without starting a coding agent.
    pub(in crate::app) fn open_background_agents(&mut self, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Agents) {
            return;
        }
        self.set_dialog(Dialog::AgentSessions);
        self.refresh_background_agents(None, ctx);
    }
    /// Queue either read-only discovery or an already confirmed termination on the captured host.
    fn refresh_background_agents(&mut self, end: Option<Info>, ctx: &egui::Context) {
        if self.agents.running.pending.is_some() {
            return;
        }
        let launch = self.agent_launch();
        let upload = !self.agents.running.ready;
        self.agents.running.mutating = end.is_some();
        if end.is_some() {
            self.agents.running.error.clear();
        }
        let request = end
            .map(|info| rode_session::Request::EndAgent {
                id: info.id,
                policy: info.policy,
            })
            .unwrap_or(rode_session::Request::Agents);
        let (sender, receiver) = mpsc::channel();
        self.agents.running.pending = Some(receiver);
        self.agents.running.last = Some(Instant::now());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = agent::background::request(&launch, request, upload)
                .map(|reply| reply.response)
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }
    /// Drain discovery/mutation acknowledgements and preserve shutdown errors for a waiting exit.
    pub(super) fn poll_background_agents(&mut self, ctx: &egui::Context) {
        let result = self
            .agents
            .running
            .pending
            .as_ref()
            .and_then(|r| match r.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                    "Background-agent worker disconnected; inspect Running agents before exiting"
                        .into(),
                )),
            });
        if let Some(result) = result {
            self.agents.running.pending = None;
            let mutating = std::mem::take(&mut self.agents.running.mutating);
            match result {
                Ok(rode_session::Response::Agents(items)) if !mutating => {
                    self.agents.running.items = items;
                    self.agents.running.error.clear();
                    self.agents.running.ready = true;
                }
                Ok(rode_session::Response::Ok) => {
                    self.agents.running.last = None;
                }
                Ok(rode_session::Response::Error(error)) | Err(error) => {
                    self.agents.running.error = error
                }
                _ => self.agents.running.error = "Unexpected background-agent service reply".into(),
            }
            if mutating && !self.agents.running.error.is_empty() {
                self.abort_session_quit(self.agents.running.error.clone(), ctx);
            }
        }
        if self.agents.running.mutating {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.dialog == Dialog::AgentSessions {
            if self
                .agents
                .running
                .last
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(3))
            {
                self.refresh_background_agents(None, ctx);
            }
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
    /// Render cached live sessions and their compatibility constraints; attachments
    /// retain the original project/sandbox identity and End requires confirmation.
    pub(in crate::app) fn background_agents_dialog(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        let palette = self.themes.active().palette;
        let (root, host, _) = self.agent_location();
        let launch = self.agent_launch();
        let mode = launch.plan.label();
        ui.horizontal(|ui| {
            ui.heading("Running agents");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        self.agents.running.pending.is_none(),
                        egui::Button::new("Refresh"),
                    )
                    .clicked()
                {
                    self.refresh_background_agents(None, ctx);
                }
                if self.agents.running.pending.is_some() {
                    ui.spinner();
                }
            });
        });
        ui.add(egui::Label::new(palette.tag(format!("{host} · {mode}"))).truncate());
        ui.add(egui::Label::new(format!("Current project: {root}")).truncate())
            .on_hover_text(&root);
        ui.add(egui::Label::new(RichText::new("Live chats on this host. Attach uses the original project and sandbox.").small().color(palette.muted)).wrap())
            .on_hover_text("Persist keeps work running after its controlling view closes. Only one window can control a chat. Permission requests wait for you; they are never automatically approved.");
        if let Some(info) = self.agents.running.confirm.clone() {
            ui.separator();
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "End agent '{}' in {}? This stops its current task and tools.",
                        info.name, info.root
                    ))
                    .color(palette.error),
                )
                .wrap(),
            );
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    self.agents.running.confirm = None;
                }
                if ui
                    .add_enabled(
                        self.agents.running.pending.is_none(),
                        egui::Button::new("Confirm end"),
                    )
                    .clicked()
                {
                    self.agents.running.confirm = None;
                    self.refresh_background_agents(Some(info), ctx);
                }
            });
        }
        if !self.agents.running.error.is_empty() {
            ui.colored_label(palette.error, &self.agents.running.error);
        }
        let policy = crate::sessions::fingerprint(&launch.plan);
        let context = rows::Context {
            current: self
                .agents
                .chat
                .native_id()
                .filter(|_| self.agents.chat.connected() || self.agents.chat.busy),
            root: &root,
            policy: &policy,
            mode,
            can_leave: !self.agents.chat.busy || self.agents.chat.persistent(),
        };
        let selected = egui::ScrollArea::vertical()
            .id_salt("running-agents")
            .auto_shrink([false, true])
            .min_scrolled_height(0.)
            .max_height((ctx.content_rect().height() - 300.).clamp(140., 520.))
            .show(ui, |ui| {
                if self.agents.running.items.is_empty() {
                    ui.add_space(16.);
                    ui.strong("No running agents on this host");
                    ui.add(
                        egui::Label::new("Saved conversations are available through History.")
                            .wrap(),
                    );
                    ui.add_space(16.);
                }
                rows::show(ui, &self.agents.running.items, &context, palette)
            })
            .inner;
        let mut attach = None;
        if let Some((id, action)) = selected {
            match action {
                rows::Action::Show => {
                    self.dialog = Dialog::None;
                    self.workbench.panel = Some(Panel::Agent);
                    self.focus_pane(navigation::Pane::Tools, ctx);
                }
                rows::Action::Attach => {
                    attach = self
                        .agents
                        .running
                        .items
                        .iter()
                        .find(|info| info.id == id)
                        .cloned();
                }
                rows::Action::End => {
                    self.agents.running.confirm = self
                        .agents
                        .running
                        .items
                        .iter()
                        .find(|info| info.id == id)
                        .cloned();
                }
            }
        }
        if let Some(info) = attach {
            let launch = self.agent_launch();
            let wake = ctx.clone();
            self.agents.replace_chat();
            self.agents
                .chat
                .attach(launch, info, Arc::new(move || wake.request_repaint()));
            self.agents.attaching = true;
            self.agents.focus = true;
            self.dialog = Dialog::None;
            self.workbench.panel = Some(Panel::Agent);
            self.focus_pane(navigation::Pane::Tools, ctx);
        }
        if ui.button("Close").clicked() {
            self.dialog = Dialog::None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_switch_keeps_pending_agent_end() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.poll_agents(&ctx);
        let (reply, receiver) = mpsc::channel();
        app.agents.running.pending = Some(receiver);
        app.agents.running.mutating = true;
        assert!(app.agent_installation_pending());
        let replacement = tempfile::tempdir().unwrap();
        app.guarded(
            Action::Project {
                path: replacement.path().to_string_lossy().into(),
                remote: false,
                options: sandbox::Options::default(),
            },
            &ctx,
        );
        let until = Instant::now() + Duration::from_secs(5);
        while app.preparing {
            app.poll(&ctx);
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        app.poll_agents(&ctx);
        app.quit_terminals(&ctx);
        assert!(app.session_exit_pending());
        assert!(!app.quitting);
        assert!(
            reply
                .send(Err("End agent failed on the original host".into()))
                .is_ok(),
            "Project switching discarded the mutation receiver"
        );
        app.poll_agents(&ctx);
        assert!(app.agents.running.error.contains("End agent failed"));
        assert!(!app.session_exit_pending());
        assert!(!app.quitting);
    }

    #[test]
    fn disconnected_shutdown_worker_aborts_exit_instead_of_waiting_forever() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.poll_agents(&ctx);
        let (reply, receiver) = mpsc::channel();
        app.agents.running.pending = Some(receiver);
        app.agents.running.mutating = true;
        app.quit_terminals(&ctx);
        assert!(app.session_exit_pending());
        drop(reply);
        app.poll_agents(&ctx);
        assert!(!app.agent_installation_pending());
        assert!(!app.session_exit_pending());
        assert!(!app.quitting);
        assert!(app.agents.running.error.contains("worker disconnected"));
    }
}

//! Ordered restore: guarded project preparation, protected file reads, then
//! policy-checked attachment to already existing host work. No work is replayed.
use super::*;
use crate::agent;
use rode_runtime_core::service::Kind;
use std::collections::VecDeque;

struct Services {
    agents: Vec<rode_agent_core::service::Info>,
    terminals: Option<crate::sessions::Reply>,
    runtimes: Vec<rode_runtime_core::service::Info>,
}
pub(super) struct Restoring {
    pub location: Location,
    pub approved: Vec<(u64, u64)>,
    record: Option<Record>,
    takeover: bool,
    generation: Option<u64>,
    queue: VecDeque<model::Document>,
    pending: Option<(model::Document, String, egui::ViewportId)>,
    groups: HashMap<u64, egui::ViewportId>,
    documents: HashMap<u64, u64>,
    services: Option<mpsc::Receiver<Result<Services, String>>>,
    discovery_started: bool,
}
impl Restoring {
    pub fn claiming(location: Location, approved: Vec<(u64, u64)>) -> Self {
        Self {
            location,
            approved,
            record: None,
            takeover: false,
            generation: None,
            queue: VecDeque::new(),
            pending: None,
            groups: HashMap::from([(0, egui::ViewportId::ROOT)]),
            documents: HashMap::new(),
            services: None,
            discovery_started: false,
        }
    }
    pub fn opening(&self) -> bool {
        self.generation.is_none()
    }
}
impl EditorApp {
    pub(in crate::app) fn workspace_session_transition<'a>(
        &'a self,
        selection: &'a Resume,
    ) -> vaults::Transition<'a> {
        match &selection.location {
            Location::Project {
                project: project_bookmarks::Project::Local { root, .. },
            } => vaults::Transition::Project(root, None),
            Location::Project {
                project: project_bookmarks::Project::Ssh { profile },
            } => self.saved_session_remote_transition(profile),
            Location::Encrypted {
                cipherdir,
                remote: None,
                ..
            } => vaults::Transition::Project(cipherdir, None),
            Location::Encrypted {
                remote: Some(profile),
                ..
            } => self.saved_session_remote_transition(profile),
        }
    }
    fn saved_session_remote_transition<'a>(
        &'a self,
        profile: &'a Profile,
    ) -> vaults::Transition<'a> {
        let remote = self
            .workspace
            .as_ref()
            .and_then(|w| w.remote.as_ref())
            .filter(|r| project_bookmarks::Project::same_connection(&r.profile, profile));
        remote.map_or(vaults::Transition::Connection, |r| {
            vaults::Transition::Project(&profile.root, Some(r))
        })
    }
    pub(super) fn workspace_session_claimed(
        &mut self,
        mut record: Record,
        takeover: bool,
        ctx: &egui::Context,
    ) {
        let state = self.workspace_sessions.as_mut().unwrap();
        let Some(restoring) = state.restore.as_mut() else {
            state.request(Request::Release(record.key()));
            return;
        };
        restoring.takeover = takeover;
        // Keep metadata for selection/runtime mapping without duplicating draft
        // payloads in both the record and the restore queue.
        restoring.queue = record
            .snapshot
            .documents
            .iter_mut()
            .map(|document| {
                let draft = document.draft.take();
                let mut queued = document.clone();
                queued.draft = draft;
                queued
            })
            .collect();
        state.own = Some(Owned {
            key: record.key(),
            location: record.snapshot.location.identity(),
            policy: record.snapshot.policy.clone(),
        });
        state.acknowledged = None;
        state.submitted = None;
        state.frozen = false;
        let location = record.snapshot.location.clone();
        restoring.record = Some(record);
        match location {
            Location::Project {
                project: project_bookmarks::Project::Local { root, sandbox },
            } => self.guarded(
                Action::Project {
                    path: root,
                    remote: false,
                    options: sandbox,
                },
                ctx,
            ),
            Location::Project {
                project: project_bookmarks::Project::Ssh { profile },
            } => self.guarded(Action::Remote(profile), ctx),
            Location::Encrypted {
                cipherdir,
                remote,
                sandbox,
            } => {
                if let Some(profile) = remote {
                    self.guarded(Action::Remote(profile), ctx);
                }
                self.show_workspace_unlock(&cipherdir, sandbox);
            }
        }
        ctx.request_repaint();
    }
    pub(super) fn show_workspace_unlock(&mut self, path: &str, options: sandbox::Options) {
        self.encrypted_dialog_at(path.into());
        self.project_picker.sandbox = options;
    }
    /// Reuse the first admission's exact source versions for the subsequent
    /// project action; edits arriving during filesystem claim still require approval.
    pub(in crate::app) fn workspace_resume_approval(
        &self,
        action: &Action,
        changes: &documents::Changes,
    ) -> bool {
        let Some(restoring) = self
            .workspace_sessions
            .as_ref()
            .and_then(|s| s.restore.as_ref())
        else {
            return false;
        };
        if !restoring.opening() || changes.dirty_versions != restoring.approved {
            return false;
        }
        match (&restoring.location, action) {
            (
                Location::Project {
                    project: project_bookmarks::Project::Local { root, .. },
                },
                Action::Project {
                    path,
                    remote: false,
                    ..
                },
            ) => root == path,
            (
                Location::Project {
                    project: project_bookmarks::Project::Ssh { profile },
                },
                Action::Remote(next),
            ) => {
                profile.root == next.root
                    && project_bookmarks::Project::same_connection(profile, next)
            }
            (
                Location::Encrypted {
                    remote: Some(profile),
                    ..
                },
                Action::Remote(next),
            ) => project_bookmarks::Project::same_connection(profile, next),
            _ => false,
        }
    }
    pub(in crate::app) fn advance_workspace_resume(&mut self, ctx: &egui::Context) {
        let Some(mut restoring) = self
            .workspace_sessions
            .as_mut()
            .and_then(|s| s.restore.take())
        else {
            return;
        };
        let Some(record) = restoring.record.as_ref() else {
            self.workspace_sessions.as_mut().unwrap().restore = Some(restoring);
            return;
        };
        let ready = self.project_ready()
            && !self.workspace.preparing()
            && self.pending.as_ref().is_none()
            && self.dialog != Dialog::Folder;
        if !ready {
            self.workspace_sessions.as_mut().unwrap().restore = Some(restoring);
            return;
        }
        if self
            .workspace_session_location()
            .is_none_or(|l| l.identity() != restoring.location.identity())
        {
            if restoring.location.encrypted() {
                self.workspace_sessions.as_mut().unwrap().restore = Some(restoring);
                return;
            }
            if restoring.opening() {
                self.workspace_sessions.as_mut().unwrap().restore = Some(restoring);
                return;
            }
            self.workspace_sessions.as_mut().unwrap().release();
            return;
        }
        if crate::sessions::fingerprint(&self.session_plan()) != record.snapshot.policy {
            let state = self.workspace_sessions.as_mut().unwrap();
            state.release();
            state.error = "The original sandbox policy is unavailable. Saved files and live jobs were not restored under a different policy.".into();
            return;
        }
        if restoring
            .generation
            .is_some_and(|g| g != self.workers.generation())
        {
            self.workspace_sessions.as_mut().unwrap().release();
            return;
        }
        restoring.generation = Some(self.workers.generation());
        if let Some((saved, path, viewport)) = restoring.pending.take() {
            if let Some(id) = self
                .documents
                .iter()
                .find(|d| d.path.as_deref() == Some(&path))
                .map(|d| d.id)
            {
                let result = self
                    .documents
                    .by_id_mut(id)
                    .unwrap()
                    .restore_workspace_state(
                        &saved,
                        &self.markdown_images,
                        &self.notebook_shutdowns,
                    );
                let restored = match result {
                    documents::Restore::Edited => {
                        let recovered = self.restore_unbound_workspace_document(&saved, viewport);
                        self.notify(format!("Your current edits to {} were kept. Any saved draft is recovered separately.", saved.name), false);
                        recovered.unwrap_or(id)
                    }
                    documents::Restore::FileChanged => {
                        self.notify(format!("{} changed on its host. Its unsaved draft is recovered separately; choose Save As.", saved.name), true);
                        id
                    }
                    documents::Restore::Applied => id,
                };
                restoring.documents.insert(saved.key, restored);
            } else if self.documents.is_opening(&path) {
                restoring.pending = Some((saved, path, viewport));
                self.workspace_sessions.as_mut().unwrap().restore = Some(restoring);
                return;
            } else if let Some(id) = self.restore_unbound_workspace_document(&saved, viewport) {
                restoring.documents.insert(saved.key, id);
            }
        }

        if let Some(saved) = restoring.queue.pop_front() {
            let viewport = if let Some(viewport) = restoring.groups.get(&saved.group) {
                *viewport
            } else {
                let viewport = self.allocate_editor_window();
                if let Some(window) = record
                    .snapshot
                    .layout
                    .editor_windows
                    .iter()
                    .find(|w| w.group == saved.group)
                    && window.size.iter().all(|s| s.is_finite())
                {
                    ctx.send_viewport_cmd_to(
                        viewport,
                        egui::ViewportCommand::InnerSize(egui::vec2(
                            window.size[0].clamp(480., 3200.),
                            window.size[1].clamp(320., 1800.),
                        )),
                    );
                }
                restoring.groups.insert(saved.group, viewport);
                viewport
            };
            if let Some(path) = &saved.path {
                let path = if saved.relative {
                    std::path::Path::new(&self.workspace.as_ref().unwrap().root)
                        .join(path)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    path.clone()
                };
                self.read_workspace_file(path.clone(), viewport, ctx, false);
                restoring.pending = Some((saved, path, viewport));
            } else if let Some(id) = self.restore_unbound_workspace_document(&saved, viewport) {
                restoring.documents.insert(saved.key, id);
            }
            self.workspace_sessions.as_mut().unwrap().restore = Some(restoring);
            ctx.request_repaint();
            return;
        }
        if !restoring.discovery_started {
            restoring.discovery_started = true;
            let agents_needed = record.snapshot.agent.is_some();
            let terminals_needed = !record.snapshot.terminals.is_empty();
            let runtimes_needed = !record.snapshot.runtimes.is_empty();
            if agents_needed || terminals_needed || runtimes_needed {
                let launch = self.agent_launch();
                let (tx, rx) = mpsc::channel();
                restoring.services = Some(rx);
                let wake = ctx.clone();
                std::thread::spawn(move || {
                    let result = (|| -> anyhow::Result<Services> {
                        let agents = if agents_needed {
                            match agent::background::request(
                                &launch,
                                rode_session::Request::Agents,
                            )?
                            .response
                            {
                                rode_session::Response::Agents(items) => items,
                                _ => Vec::new(),
                            }
                        } else {
                            Vec::new()
                        };
                        let terminals = if !terminals_needed {
                            None
                        } else {
                            Some(crate::sessions::on_host(
                                &launch.plan,
                                launch.remote.as_ref(),
                                crate::sessions::Operation::List,
                            )?)
                        };
                        let runtimes = if !runtimes_needed {
                            Vec::new()
                        } else {
                            match crate::runtime::request(
                                &launch.plan,
                                launch.remote.as_ref(),
                                rode_session::Request::Runtimes,
                            )?
                            .response
                            {
                                rode_session::Response::Runtimes(items) => items,
                                _ => Vec::new(),
                            }
                        };
                        Ok(Services {
                            agents,
                            terminals,
                            runtimes,
                        })
                    })()
                    .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(result);
                    wake.request_repaint();
                });
            }
        }
        if let Some(receiver) = &restoring.services {
            match receiver.try_recv() {
                Ok(Ok(services)) => {
                    self.restore_workspace_services(&restoring, services, ctx);
                    restoring.services = None;
                }
                Ok(Err(error)) => {
                    self.notify(format!("Workspace files restored; live sessions could not be discovered: {error}"), true);
                    restoring.services = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.workspace_sessions.as_mut().unwrap().restore = Some(restoring);
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.notify("Live-session discovery stopped; inspect Running agents or Sessions before reattaching.", true);
                    restoring.services = None;
                }
            }
        }
        let record = restoring.record.take().unwrap();
        self.restore_workspace_prompt(&record.snapshot.draft);
        for saved in &record.snapshot.documents {
            if saved.selected
                && let Some(id) = restoring.documents.get(&saved.key)
            {
                self.documents.select(*id);
            }
        }
        let mut layout = record.snapshot.layout;
        layout.floating.retain(|tab| match tab {
            ToolTab::Panel(panel) => panel
                .definition()
                .feature
                .is_none_or(|feature| self.feature(feature)),
            ToolTab::Extension(_) => false,
            _ => true,
        });
        for id in &layout.floating_extensions {
            if let Some(tab) = self.extension_panels.tab(id) {
                layout.floating.push(tab);
            }
        }
        if layout
            .panel
            .is_some_and(|p| p.definition().feature.is_some_and(|f| !self.feature(f)))
        {
            layout.panel = None;
            layout.tools = false;
        }
        let last_tool = layout.last_tool.filter(|tab| match tab {
            ToolTab::Panel(panel) => panel.definition().feature.is_none_or(|f| self.feature(f)),
            _ => true,
        });
        self.presentation
            .restore_workspace(presentation::Selection {
                sidebar: layout.sidebar,
                panel: layout.panel,
                surface: if layout.terminal {
                    tool_windows::Surface::Terminal
                } else if layout.tools {
                    tool_windows::Surface::Tools
                } else {
                    tool_windows::Surface::Editor
                },
                detached: layout.detached,
                tools_hidden: layout.tools_hidden,
                floating_tabs: layout.floating,
                last_tool,
                ..Default::default()
            });
        self.navigation.explorer_width = layout.explorer_width;
        self.sidebar.connections_width = layout.connections_width;
        if let Some([width, height]) = layout
            .window_size
            .filter(|s| s.iter().all(|n| n.is_finite()))
        {
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::InnerSize(egui::vec2(
                    width.clamp(850., 3200.),
                    height.clamp(560., 1800.),
                )),
            );
        }
        self.workspace_sessions.as_mut().unwrap().frozen = false;
        self.workspace_sessions.as_mut().unwrap().refresh();
        self.status = format!("Workspace resumed · {}", record.name);
        ctx.request_repaint();
    }
    fn restore_unbound_workspace_document(
        &mut self,
        saved: &model::Document,
        viewport: egui::ViewportId,
    ) -> Option<u64> {
        let entry = saved.draft.as_ref()?;
        let id = self.documents.restore_crash_entry(
            entry,
            &self.markdown_images,
            &self.notebook_shutdowns,
        );
        self.documents
            .by_id_mut(id)
            .unwrap()
            .restore_workspace_state(saved, &self.markdown_images, &self.notebook_shutdowns);
        self.documents.move_to(id, viewport);
        Some(id)
    }
    /// Resuming must not erase a draft already being composed in this window.
    /// Keep it in place and recover a differing saved prompt as an unbound buffer.
    fn restore_workspace_prompt(&mut self, saved: &str) {
        let current = self.agents.recovery_draft();
        if !current.is_empty() && current != saved {
            if !saved.is_empty() {
                self.documents.restore_crash_entry(
                    &crash_recovery::Entry {
                        name: "Saved agent prompt draft".into(),
                        text: saved.into(),
                        notebook: false,
                    },
                    &self.markdown_images,
                    &self.notebook_shutdowns,
                );
                self.notify("Your current prompt draft was kept. The saved prompt is open as a separate unsaved file.", false);
            }
        } else {
            self.agents.restore_workspace_draft(saved.into());
        }
    }
    fn restore_workspace_services(
        &mut self,
        restoring: &Restoring,
        services: Services,
        ctx: &egui::Context,
    ) {
        let snapshot = &restoring.record.as_ref().unwrap().snapshot;
        if let Some(saved) = &snapshot.agent
            && let Some(info) = services
                .agents
                .into_iter()
                .find(|i| i.id == saved.id && i.policy == saved.policy && i.alive)
        {
            if !info.attached {
                self.attach_workspace_agent(info, false, ctx);
            } else if restoring.takeover
                && saved.controller.is_some()
                && info.controller.as_ref().map(|c| &c.id) == saved.controller.as_ref()
            {
                self.attach_workspace_agent(info, true, ctx);
            } else {
                self.notify("The saved agent is controlled by another window. Use Running agents to confirm a takeover.", false);
            }
        }
        if let Some(reply) = services.terminals {
            self.restore_workspace_terminals(
                reply,
                &snapshot.terminals,
                snapshot.selected_terminal.as_deref(),
                ctx,
            );
        }
        for saved in &snapshot.runtimes {
            let Some(info) = services
                .runtimes
                .iter()
                .find(|i| i.id == saved.id && i.alive && !i.attached)
                .cloned()
            else {
                continue;
            };
            let result = match info.kind {
                Kind::Notebook => saved
                    .document
                    .and_then(|key| restoring.documents.get(&key))
                    .and_then(|id| self.documents.iter().position(|d| d.id == *id))
                    .map(|index| self.attach_notebook_runtime(index, info, ctx)),
                Kind::Debugger => Some(self.attach_debugger_runtime(info, ctx)),
            };
            if let Some(Err(error)) = result {
                self.notify(format!("Cannot reattach runtime: {error:#}"), true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resuming_preserves_an_existing_private_prompt_and_recovers_the_saved_prompt_separately() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = EditorApp::new(&cc, Startup::default());
        app.agents
            .restore_workspace_draft("Current private draft".into());
        assert!(app.agents.checkpoint_draft(true).is_none());
        app.restore_workspace_prompt("Previously saved draft");
        assert_eq!(app.agents.recovery_draft(), "Current private draft");
        assert!(app.agents.checkpoint_draft(false).is_none());
        let recovered = app.documents.active().unwrap();
        assert_eq!(recovered.text, "Previously saved draft");
        assert!(recovered.path.is_none());
        assert!(recovered.dirty());
    }
}

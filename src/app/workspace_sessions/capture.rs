//! Capture only stable, prepared workspaces. Snapshots never change service
//! persistence and encrypted contents remain excluded across disconnects.
use super::*;
use sha2::{Digest, Sha256};
impl EditorApp {
    pub(in crate::app) fn workspace_session_location(&self) -> Option<Location> {
        let workspace = self.workspace.as_ref().filter(|w| w.ready)?;
        if self.workspace.preparing() {
            return None;
        }
        self.vaults
            .workspace_location(&workspace.sandbox_options)
            .or_else(|| {
                let project = if let Some(remote) = &workspace.remote {
                    let mut profile = remote.profile.clone();
                    profile.root = workspace.root.clone();
                    profile.sandbox = workspace.sandbox_options.clone();
                    project_bookmarks::Project::Ssh {
                        profile: Box::new(profile),
                    }
                } else {
                    project_bookmarks::Project::Local {
                        root: workspace.root.clone(),
                        sandbox: workspace.sandbox_options.clone(),
                    }
                };
                Some(Location::Project { project })
            })
    }
    fn saved_layout(&self, ctx: &egui::Context) -> model::Layout {
        model::Layout {
            sidebar: self.presentation.sidebar,
            panel: self.presentation.panel,
            terminal: self.presentation.surface == tool_windows::Surface::Terminal,
            tools: self.presentation.surface == tool_windows::Surface::Tools,
            detached: self.presentation.detached,
            tools_hidden: self.presentation.tools_hidden,
            last_tool: self.presentation.last_tool,
            floating: self
                .presentation
                .floating_tabs
                .iter()
                .copied()
                .filter(|t| !matches!(t, ToolTab::Extension(_) | ToolTab::Panel(Panel::Extension)))
                .collect(),
            floating_extensions: self
                .presentation
                .floating_tabs
                .iter()
                .filter_map(|t| self.extension_panels.id_for_tab(*t).map(str::to_owned))
                .collect(),
            explorer_width: egui::containers::panel::PanelState::load(
                ctx,
                explorer::panel_id(egui::ViewportId::ROOT, false),
            )
            .map(|s| s.rect.width()),
            connections_width: egui::containers::panel::PanelState::load(
                ctx,
                sidebar::connections_panel_id(false),
            )
            .map(|s| s.rect.width()),
            editor_windows: ctx.input(|i| {
                i.raw
                    .viewports
                    .iter()
                    .filter(|(id, _)| {
                        **id != egui::ViewportId::ROOT
                            && self
                                .documents
                                .iter()
                                .any(|d| self.documents.owner(d.id) == Some(**id))
                    })
                    .filter_map(|(id, info)| {
                        info.inner_rect.map(|r| model::EditorWindow {
                            group: id.0.value(),
                            size: [r.width(), r.height()],
                        })
                    })
                    .collect()
            }),
            window_size: ctx.input(|i| {
                i.raw
                    .viewports
                    .get(&egui::ViewportId::ROOT)
                    .and_then(|v| v.inner_rect)
                    .map(|r| [r.width(), r.height()])
            }),
        }
    }
    fn session_stamp(&self, ctx: &egui::Context, location: &Location) -> Stamp {
        let signature = serde_json::json!([
            location.identity(),
            self.documents.workspace_signature(),
            self.saved_layout(ctx),
            self.agents.recovery_draft(),
            self.agents.chat.native_id(),
            self.agents.chat.native_controller(),
            self.sessions
                .tabs()
                .iter()
                .map(|m| &m.key)
                .collect::<Vec<_>>(),
            self.debugger.client.as_ref().and_then(|c| c.native_id()),
            self.documents
                .iter()
                .map(|d| d.notebook().and_then(|n| n.native_id()))
                .collect::<Vec<_>>()
        ]);
        let dirty = self.documents.changes().dirty_versions;
        let eligible = !location.encrypted()
            && self
                .documents
                .iter()
                .filter(|d| d.dirty())
                .all(|d| !d.workspace_private() && d.hex().is_none());
        Stamp {
            prompt: format!(
                "{:x}",
                Sha256::digest(self.agents.recovery_draft().as_bytes())
            ),
            hash: format!("{:x}", Sha256::digest(signature.to_string().as_bytes())),
            dirty,
            eligible,
        }
    }
    fn capture_workspace(
        &mut self,
        ctx: &egui::Context,
        location: Location,
    ) -> anyhow::Result<Snapshot> {
        let root = self.workspace.as_ref().unwrap().root.clone();
        let policy = crate::sessions::fingerprint(&self.session_plan());
        let mut runtimes: Vec<_> = self
            .documents
            .iter()
            .filter_map(|d| {
                d.notebook()
                    .and_then(|n| n.native_id())
                    .map(|id| model::Runtime {
                        id: id.into(),
                        document: Some(d.id),
                    })
            })
            .collect();
        if let Some(id) = self.debugger.client.as_ref().and_then(|c| c.native_id()) {
            runtimes.push(model::Runtime {
                id: id.into(),
                document: None,
            });
        }
        let snapshot = Snapshot {
            draft: self
                .agents
                .checkpoint_draft(location.encrypted())
                .unwrap_or_default()
                .to_owned(),
            agent: self.agents.chat.native_id().map(|id| model::Agent {
                id: id.into(),
                policy: policy.clone(),
                controller: self.agents.chat.native_controller(),
            }),
            terminals: self
                .sessions
                .tabs()
                .into_iter()
                .filter(|m| m.native.is_some())
                .map(|m| m.key)
                .collect(),
            selected_terminal: self
                .sessions
                .active
                .as_ref()
                .filter(|m| m.native.is_some())
                .map(|m| m.key.clone()),
            documents: self
                .documents
                .workspace_documents(&root, location.encrypted())?,
            layout: self.saved_layout(ctx),
            location,
            policy,
            runtimes,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
    pub(in crate::app) fn enable_workspace_session(&mut self, ctx: &egui::Context) {
        if self
            .workspace_sessions
            .as_ref()
            .is_none_or(|s| s.own.is_some() || s.enabling || s.restore.is_some())
        {
            return;
        }
        let Some(location) = self.workspace_session_location() else {
            return;
        };
        let stamp = self.session_stamp(ctx, &location);
        let result = self.capture_workspace(ctx, location).and_then(|snapshot| {
            let state = self
                .workspace_sessions
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Workspace storage is unavailable"))?;
            let previous = state
                .paused
                .as_ref()
                .filter(|old| {
                    old.location == snapshot.location.identity() && old.policy == snapshot.policy
                })
                .map(|old| old.key.clone());
            state.serial += 1;
            state.worker.keep(previous, state.serial, snapshot)?;
            state.submitted = Some((state.serial, stamp));
            state.enabling = true;
            state.failed = None;
            state.error.clear();
            Ok(())
        });
        if let Err(error) = result {
            self.notify(format!("Cannot keep workspace: {error:#}"), true);
        }
    }
    pub(in crate::app) fn checkpoint_workspace_session(&mut self, ctx: &egui::Context) {
        let Some(location) = self.workspace_session_location() else {
            return;
        };
        let Some(state) = &self.workspace_sessions else {
            return;
        };
        if state.restore.is_some() || state.enabling {
            return;
        }
        let Some(own) = &state.own else { return };
        let identity = location.identity();
        if own.location != identity
            || own.policy != crate::sessions::fingerprint(&self.session_plan())
        {
            self.workspace_sessions.as_mut().unwrap().release();
            return;
        }
        if state.frozen {
            if self.exit.waiting()
                || self.exit.closing()
                || self.pending.as_ref().is_some()
                || self.vaults.locking()
            {
                return;
            }
            self.workspace_sessions.as_mut().unwrap().frozen = false;
        }
        let state = self.workspace_sessions.as_ref().unwrap();
        let own = state.own.as_ref().unwrap();
        let stamp = self.session_stamp(ctx, &location);
        if state.failed.as_ref() == Some(&stamp.hash) {
            return;
        }
        if state.checkpoint_matches(&stamp.hash) {
            return;
        }
        let key = own.key.clone();
        match self.capture_workspace(ctx, location) {
            Ok(snapshot) => {
                let state = self.workspace_sessions.as_mut().unwrap();
                state.failed = None;
                state.error.clear();
                state.serial += 1;
                state.worker.save(key, state.serial, snapshot);
                state.submitted = Some((state.serial, stamp));
            }
            Err(error) => {
                let state = self.workspace_sessions.as_mut().unwrap();
                state.failed = Some(stamp.hash);
                state.error = format!("Workspace checkpoint failed: {error:#}");
                let message = state.error.clone();
                self.notify(format!("{message}. Open Welcome to retry saving or turn off Keep workspace before leaving."), true);
            }
        }
    }
    /// A named durable record already owns these drafts. Clear the window's
    /// fallback crash copy so reopening offers Resume instead of duplicate recovery.
    pub(in crate::app) fn workspace_drafts_durable(
        &self,
        versions: &[(u64, u64)],
        draft: &str,
    ) -> bool {
        self.workspace_sessions.as_ref().is_some_and(|state| {
            state.own.is_some()
                && state.restore.is_none()
                && state.failed.is_none()
                && state.submitted.is_none()
                && state.acknowledged.as_ref().is_some_and(|stamp| {
                    stamp.dirty == versions
                        && stamp.prompt == format!("{:x}", Sha256::digest(draft.as_bytes()))
                })
        })
    }
    /// A durable snapshot approves only its exact source versions. Binary and
    /// encrypted edits continue through the ordinary unsaved-buffer decision.
    pub(in crate::app) fn workspace_session_retains(
        &self,
        changes: &documents::Changes,
    ) -> (bool, bool) {
        let Some(state) = &self.workspace_sessions else {
            return (false, false);
        };
        if (state.own.is_none() && !state.enabling) || state.restore.is_some() {
            return (false, false);
        }
        let prompt = format!(
            "{:x}",
            Sha256::digest(self.agents.recovery_draft().as_bytes())
        );
        if let Some(stamp) = &state.acknowledged
            && state.submitted.is_none()
            && stamp.prompt == prompt
            && stamp.eligible
            && stamp.dirty == changes.dirty_versions
        {
            return (true, false);
        }
        let waiting = state.enabling
            || state.submitted.as_ref().is_some_and(|(_, s)| {
                s.eligible && s.dirty == changes.dirty_versions && s.prompt == prompt
            });
        let eligible = self
            .workspace_session_location()
            .is_some_and(|location| !location.encrypted())
            && self
                .documents
                .iter()
                .filter(|d| d.dirty())
                .all(|d| !d.workspace_private() && d.hex().is_none());
        (
            false,
            waiting
                || (eligible && state.failed.is_some() && !self.agents.recovery_draft().is_empty())
                || (eligible && state.failed.is_none() && state.error.is_empty() && !state.frozen),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverting_a_prompt_cannot_admit_close_or_retire_recovery_while_a_different_write_is_pending()
    {
        let temp = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = EditorApp::new(&cc, Startup::default());
        app.agents.restore_workspace_draft("original prompt".into());
        let mut state =
            State::new(worker::Worker::start(temp.path().to_owned(), Arc::new(|| {})).unwrap());
        state.own = Some(Owned {
            key: Key {
                id: rode_session::new_id().unwrap(),
                generation: rode_session::new_id().unwrap(),
            },
            location: String::new(),
            policy: "a".repeat(64),
        });
        let stamp = |hash: &str, prompt: &str| Stamp {
            hash: hash.into(),
            prompt: format!("{:x}", Sha256::digest(prompt.as_bytes())),
            dirty: vec![],
            eligible: true,
        };
        state.acknowledged = Some(stamp("original", "original prompt"));
        state.submitted = Some((2, stamp("intermediate", "intermediate prompt")));
        assert!(
            !state.checkpoint_matches("original"),
            "Reverting must supersede the queued intermediate write"
        );
        app.workspace_sessions = Some(state);
        assert!(!app.workspace_session_retains(&app.documents.changes()).0);
        assert!(!app.workspace_drafts_durable(&[], "original prompt"));
        let key = app
            .workspace_sessions
            .as_ref()
            .unwrap()
            .own
            .as_ref()
            .unwrap()
            .key
            .clone();
        app.workspace_sessions
            .as_mut()
            .unwrap()
            .save_failed(&key, 2);
        assert!(
            !app.workspace_sessions
                .as_ref()
                .unwrap()
                .checkpoint_matches("original")
        );
        assert!(!app.workspace_session_retains(&app.documents.changes()).0);
        assert!(!app.workspace_drafts_durable(&[], "original prompt"));
        let state = app.workspace_sessions.as_mut().unwrap();
        state.acknowledged = Some(stamp("original", "original prompt"));
        state.failed = None;
        assert!(app.workspace_session_retains(&app.documents.changes()).0);
        assert!(app.workspace_drafts_durable(&[], "original prompt"));
    }
}

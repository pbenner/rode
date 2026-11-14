//! Losing a transport clears workspace selection. Retained buffers have no
//! writable path until Save As or an explicit reconnect to the same project.
//!
//! Disconnect invalidates host work and unbinds retained documents before restoring local
//! opening. Reconnect restores paths/revisions only for the same explicitly selected
//! host/project after preparation; recovery metadata alone never authorizes a save.
use super::*;

impl EditorApp {
    /// Start authenticated SSH preparation for a captured profile after ownership guards;
    /// reconnect preserves only recovery state belonging to the matching connection.
    pub(super) fn connect(&mut self, profile: Profile, ctx: &egui::Context, reconnect: bool) {
        let barrier = self.vaults.admission(if reconnect {
            vaults::Transition::RecoveryConnection
        } else {
            vaults::Transition::Connection
        });
        if barrier != vaults::Barrier::None {
            self.notify(barrier.message(), true);
            return;
        }
        self.startup_restore = None;
        let remote = match Remote::new(profile.clone()) {
            Ok(remote) => remote,
            Err(error) => {
                self.notify(error.to_string(), true);
                return;
            }
        };
        if reconnect {
            self.leave_debugger();
            self.stop_latex();
            self.workspace.cancel_connection();
            self.terminal = None;
            self.release_workspace_transport();
            self.workers.invalidate_workspace();
            self.workbench.reset_monitor();
            self.explorer_state.loading.clear();
            self.documents.reconnecting();
        } else {
            self.reset_workspace(workspace::LeaveReason::ProjectChanged);
        }
        self.workspace.connecting(
            remote.clone(),
            profile.root.clone(),
            profile.sandbox.clone(),
        );
        match Terminal::remote(ctx, &remote) {
            Ok(terminal) => {
                let alive = terminal.alive.clone();
                self.ssh_master = Some(terminal);
                self.show_terminal = true;
                self.terminal_intent();
                self.status = "Connecting · complete any prompts in the terminal".into();
                let cancel = self.workspace.begin_connection();
                let replies = self.workers.workspace(ctx);
                let vault_work = self.vaults.work();
                std::thread::spawn(move || {
                    let _vault_work = vault_work;
                    let start = Instant::now();
                    let result = loop {
                        if cancel.load(Ordering::Acquire) {
                            return;
                        }
                        if !alive.load(Ordering::Acquire) {
                            break Err("SSH login failed.".into());
                        }
                        if remote.connected() {
                            break Ok(());
                        }
                        if start.elapsed() > Duration::from_secs(300) {
                            break Err(
                                "SSH login timed out after five minutes. Reconnect to try again."
                                    .into(),
                            );
                        }
                        std::thread::sleep(Duration::from_millis(200));
                    };
                    let _ = replies.send(WorkspaceEvent::Connected { result });
                });
                Terminal::focus(ctx);
            }
            Err(error) => {
                self.ssh_failed(format!("{error:#}"), ctx);
            }
        }
    }

    /// Await saves/file/lifetime acknowledgements before explicit disconnect; encrypted
    /// leases retain their lock guard. Forced transport loss uses detach_ssh directly.
    pub(super) fn disconnect_ssh(&mut self, ctx: &egui::Context) {
        self.guarded(Action::Disconnect, ctx);
    }

    /// A successful transport still needs guarded host-policy preparation before file access.
    pub(super) fn connection_completed(&mut self, result: Result<(), String>, ctx: &egui::Context) {
        match result {
            Ok(()) => {
                if let Some((root, options)) = self.workspace.connected() {
                    self.status = "SSH connected".into();
                    let restore = self.startup_restore.take();
                    if self.vaults.recovery_required() {
                        self.encrypted_dialog();
                        self.vaults.reconnected();
                    } else {
                        self.prepare_project(root, options, true, true, restore, ctx);
                    }
                }
            }
            Err(error) => self.ssh_failed(error, ctx),
        }
    }

    /// Keep the login's parsed output while the normal detach path stops its
    /// process and clears remote bindings. This view owns no SSH resources and
    /// can accept final reader bytes after the child-exit notification arrives.
    pub(super) fn ssh_failed(&mut self, message: String, ctx: &egui::Context) {
        let output = self.ssh_master.as_ref().map(|terminal| {
            let host = self
                .workspace
                .as_ref()
                .and_then(|w| w.remote.as_ref())
                .map(|remote| remote.profile.endpoint())
                .unwrap_or_default();
            terminal.output_view(format!("SSH ERROR / {host}"))
        });
        self.detach_ssh(ctx);
        self.status = "SSH connection failed · local mode".into();
        if let Some(output) = output {
            output.set_visible(true);
            self.terminal = Some(output);
            self.show_terminal = true;
            self.terminal_intent();
            self.notify(format!("{message} SSH login output is retained in the terminal. Choose New for a local shell."), true);
        } else {
            self.notify(message, true);
        }
    }

    /// Allow recovery binding only when all tabs are unbound and the explicit profile matches
    /// the previous connection/project identity.
    pub(super) fn can_resume_ssh(&self, profile: &Profile) -> bool {
        self.workspace
            .can_resume(profile, self.documents.all_unbound())
    }
    /// Retaining bindings requires the current host identity or its matching recovery record.
    pub(super) fn can_reconnect_ssh(&self, profile: &Profile) -> bool {
        self.workspace
            .can_reconnect(profile, self.documents.all_unbound())
    }
    /// Reconnect the selected/last profile, using common dirty-buffer guards when local
    /// state cannot safely be treated as recovery for that same connection.
    pub(super) fn reconnect_ssh(&mut self, ctx: &egui::Context) {
        let profile = self.workspace.reconnect_profile().cloned();
        if let Some(profile) = profile {
            if self.can_reconnect_ssh(&profile) {
                self.guarded(Action::Reconnect(Box::new(profile)), ctx);
            } else {
                // Local paths must never carry over into an SSH workspace.
                self.guarded(Action::Remote(Box::new(profile)), ctx);
            }
        } else {
            self.set_dialog(Dialog::Connect);
        }
    }
    /// Unbind remote paths before invalidating host work and dropping transport ownership;
    /// retain buffers and recovery metadata for Save As or an explicit matching reconnect.
    pub(super) fn detach_ssh(&mut self, ctx: &egui::Context) {
        let Some(connection) = self.workspace.remember_disconnect() else {
            return;
        };
        self.documents
            .detach_connection(&connection, self.vaults.has_active());
        self.workspace.cancel_connection();
        self.startup_restore = None;
        // Drop views before the master; persistent services retain their own jobs.
        // A lost encrypted lease still requires explicit unlock/lock recovery.
        let encrypted_lease = self.vaults.retain_for_disconnect();
        self.clear_project(
            documents::Retention::Keep,
            workspace::LeaveReason::Disconnected,
        );
        self.vaults.disconnected(encrypted_lease);
        self.release_workspace_transport();
        self.invalidate_project_candidate(workspace::PickerReset::Close);
        self.dialog_input.clear();
        self.cancel_guarded_action();
        self.dialog = Dialog::None;
        self.show_terminal = false;
        self.status = "SSH disconnected · local mode".into();
        self.focus_editor(ctx);
    }
    /// Restore bound paths/revisions only for recovery records matching the prepared connection identity.
    pub(super) fn restore_ssh_buffers(&mut self) {
        let Some((remote, connection)) = self.workspace.restoration_binding() else {
            return;
        };
        let encrypted_ready = self
            .vaults
            .can_restore_bindings(&remote.profile.root, remote);
        self.documents
            .restore_connection(&connection, encrypted_ready);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_shortcut_requests_connection_and_preserves_the_unsaved_buffer_guard() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].text = "unsaved local edits".into();
        let profile = Profile {
            name: "Saved connection shortcut".into(),
            host: "fixture.invalid".into(),
            user: "researcher".into(),
            port: "2222".into(),
            root: "/scratch/project".into(),
            identity: "/fixture/key".into(),
            config: "/fixture/config".into(),
            jump: "fixture-jump".into(),
            ..Default::default()
        };
        app.connection_profiles.profiles = vec![profile.clone()];
        // Replacing an unrelated edited form must also replace its save identity,
        // or a later Save could rename the wrong profile.
        app.connection_profiles.baseline = Some(Profile {
            name: "Other profile".into(),
            host: "other.invalid".into(),
            ..Default::default()
        });
        let mut frame = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 760.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    app.explorer(ctx);
                    egui::CentralPanel::default().show(ctx, |_| {});
                },
            )
        };
        let output = frame(vec![]);
        let point = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == profile.name => {
                    Some(text.pos + text.galley.rect.center().to_vec2())
                }
                _ => None,
            })
            .expect("saved connection shortcut");
        let pointer = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(point), pointer(true)]);
        frame(vec![pointer(false)]);
        assert!(app.dialog == Dialog::None);
        let Some(Action::Remote(requested)) = app.pending.as_ref() else {
            panic!("shortcut must request a connection, not open connection settings");
        };
        assert_eq!(
            serde_json::to_value(requested).unwrap(),
            serde_json::to_value(&profile).unwrap()
        );
        assert_eq!(app.documents[0].text, "unsaved local edits");
        assert!(app.workspace.is_none());
        assert_eq!(
            app.connection_profiles.baseline.as_ref().unwrap().label(),
            profile.label()
        );
    }

    fn remote_buffer() -> (EditorApp, egui::Context) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let mut workspace = crate::app::tests::remote_workspace();
        workspace.remote.as_mut().unwrap().profile.root = workspace.root.clone();
        app.workspace.set_fixture(Some(workspace));
        app.new_file();
        let doc = &mut app.documents[0];
        doc.path = Some("/scratch/researcher/project/main.rs".into());
        doc.revision = Some("original remote revision".into());
        doc.text = "retained edits".into();
        doc.set_version_fixture(7);
        (app, ctx)
    }

    fn remembered_workspace(app: &EditorApp) -> Workspace {
        let mut workspace = crate::app::tests::remote_workspace();
        workspace.remote.as_mut().unwrap().profile = app
            .workspace
            .as_ref()
            .unwrap()
            .remote
            .as_ref()
            .unwrap()
            .profile
            .clone();
        workspace
    }

    fn finish_files(app: &mut EditorApp, ctx: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.documents.iter().any(|doc| doc.saving) || app.documents.has_pending_opens() {
            assert!(Instant::now() < deadline, "file operation did not finish");
            app.poll(ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn prepared_ssh_connections_focus_chat_and_hide_the_explorer() {
        for ready in [false, true] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            let mut workspace = crate::app::tests::remote_workspace();
            workspace.ready = ready;
            app.workspace.set_fixture(Some(workspace));
            app.show_terminal = true;
            app.terminal_intent();
            Terminal::focus(&ctx);
            let project = tempfile::tempdir().unwrap();
            let response = files::local(&Request::Prepare {
                path: project.path().to_string_lossy().into(),
                sandbox: Default::default(),
            })
            .unwrap();
            app.commit_project(response, true, Default::default(), true, None, &ctx);
            assert!(app.project_ready());
            assert!(app.session_busy(), "The background shell still starts");
            assert_eq!(app.show_explorer, ready);
            assert_eq!(app.panel_visible(Panel::Agent), !ready);
            assert_eq!(app.terminal_visible(), ready);
            if !ready {
                let render = |app: &mut EditorApp| {
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(1000., 760.),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            app.navigation.begin_layout();
                            app.explorer(ctx);
                            app.workspace_content(ctx);
                        },
                    );
                };
                render(&mut app);
                assert_eq!(
                    ctx.memory(|memory| memory.focused()),
                    Some(egui::Id::new("agent-composer"))
                );
                assert!(app.navigation.rect(navigation::Pane::Explorer).is_none());
                assert!(!app.agents.chat.busy);

                // A shell can attach before a second chat frame remembers its child focus.
                let plan = &app.workspace.as_ref().unwrap().plan;
                let policy = crate::sessions::fingerprint(plan);
                let info = serde_json::from_value(serde_json::json!({
                    "id": "background-shell", "name": "Shell", "root": plan.root,
                    "policy": policy, "mode": plan.label(), "alive": true,
                    "clients": 1, "created": 0, "exit_code": null,
                    "persistent": false, "allocation": null
                }))
                .unwrap();
                let (terminal, _stream) = Terminal::stream(&ctx, "Shell".into(), |_| {});
                app.add_terminal_view(terminal, "background-shell".into(), "Shell".into());
                app.handle_session(
                    sessions::Job::Create {
                        persistent: false,
                        policy,
                    },
                    Ok(crate::project::Response::Sessions(crate::sessions::Reply {
                        response: rode_session::Response::Session(info),
                        executable: String::new(),
                        prefix: vec![],
                    })),
                    &ctx,
                );
                render(&mut app);
                assert!(app.panel_visible(Panel::Agent));
                assert!(!app.terminal_visible());
                assert_eq!(
                    ctx.memory(|memory| memory.focused()),
                    Some(egui::Id::new("agent-composer"))
                );
            }
        }
    }

    #[test]
    fn a_dead_master_clears_the_host_and_allows_local_file_access() {
        let (mut app, ctx) = remote_buffer();
        let (master, stream) = Terminal::stream(&ctx, "SSH fixture".into(), |_| {});
        app.ssh_master = Some(master);
        app.last_window_title = "RODE [SSH old host]".into();
        let generation = app.workers.generation();
        stream.end();
        app.poll(&ctx);
        assert!(app.workspace.is_none());
        assert!(app.workers.generation() > generation);
        assert!(app.ssh_master.is_none());
        assert!(app.documents[0].path.is_none());
        app.workspace_banner(&ctx);
        assert_eq!(app.last_window_title, "RODE");
        app.set_dialog(Dialog::Folder);
        assert!(!app.project_picker.remote);
        app.set_dialog(Dialog::None);

        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("local.txt").to_string_lossy().into_owned();
        std::fs::write(&local, "local source").unwrap();
        app.open_file(local.clone(), &ctx);
        finish_files(&mut app, &ctx);
        assert_eq!(app.documents.len(), 2);
        assert_eq!(app.documents[1].path.as_deref(), Some(local.as_str()));
        assert_eq!(app.documents[1].text, "local source");
        assert_eq!(app.documents[0].text, "retained edits");
        // Opening a local file without a project must also prevent its path
        // from being rebound to the last SSH host during reconnect.
        assert!(!app.can_resume_ssh(app.workspace.reconnect_profile().unwrap()));
        app.reconnect_ssh(&ctx);
        assert!(matches!(app.pending.as_ref(), Some(Action::Remote(_))));
        assert!(app.workspace.is_none());
    }

    #[test]
    fn failed_authentication_returns_to_local_mode_and_ignores_late_replies() {
        let (mut app, ctx) = remote_buffer();
        app.workspace.as_mut().unwrap().connected = false;
        let (master, stream) = Terminal::stream(&ctx, "SSH login".into(), |_| {});
        app.ssh_master = Some(master);
        let generation = app.workers.generation();
        app.workers
            .workspace_at(&ctx, generation)
            .send(WorkspaceEvent::Connected {
                result: Err("fixture authentication failed".into()),
            })
            .unwrap();
        app.poll(&ctx);
        // Feed after cleanup, exercising the PTY-exit/read race explicitly.
        stream
            .feed(b"Permission denied (publickey).\r\n".to_vec())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !app
            .terminal
            .as_ref()
            .unwrap()
            .contents()
            .contains("Permission denied")
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.workspace.is_none());
        assert!(app.ssh_master.is_none());
        assert!(app.show_terminal);
        assert!(!app.terminal.as_ref().unwrap().is_alive());
        assert!(
            app.terminal
                .as_ref()
                .unwrap()
                .title
                .starts_with("SSH ERROR")
        );
        assert_eq!(app.documents[0].text, "retained edits");
        app.workspace_banner(&ctx);
        assert_eq!(app.last_window_title, "RODE");
        assert!(app.documents[0].path.is_none());
        assert!(
            app.notice
                .as_ref()
                .unwrap()
                .0
                .contains("authentication failed")
        );
        app.workers
            .workspace_at(&ctx, generation)
            .send(WorkspaceEvent::Connected { result: Ok(()) })
            .unwrap();
        app.poll(&ctx);
        assert!(app.workspace.is_none());
        assert!(!app.workspace.preparing());
    }

    #[test]
    fn a_real_fast_ssh_failure_retains_the_error_in_local_mode() {
        if sandbox::find_program("ssh").is_none() {
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("ssh_config");
        std::fs::write(&config, format!(
            "Host failed-login\n HostName 127.0.0.1\n Port {port}\n BatchMode yes\n ConnectTimeout 2\n ConnectionAttempts 1\n"
        )).unwrap();
        drop(listener);
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.connect(
            Profile {
                host: "failed-login".into(),
                backend: crate::ssh::Backend::OpenSsh,
                config: config.to_string_lossy().into(),
                ..Default::default()
            },
            &ctx,
            false,
        );
        let deadline = Instant::now() + Duration::from_secs(8);
        while app.workspace.is_some()
            || app
                .terminal
                .as_ref()
                .is_none_or(|t| !t.contents().contains("Connection refused"))
        {
            app.poll(&ctx);
            assert!(
                Instant::now() < deadline,
                "SSH failure was not retained: {:?}",
                app.notice.as_ref().map(|n| &n.0)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.ssh_master.is_none());
        assert!(app.show_terminal);
        assert!(!app.terminal.as_ref().unwrap().is_alive());
        app.set_dialog(Dialog::Folder);
        assert!(!app.project_picker.remote);
        app.set_dialog(Dialog::None);
        app.workspace_banner(&ctx);
        assert_eq!(app.last_window_title, "RODE");
    }

    #[test]
    fn disconnected_save_cannot_overwrite_a_matching_local_path() {
        let (mut app, ctx) = remote_buffer();
        let temp = tempfile::tempdir().unwrap();
        let original = temp
            .path()
            .join("same-path.txt")
            .to_string_lossy()
            .into_owned();
        let copy = temp.path().join("copy.txt").to_string_lossy().into_owned();
        std::fs::write(&original, "unrelated local source").unwrap();
        app.documents[0].path = Some(original.clone());
        let id = app.documents[0].id;
        app.disconnect_ssh(&ctx);
        app.save_document(id, original.clone(), false, &ctx);
        assert!(!app.documents[0].saving);
        assert_eq!(
            std::fs::read_to_string(original).unwrap(),
            "unrelated local source"
        );
        app.save_document(id, copy.clone(), true, &ctx);
        finish_files(&mut app, &ctx);
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "retained edits");
        assert_eq!(app.documents[0].path.as_deref(), Some(copy.as_str()));
        assert!(!app.documents[0].is_recovery());
        assert!(!app.documents[0].dirty());
        assert!(!app.can_resume_ssh(app.workspace.reconnect_profile().unwrap()));
    }

    #[test]
    fn only_the_same_ready_ssh_project_restores_buffer_bindings_and_revisions() {
        let (mut app, ctx) = remote_buffer();
        let mut workspace = remembered_workspace(&app);
        app.disconnect_ssh(&ctx);
        assert!(app.can_resume_ssh(app.workspace.reconnect_profile().unwrap()));
        let original_host = workspace.remote.as_ref().unwrap().profile.host.clone();
        workspace.remote.as_mut().unwrap().profile.host = "different-host".into();
        app.workspace.set_fixture(Some(workspace));
        app.restore_ssh_buffers();
        assert!(app.documents[0].path.is_none());
        app.workspace
            .as_mut()
            .unwrap()
            .remote
            .as_mut()
            .unwrap()
            .profile
            .host = original_host;
        let original_root = app.workspace.as_ref().unwrap().root.clone();
        app.workspace
            .as_mut()
            .unwrap()
            .remote
            .as_mut()
            .unwrap()
            .profile
            .root = "/other/project".into();
        app.restore_ssh_buffers();
        assert!(app.documents[0].path.is_none());
        app.workspace
            .as_mut()
            .unwrap()
            .remote
            .as_mut()
            .unwrap()
            .profile
            .root = original_root;
        app.workspace.as_mut().unwrap().ready = false;
        app.restore_ssh_buffers();
        assert!(app.documents[0].path.is_none());
        app.workspace.as_mut().unwrap().ready = true;
        app.restore_ssh_buffers();
        let doc = &app.documents[0];
        assert_eq!(
            doc.path.as_deref(),
            Some("/scratch/researcher/project/main.rs")
        );
        assert_eq!(doc.revision.as_deref(), Some("original remote revision"));
        assert!(!doc.is_recovery());
        assert_eq!(doc.text, "retained edits");
        assert!(doc.dirty());
        assert_eq!(doc.version(), 7);
    }

    #[test]
    fn encrypted_buffers_require_a_verified_lease_before_restoring_paths() {
        let (mut app, ctx) = remote_buffer();
        let workspace = remembered_workspace(&app);
        app.documents.detach_connection(
            &workspace::ConnectionId::for_profile(&workspace.remote.as_ref().unwrap().profile),
            true,
        );
        app.disconnect_ssh(&ctx);
        app.workspace.set_fixture(Some(workspace));
        app.restore_ssh_buffers();
        assert!(app.documents[0].path.is_none());
        assert!(app.documents[0].is_recovery());
    }
}

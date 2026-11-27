//! Losing a transport restores the desktop CWD. Retained remote buffers have no
//! writable path until Save As or an explicit reconnect to the same project.
//! Explicit disconnect closes all tabs after the common unsaved-change decision.
//!
//! Disconnect invalidates host work and unbinds retained documents before restoring local
//! opening in Normal mode. Reconnect restores paths/revisions only for the same explicitly selected
//! host/project after preparation; recovery metadata alone never authorizes a save.
use super::*;

mod view;

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
            self.prepare_ssh_recovery();
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
                self.status = "Connecting · open Terminal for authentication prompts".into();
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
            }
            Err(error) => {
                self.ssh_failed(format!("{error:#}"), ctx);
            }
        }
    }

    /// Release views bound to the replaced SSH transport while retaining recovery
    /// bindings. Independent host tools reconcile their own selected connections;
    /// reconnecting a project must not stop a local monitor.
    fn prepare_ssh_recovery(&mut self) {
        self.leave_debugger();
        self.stop_latex();
        self.workspace.cancel_connection();
        self.terminal = None;
        self.release_workspace_transport();
        self.workers.invalidate_workspace();
        self.explorer_state.loading.clear();
        self.documents.reconnecting();
    }

    /// Confirm unsaved changes and await saves/file/lifetime acknowledgements before closing
    /// all tabs on explicit disconnect. Encrypted leases retain their lock guard.
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
                        self.prepare_project(
                            root,
                            options,
                            true,
                            true,
                            restore.map_or(
                                workspace::PreparationPolicy::Given,
                                workspace::PreparationPolicy::Saved,
                            ),
                            ctx,
                        );
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
        self.detach_ssh(documents::Retention::Keep, ctx);
        self.status = "SSH connection failed · local mode".into();
        if let Some(output) = output {
            output.set_visible(true);
            self.terminal = Some(output);
            self.show_terminal = true;
            self.notify(format!("{message} Open Terminal to review the retained SSH login output. Choose New for a local shell."), true);
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
    /// Release transport ownership after invalidating host work. Guarded explicit disconnect
    /// discards tabs; unexpected loss unbinds and retains buffers for Save As or reconnect.
    pub(super) fn detach_ssh(&mut self, retention: documents::Retention, ctx: &egui::Context) {
        let Some(connection) = self.workspace.remember_disconnect() else {
            return;
        };
        if retention == documents::Retention::Keep {
            self.documents
                .detach_connection(&connection, self.vaults.has_active());
        }
        self.workspace.cancel_connection();
        self.startup_restore = None;
        // Drop views before the master; persistent services retain their own jobs.
        // A lost encrypted lease still requires explicit unlock/lock recovery.
        let encrypted_lease = self.vaults.retain_for_disconnect();
        self.clear_project(retention, workspace::LeaveReason::Disconnected);
        self.vaults.disconnected(encrypted_lease);
        self.release_workspace_transport();
        self.invalidate_project_candidate(workspace::PickerReset::Close);
        self.dialog_input.clear();
        self.cancel_guarded_action();
        self.dialog = Dialog::None;
        self.workspace.request_local_fallback();
        self.status = "SSH disconnected · local mode".into();
        ctx.request_repaint();
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
    fn ssh_recovery_stops_only_monitoring_bound_to_the_replaced_connection() {
        for remote_monitor in [false, true] {
            let ctx = egui::Context::default();
            let mut app = EditorApp::new(
                &eframe::CreationContext::_new_kittest(ctx.clone()),
                Startup::default(),
            );
            if !remote_monitor {
                app.initialize_host_tool(Panel::Monitor);
            }
            app.workspace
                .set_fixture(Some(crate::app::tests::remote_workspace()));
            app.initialize_host_tool(Panel::Monitor);
            app.host_tools.monitoring.set_active(true);
            app.host_tools.monitoring.record(
                crate::project::Monitor::default(),
                Instant::now(),
                Duration::from_secs(2),
            );
            app.prepare_ssh_recovery();
            app.poll_host_tools(&ctx);
            assert_eq!(app.host_tools.monitoring.active, !remote_monitor);
            assert_eq!(app.host_tools.monitoring.history.len(), 1);
        }
    }

    #[test]
    fn sidebar_shortcut_requests_connection_and_preserves_the_unsaved_buffer_guard() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.show_connections(&ctx);
        app.documents[0].fixture_content().text = "unsaved local edits".into();
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
                    app.workspace_sidebar(ctx);
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
        doc.fixture_content().path = Some("/scratch/researcher/project/main.rs".into());
        doc.fixture_content().revision = Some("original remote revision".into());
        doc.fixture_content().text = "retained edits".into();
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
    fn transport_loss_restores_normal_cwd_without_rebinding_remote_edits_or_saved_grants() {
        let (mut app, ctx) = remote_buffer();
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        let profile = app.workspace.reconnect_profile().unwrap().clone();
        app.sandbox_defaults.enabled = true;
        app.sandbox_defaults.read_only = vec!["invalid relative grant".into()];
        app.detach_ssh(documents::Retention::Keep, &ctx);
        app.restore_local_workspace(&ctx);
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.workspace.preparing() {
            app.poll(&ctx);
            assert!(
                Instant::now() < deadline,
                "Local fallback did not finish: {:?}",
                app.notice
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let workspace = app.workspace.as_ref().unwrap();
        assert_eq!(workspace.root, cwd.to_string_lossy());
        assert!(workspace.ready && workspace.connected && workspace.remote.is_none());
        assert!(!workspace.plan.active && !workspace.plan.requested);
        assert_eq!(workspace.sandbox_options, sandbox::Options::default());
        assert!(
            app.explorer_state
                .directories
                .contains_key(cwd.to_str().unwrap())
        );
        assert_eq!(app.documents[0].text, "retained edits");
        assert_eq!(app.documents[0].version(), 7);
        assert!(app.documents[0].is_recovery());
        assert!(app.documents[0].path.is_none() && app.documents[0].revision.is_none());
        assert!(app.can_resume_ssh(&profile));
        app.save_document(
            app.documents[0].id,
            "/must-not-write-local.rs".into(),
            false,
            &ctx,
        );
        assert!(!app.documents[0].saving);
    }

    #[test]
    fn connection_preparation_late_shells_and_disconnect_preserve_arrangement() {
        use areas::Area;
        use sidebar::View;
        let arrangement = |app: &EditorApp| {
            (
                app.presentation.area,
                app.presentation.sidebar,
                app.presentation.surface,
                app.presentation.panel,
                app.presentation.last_tool,
                app.presentation.detached,
                app.presentation.tools_hidden,
                app.presentation.floating_tabs.clone(),
                app.navigation.zoom,
            )
        };
        for (area, sidebar, tool, detached) in [
            (Area::Welcome, None, None, false),
            (Area::Projects, Some(View::Explorer), None, false),
            (
                Area::Projects,
                Some(View::Connections),
                Some(ToolTab::Panel(Panel::Agent)),
                false,
            ),
            (
                Area::Projects,
                Some(View::Explorer),
                Some(ToolTab::Terminal),
                false,
            ),
            (
                Area::Monitor,
                Some(View::Connections),
                Some(ToolTab::Panel(Panel::Problems)),
                true,
            ),
        ] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.new_file();
            let mut workspace = crate::app::tests::remote_workspace();
            workspace.ready = false;
            app.workspace.set_fixture(Some(workspace));
            app.show_terminal = true;
            if let Some(tool) = tool {
                match tool {
                    ToolTab::Terminal => app.terminal_intent(),
                    ToolTab::Panel(panel) => app.presentation.select_panel(panel),
                    _ => unreachable!(),
                }
            }
            match sidebar {
                Some(View::Explorer) => app.presentation.explorer(true),
                Some(View::Connections) => app.presentation.show_connections(),
                None => app.presentation.hide_sidebar(),
            }
            app.presentation.enter_area(area);
            if detached {
                app.presentation.detach_tools();
                app.presentation.hide_tools();
                app.detach_tab(ToolTab::Panel(Panel::Git), &ctx);
            }
            if sidebar == Some(View::Connections) {
                app.navigation.zoom = Some(navigation::Pane::Connections);
            }
            let before = arrangement(&app);
            let project = tempfile::tempdir().unwrap();
            let response = files::local(&Request::Prepare {
                path: project.path().to_string_lossy().into(),
                sandbox: Default::default(),
            })
            .unwrap();
            app.commit_project(response, true, Default::default(), true, None, &ctx);
            assert!(app.project_ready());
            assert!(app.session_busy(), "The background shell still starts");
            assert_eq!(arrangement(&app), before);

            // Delayed shell creation must not replay navigation or reset zoom.
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
            assert_eq!(arrangement(&app), before);
            assert!(!app.documents.is_empty());
            app.finish_project_close(
                workspace::CloseTarget::Disconnect,
                workspace::LeaveReason::Disconnected,
                &ctx,
            );
            assert!(app.documents.is_empty());
            assert_eq!(arrangement(&app), before);
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
        app.update_window_title(&ctx);
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
        app.update_window_title(&ctx);
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
        app.presentation.explorer(true);
        app.presentation.select_panel(Panel::Problems);
        let arrangement = (
            app.presentation.area,
            app.presentation.sidebar,
            app.presentation.surface,
            app.presentation.panel,
        );
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
        assert_eq!(
            (
                app.presentation.area,
                app.presentation.sidebar,
                app.presentation.surface,
                app.presentation.panel,
            ),
            arrangement
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
        assert_eq!(
            (
                app.presentation.area,
                app.presentation.sidebar,
                app.presentation.surface,
                app.presentation.panel,
            ),
            arrangement
        );
        app.set_dialog(Dialog::Folder);
        assert!(!app.project_picker.remote);
        app.set_dialog(Dialog::None);
        app.update_window_title(&ctx);
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
        app.documents[0].fixture_content().path = Some(original.clone());
        let id = app.documents[0].id;
        app.detach_ssh(documents::Retention::Keep, &ctx);
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
        app.detach_ssh(documents::Retention::Keep, &ctx);
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
        app.detach_ssh(documents::Retention::Keep, &ctx);
        app.workspace.set_fixture(Some(workspace));
        app.restore_ssh_buffers();
        assert!(app.documents[0].path.is_none());
        assert!(app.documents[0].is_recovery());
    }
}

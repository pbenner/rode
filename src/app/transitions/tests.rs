//! Cross-owner regressions use real guards, worker admission and native TextEdit.
use super::*;

fn fixture() -> (EditorApp, egui::Context) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    (EditorApp::new(&cc, Startup::default()), ctx)
}

#[test]
fn reopening_a_disconnected_ssh_bookmark_retains_edits_without_a_discard_decision() {
    let (mut app, ctx) = fixture();
    let mut workspace = crate::app::tests::remote_workspace();
    workspace.remote.as_mut().unwrap().profile.root = workspace.root.clone();
    app.workspace.set_fixture(Some(workspace));
    app.new_file();
    app.documents
        .active_mut()
        .unwrap()
        .set_source("retained remote edits".into());
    let bookmark = app.bookmark_candidate().unwrap();
    app.detach_ssh(documents::Retention::Keep, &ctx);
    assert!(app.workspace.is_none());
    assert_eq!(
        app.documents.active().unwrap().text,
        "retained remote edits"
    );
    let action = Action::Bookmark(Box::new(bookmark));
    let (policy, changes) = app.transition_facts(&action, &ctx);
    assert!(policy.preserve_buffers);
    assert!(!changes.dirty_versions.is_empty());
    let mut controller = Controller::default();
    assert!(matches!(
        controller.request(action, policy, changes, vec![]),
        Effect::Execute(Action::Bookmark(_))
    ));
}
fn source(app: &mut EditorApp) -> u64 {
    app.new_file();
    let doc = app.documents.active_mut().unwrap();
    doc.acknowledge_save("source.txt".into(), "".into(), "revision".into());
    doc.id
}

#[test]
fn closing_a_project_guards_every_editor_then_returns_to_a_neutral_context() {
    for remote in [false, true] {
        let (mut app, ctx) = fixture();
        let mut workspace = crate::app::tests::remote_workspace();
        if !remote {
            workspace.remote = None;
        }
        app.workspace.set_fixture(Some(workspace));
        source(&mut app);
        let child = app.allocate_editor_window();
        app.in_editor(child, |app| {
            app.new_file();
            app.documents
                .active_mut()
                .unwrap()
                .set_source("unsaved child source".into());
        });
        app.guarded(Action::CloseProject, &ctx);
        let (decision, _) = app.pending.presentation().unwrap();
        app.cancel_guarded_decision(decision);
        assert!(app.workspace.project_open());
        assert_eq!(app.documents.len(), 2);
        app.guarded(Action::CloseProject, &ctx);
        let (decision, _) = app.pending.presentation().unwrap();
        app.discard_guarded_action(decision, &ctx);
        assert!(app.documents.is_empty());
        assert!(!app.workspace.project_open());
        assert_eq!(app.presentation.area, areas::Area::Welcome);
        assert!(!app.explorer_visible(egui::ViewportId::ROOT));
        if remote {
            assert!(app.workspace.as_ref().unwrap().connected);
            assert!(app.workspace.as_ref().unwrap().remote.is_some());
        } else {
            assert!(app.workspace.is_none());
            assert!(app.workspace.take_local_fallback());
        }
    }
}

#[test]
fn failed_saves_and_in_flight_saves_keep_a_project_open_until_acknowledged() {
    for success in [false, true] {
        let (mut app, ctx) = fixture();
        app.workspace
            .set_fixture(Some(crate::app::tests::remote_workspace()));
        let id = source(&mut app);
        app.documents[0].set_source("saved before close".into());
        app.documents[0].fixture_content().saving = true;
        app.guarded(Action::CloseProject, &ctx);
        assert!(app.pending.waiting_for_save());
        assert!(app.workspace.project_open());
        app.workers
            .workspace(&ctx)
            .send(WorkspaceEvent::Done {
                job: Job::Save {
                    id,
                    text: "saved before close".into(),
                },
                result: Box::new(if success {
                    Ok(Response {
                        path: "source.txt".into(),
                        revision: "new".into(),
                        ..Default::default()
                    })
                } else {
                    Err("Save refused".into())
                }),
            })
            .unwrap();
        app.poll(&ctx);
        app.advance_guarded_action(&ctx);
        assert_eq!(app.workspace.project_open(), !success);
        assert_eq!(app.documents.is_empty(), success);
        assert!(app.workspace.as_ref().unwrap().connected);
    }
}
fn frame(
    app: &mut EditorApp,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    action: Option<Action>,
) {
    let mut action = action;
    let _ = ctx.run(
        egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 700.),
            )),
            ..Default::default()
        },
        |ctx| {
            app.route_input(ctx);
            // A menu/picker may request this action after routing but before source rendering.
            if let Some(action) = action.take() {
                app.guarded(action, ctx);
            }
            app.editor(ctx);
            app.handle_close_request(ctx);
        },
    );
}
fn prepared(
    app: &mut EditorApp,
    ctx: &egui::Context,
    path: &std::path::Path,
) -> workspace::PreparationId {
    let token = app.workspace.begin_preparation();
    let versions = app.documents.versions();
    let response = files::local(&Request::Prepare {
        path: path.to_string_lossy().into(),
        sandbox: Default::default(),
    })
    .unwrap();
    app.workers
        .workspace(ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Prepare {
                token,
                remote: false,
                options: Default::default(),
                keep_buffers: false,
                documents_at_start: versions,
                policy: workspace::PreparationPolicy::Given,
                open_file: None,
            },
            result: Box::new(Ok(response)),
        })
        .unwrap();
    app.poll(ctx);
    token
}

#[test]
fn menu_close_and_quit_apply_same_frame_edits_before_discard_admission() {
    for quit in [false, true] {
        let (mut app, ctx) = fixture();
        let id = source(&mut app);
        frame(&mut app, &ctx, vec![], None);
        frame(&mut app, &ctx, vec![], None);
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::Text("accepted edit".into())],
            Some(if quit {
                Action::Quit
            } else {
                Action::CloseTab(id)
            }),
        );
        assert_eq!(app.documents[0].text, "accepted edit");
        assert!(
            app.pending
                .presentation()
                .is_some_and(|(_, wait)| wait.is_none())
        );
        assert!(!app.exit.closing());
    }
}

#[test]
fn a_nonmodal_quit_intent_preserves_input_on_both_sides_of_a_document_handoff() {
    let (mut app, ctx) = fixture();
    source(&mut app);
    source(&mut app);
    frame(&mut app, &ctx, vec![], None);
    frame(&mut app, &ctx, vec![], None);
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::Text("before".into()),
            egui::Event::Key {
                key: Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL,
            },
            egui::Event::Text("after".into()),
        ],
        Some(Action::Quit),
    );
    assert!(app.pending.is_some());
    assert!(
        !app.pending.modal(),
        "A modal would prevent the queued input acknowledgement"
    );
    for _ in 0..8 {
        frame(&mut app, &ctx, vec![], None);
        if !app.editor.input.pending() {
            break;
        }
    }
    assert_eq!(app.documents[0].text, "after");
    assert_eq!(app.documents[1].text, "before");
    assert!(app.pending.modal());
    assert!(!app.exit.closing());
}

#[test]
fn discard_decisions_are_bound_to_the_visible_versions_and_current_intent() {
    let (mut app, ctx) = fixture();
    let id = source(&mut app);
    app.documents[0].fixture_content().text = "first edit".into();
    app.documents[0].set_dirty_fixture(true);
    app.guarded(Action::CloseTab(id), &ctx);
    let (old, _) = app.pending.presentation().unwrap();
    app.documents[0].fixture_content().text = "new worker edit".into();
    app.documents[0].advance_version_fixture();
    app.discard_guarded_action(old, &ctx);
    let (current, _) = app.pending.presentation().unwrap();
    assert_ne!(old, current);
    assert_eq!(app.documents[0].text, "new worker edit");
    app.cancel_guarded_decision(old);
    assert!(app.pending.modal());
    app.guarded(Action::Quit, &ctx);
    app.discard_guarded_action(current, &ctx);
    assert!(matches!(app.pending.as_ref(), Some(Action::Quit)));
    assert!(!app.exit.closing());
}

#[test]
fn a_clean_save_finishes_before_a_project_request_can_capture_its_workspace() {
    let project = tempfile::tempdir().unwrap();
    let (mut app, ctx) = fixture();
    let id = source(&mut app);
    app.documents[0].fixture_content().saving = true;
    app.guarded(
        Action::Project {
            path: project.path().to_string_lossy().into(),
            remote: false,
            options: Default::default(),
        },
        &ctx,
    );
    assert!(app.pending.waiting_for_save());
    assert!(!app.workspace.preparing());
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Save {
                id,
                text: String::new(),
            },
            result: Box::new(Ok(Response {
                path: project.path().join("source.txt").to_string_lossy().into(),
                revision: "saved".into(),
                ..Default::default()
            })),
        })
        .unwrap();
    app.poll(&ctx);
    app.advance_guarded_action(&ctx);
    assert!(app.workspace.preparing());
    assert_eq!(app.documents[0].revision.as_deref(), Some("saved"));
}

#[test]
fn failed_save_cancels_a_project_intent_instead_of_invalidating_its_error() {
    let project = tempfile::tempdir().unwrap();
    let (mut app, ctx) = fixture();
    let id = source(&mut app);
    app.documents[0].fixture_content().saving = true;
    app.guarded(
        Action::Project {
            path: project.path().to_string_lossy().into(),
            remote: false,
            options: Default::default(),
        },
        &ctx,
    );
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Save {
                id,
                text: String::new(),
            },
            result: Box::new(Err("save refused".into())),
        })
        .unwrap();
    app.poll(&ctx);
    app.advance_guarded_action(&ctx);
    assert!(app.pending.is_none());
    assert!(!app.workspace.preparing());
    assert!(app.notice.as_ref().unwrap().0.contains("save refused"));
}

#[test]
fn prepared_projects_recheck_lifetime_work_and_edits_received_during_the_wait() {
    let project = tempfile::tempdir().unwrap();
    let (mut app, ctx) = fixture();
    source(&mut app);
    app.sessions.persistence_pending_fixture(true);
    let token = prepared(&mut app, &ctx, project.path());
    assert!(app.workspace.prepared(token).is_some());
    assert!(app.workspace.is_none());
    app.documents[0].fixture_content().text = "late edit".into();
    app.documents[0].set_dirty_fixture(true);
    app.documents[0].advance_version_fixture();
    app.sessions.persistence_pending_fixture(false);
    app.advance_guarded_action(&ctx);
    let (decision, wait) = app.pending.presentation().unwrap();
    assert!(wait.is_none());
    assert_eq!(app.documents[0].text, "late edit");
    app.discard_guarded_action(decision, &ctx);
    assert_eq!(
        app.workspace.as_ref().unwrap().root,
        project.path().to_string_lossy()
    );
    assert!(app.documents.is_empty());
}

#[test]
fn a_failed_lifetime_acknowledgement_cancels_and_releases_a_prepared_project() {
    let project = tempfile::tempdir().unwrap();
    let (mut app, ctx) = fixture();
    app.sessions.persistence_pending_fixture(true);
    let token = prepared(&mut app, &ctx, project.path());
    app.handle_session(
        sessions::Job::Persistence,
        Err("persistence refused".into()),
        &ctx,
    );
    app.advance_guarded_action(&ctx);
    assert!(app.pending.is_none());
    assert!(app.workspace.prepared(token).is_none());
    assert!(app.workspace.is_none());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .0
            .contains("persistence refused")
    );
}

#[test]
fn forced_disconnect_cancels_save_waits_and_rejects_late_acknowledgements() {
    let (mut app, ctx) = fixture();
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    let id = source(&mut app);
    app.documents[0].fixture_content().text = "keep this edit".into();
    app.documents[0].set_dirty_fixture(true);
    app.documents[0].fixture_content().saving = true;
    app.guarded(Action::Quit, &ctx);
    let (old, _) = app.pending.presentation().unwrap();
    let generation = app.workers.generation();
    app.detach_ssh(documents::Retention::Keep, &ctx);
    app.workers
        .workspace_at(&ctx, generation)
        .send(WorkspaceEvent::Done {
            job: Job::Save {
                id,
                text: "old submission".into(),
            },
            result: Box::new(Ok(Response {
                path: "old-host.txt".into(),
                revision: "stale".into(),
                ..Default::default()
            })),
        })
        .unwrap();
    app.poll(&ctx);
    app.advance_guarded_action(&ctx);
    assert!(app.pending.is_none());
    assert!(app.documents[0].path.is_none());
    assert!(app.documents[0].revision.is_none());
    assert_eq!(app.documents[0].text, "keep this edit");
    app.guarded(Action::Quit, &ctx);
    app.discard_guarded_action(old, &ctx);
    assert!(app.pending.modal());
    assert!(!app.exit.closing());
}

#[test]
fn explicit_disconnect_waits_for_save_acknowledgement_then_closes_all_tabs() {
    let (mut app, ctx) = fixture();
    let workspace = crate::app::tests::remote_workspace();
    app.workspace.set_fixture(Some(workspace));
    let id = source(&mut app);
    app.documents[0].fixture_content().saving = true;
    app.disconnect_ssh(&ctx);
    assert!(app.pending.waiting_for_save());
    assert!(app.workspace.is_some());
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Save {
                id,
                text: "".into(),
            },
            result: Box::new(Ok(Response {
                path: "acknowledged-copy.txt".into(),
                revision: "acknowledged".into(),
                ..Default::default()
            })),
        })
        .unwrap();
    app.poll(&ctx);
    app.advance_guarded_action(&ctx);
    assert!(app.workspace.is_none());
    assert!(app.documents.is_empty());
    assert!(app.pending.is_none());
}

#[test]
fn explicit_disconnect_guards_all_editor_windows_and_rechecks_discard_approval() {
    let (mut app, ctx) = fixture();
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    source(&mut app);
    let child = app.allocate_editor_window();
    app.in_editor(child, |app| {
        app.new_file();
        app.documents
            .active_mut()
            .unwrap()
            .set_source("unsaved child edit".into());
    });
    let generation = app.workers.generation();
    app.disconnect_ssh(&ctx);
    let (decision, waiting) = app.pending.presentation().unwrap();
    assert!(waiting.is_none());
    assert!(app.workspace.as_ref().unwrap().connected);
    assert_eq!(app.documents.len(), 2);
    app.cancel_guarded_decision(decision);
    assert!(app.pending.is_none());
    assert_eq!(app.workers.generation(), generation);
    assert_eq!(app.documents[1].text, "unsaved child edit");

    app.disconnect_ssh(&ctx);
    let (older, _) = app.pending.presentation().unwrap();
    app.documents[1].set_source("newer child edit".into());
    app.discard_guarded_action(older, &ctx);
    let (current, _) = app.pending.presentation().unwrap();
    assert_ne!(current, older);
    assert!(app.workspace.as_ref().unwrap().connected);
    assert_eq!(app.documents.len(), 2);
    app.discard_guarded_action(current, &ctx);
    assert!(app.workspace.is_none());
    assert!(app.documents.is_empty());
    assert!(app.documents.selected_in(egui::ViewportId::ROOT).is_none());
    assert!(app.documents.selected_in(child).is_none());
    assert!(app.workers.generation() > generation);
}

#[test]
fn a_failed_save_cancels_disconnect_and_preserves_the_connection_and_edits() {
    let (mut app, ctx) = fixture();
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    let id = source(&mut app);
    app.documents[0].set_source("unsaved source".into());
    app.documents[0].fixture_content().saving = true;
    app.disconnect_ssh(&ctx);
    assert!(app.pending.waiting_for_save());
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Save {
                id,
                text: "unsaved source".into(),
            },
            result: Box::new(Err("fixture save refused".into())),
        })
        .unwrap();
    app.poll(&ctx);
    app.advance_guarded_action(&ctx);
    assert!(app.pending.is_none());
    assert!(app.workspace.as_ref().unwrap().connected);
    assert_eq!(app.documents[0].text, "unsaved source");
    assert!(app.documents[0].dirty());
}

#[test]
fn a_project_reply_started_before_quit_cannot_supersede_its_save_barrier() {
    let project = tempfile::tempdir().unwrap();
    let (mut app, ctx) = fixture();
    source(&mut app);
    let token = app.workspace.begin_preparation();
    app.documents[0].fixture_content().saving = true;
    app.guarded(Action::Quit, &ctx);
    let (decision, _) = app.pending.presentation().unwrap();
    let response = files::local(&Request::Prepare {
        path: project.path().to_string_lossy().into(),
        sandbox: Default::default(),
    })
    .unwrap();
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Prepare {
                token,
                remote: false,
                options: Default::default(),
                keep_buffers: false,
                documents_at_start: app.documents.versions(),
                policy: workspace::PreparationPolicy::Given,
                open_file: None,
            },
            result: Box::new(Ok(response)),
        })
        .unwrap();
    app.poll(&ctx);
    assert!(app.workspace.is_none());
    assert!(app.pending.waiting_for_save());
    assert_eq!(app.pending.presentation().unwrap().0, decision);
    assert!(app.workspace.prepared(token).is_none());
}

#[test]
fn reconnect_does_not_replace_transport_or_buffers_before_a_save_acknowledgement() {
    let (mut app, ctx) = fixture();
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    let socket = app
        .workspace
        .as_ref()
        .unwrap()
        .remote
        .as_ref()
        .unwrap()
        .socket
        .clone();
    let id = source(&mut app);
    app.documents[0].fixture_content().saving = true;
    let generation = app.workers.generation();
    app.reconnect_ssh(&ctx);
    assert!(matches!(app.pending.as_ref(), Some(Action::Reconnect(_))));
    assert!(app.pending.waiting_for_save());
    assert_eq!(app.workers.generation(), generation);
    assert_eq!(
        app.workspace
            .as_ref()
            .unwrap()
            .remote
            .as_ref()
            .unwrap()
            .socket,
        socket
    );
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Save {
                id,
                text: String::new(),
            },
            result: Box::new(Err("save refused".into())),
        })
        .unwrap();
    app.poll(&ctx);
    app.advance_guarded_action(&ctx);
    assert!(app.pending.is_none());
    assert_eq!(app.workers.generation(), generation);
    assert_eq!(
        app.workspace
            .as_ref()
            .unwrap()
            .remote
            .as_ref()
            .unwrap()
            .socket,
        socket
    );
    assert!(app.notice.as_ref().unwrap().0.contains("save refused"));
}

#[test]
fn a_preparation_reply_and_native_text_in_one_desktop_frame_preserve_the_text() {
    let project = tempfile::tempdir().unwrap();
    let (mut app, ctx) = fixture();
    source(&mut app);
    frame(&mut app, &ctx, vec![], None);
    frame(&mut app, &ctx, vec![], None);
    let token = app.workspace.begin_preparation();
    let response = files::local(&Request::Prepare {
        path: project.path().to_string_lossy().into(),
        sandbox: Default::default(),
    })
    .unwrap();
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Prepare {
                token,
                remote: false,
                options: Default::default(),
                keep_buffers: false,
                documents_at_start: app.documents.versions(),
                policy: workspace::PreparationPolicy::Given,
                open_file: None,
            },
            result: Box::new(Ok(response)),
        })
        .unwrap();
    let mut desktop = eframe::Frame::_new_kittest();
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Text("typed with the reply".into())],
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 700.),
            )),
            ..Default::default()
        },
        |ctx| eframe::App::update(&mut app, ctx, &mut desktop),
    );
    assert!(app.workspace.is_none());
    assert_eq!(app.documents[0].text, "typed with the reply");
    assert!(matches!(app.pending.as_ref(), Some(Action::PreparedProject { id }) if *id == token));
}

#[test]
fn a_preparation_completion_cannot_replace_an_existing_discard_decision() {
    let project = tempfile::tempdir().unwrap();
    let (mut app, ctx) = fixture();
    let id = source(&mut app);
    let token = app.workspace.begin_preparation();
    app.documents[0].fixture_content().text = "keep until confirmed".into();
    app.documents[0].set_dirty_fixture(true);
    app.guarded(Action::CloseTab(id), &ctx);
    let decision = app.pending.presentation().unwrap().0;
    let response = files::local(&Request::Prepare {
        path: project.path().to_string_lossy().into(),
        sandbox: Default::default(),
    })
    .unwrap();
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Prepare {
                token,
                remote: false,
                options: Default::default(),
                keep_buffers: false,
                documents_at_start: app.documents.versions(),
                policy: workspace::PreparationPolicy::Given,
                open_file: None,
            },
            result: Box::new(Ok(response)),
        })
        .unwrap();
    app.poll(&ctx);
    assert_eq!(app.pending.presentation().unwrap().0, decision);
    assert!(matches!(app.pending.as_ref(), Some(Action::CloseTab(current)) if *current == id));
    assert!(app.workspace.prepared(token).is_none());
    assert_eq!(app.documents[0].text, "keep until confirmed");
}

#[test]
fn prepared_discard_confirmation_keeps_the_requesting_windows_dialog_owner() {
    let project = tempfile::tempdir().unwrap();
    let (mut app, ctx) = fixture();
    source(&mut app);
    let token = app.workspace.begin_preparation();
    let versions = app.documents.versions();
    let owner = egui::ViewportId::from_hash_of("requesting-tools-window");
    app.dialog_viewport = owner;
    app.documents[0].fixture_content().text = "edited during preparation".into();
    app.documents[0].set_dirty_fixture(true);
    app.documents[0].advance_version_fixture();
    let response = files::local(&Request::Prepare {
        path: project.path().to_string_lossy().into(),
        sandbox: Default::default(),
    })
    .unwrap();
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Prepare {
                token,
                remote: false,
                options: Default::default(),
                keep_buffers: false,
                documents_at_start: versions,
                policy: workspace::PreparationPolicy::Given,
                open_file: None,
            },
            result: Box::new(Ok(response)),
        })
        .unwrap();
    let _ = ctx.run(Default::default(), |ctx| {
        assert_eq!(ctx.viewport_id(), egui::ViewportId::ROOT);
        app.poll(ctx);
    });
    assert!(app.pending.modal());
    assert!(app.workspace.is_none());
    assert_eq!(app.dialog_viewport, owner);
}

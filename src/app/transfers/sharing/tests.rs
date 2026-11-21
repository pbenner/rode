use super::*;

fn mounted() -> (EditorApp, egui::Context, Arc<Control>, Pending) {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    app.shared_folders.restore(vec![Config {
        source: "/chosen/source".into(),
        destination: "/chosen/mount".into(),
        ..Default::default()
    }]);
    let control = Arc::new(Control::default());
    let state = &mut app.shared_folders;
    state.rows[0].running = Some(control.clone());
    state.rows[0].ready = true;
    let work = Pending::new(&state.pending);
    (app, ctx, control, work)
}

fn finished(app: &mut EditorApp, result: Result<(), String>) {
    let state = &app.shared_folders;
    state
        .tx
        .send(Reply::Finished {
            generation: state.generation,
            id: state.rows[0].id,
            result,
        })
        .unwrap();
    app.poll_shared_folders();
}

#[test]
fn disconnect_waits_for_cleanup_and_its_reply_before_releasing_the_host() {
    let (mut app, ctx, control, work) = mounted();
    app.guarded(Action::Disconnect, &ctx);
    assert!(app.pending.waiting_for_shares());
    assert!(control.unmount.load(Ordering::Acquire));
    assert!(!control.cancel.load(Ordering::Acquire));
    assert!(app.workspace.as_ref().unwrap().connected);
    drop(work);
    app.advance_guarded_action(&ctx);
    assert!(
        app.pending.waiting_for_shares(),
        "A released worker count is not its cleanup acknowledgement"
    );
    finished(&mut app, Ok(()));
    app.advance_guarded_action(&ctx);
    assert!(app.workspace.is_none());
    assert!(app.pending.is_none());
}

#[test]
fn busy_unmount_cancels_the_transition_even_when_another_guard_intervenes() {
    for intervening_guard in [false, true] {
        let (mut app, ctx, control, _work) = mounted();
        let generation = app.workers.generation();
        app.guarded(Action::Disconnect, &ctx);
        if intervening_guard {
            app.explorer_state.busy = true;
            app.advance_guarded_action(&ctx);
            assert!(!app.pending.waiting_for_shares());
            assert!(app.pending.waiting_for_ack());
        }
        control.unmount.store(false, Ordering::Release);
        let state = &app.shared_folders;
        state
            .tx
            .send(Reply::Status {
                generation: state.generation,
                id: state.rows[0].id,
                event: MountEvent::Error("mount is busy".into()),
            })
            .unwrap();
        app.poll_shared_folders();
        app.advance_guarded_action(&ctx);
        assert!(app.pending.is_none());
        assert!(app.workspace.as_ref().unwrap().connected);
        assert_eq!(app.workers.generation(), generation);
        assert!(app.shared_folders.rows[0].ready);
        assert!(!app.shared_folders.rows[0].unmounting);
        assert!(!control.cancel.load(Ordering::Acquire));
        assert!(
            !control.unmount.load(Ordering::Acquire),
            "A failed unmount must not retry automatically"
        );
        assert!(app.notice.as_ref().unwrap().1);
    }
}

#[test]
fn unsaved_decisions_precede_automatic_unsharing_and_cancellation_keeps_the_mount() {
    let (mut app, ctx, control, _work) = mounted();
    app.new_file();
    app.documents[0].fixture_content().text = "unsaved".into();
    app.guarded(Action::Quit, &ctx);
    let (decision, wait) = app.pending.presentation().unwrap();
    assert!(wait.is_none());
    assert!(!control.unmount.load(Ordering::Acquire));
    app.cancel_guarded_decision(decision);
    assert!(app.shared_folders.rows[0].ready);
    app.guarded(Action::Quit, &ctx);
    let (decision, _) = app.pending.presentation().unwrap();
    app.discard_guarded_action(decision, &ctx);
    assert!(app.pending.waiting_for_shares());
    assert!(control.unmount.load(Ordering::Acquire));
    assert!(!app.exit.closing());
}

#[test]
fn disabling_the_extension_waits_for_shared_folder_cleanup() {
    let (mut app, ctx, control, work) = mounted();
    let generation = app.shared_folders.generation;
    app.request_extension(crate::extensions::Feature::Transfers.id(), false, &ctx);
    assert!(app.feature(crate::extensions::Feature::Transfers));
    assert_eq!(app.shared_folders.generation, generation);
    assert!(control.unmount.load(Ordering::Acquire));
    drop(work);
    finished(&mut app, Ok(()));
    app.advance_guarded_action(&ctx);
    assert!(!app.feature(crate::extensions::Feature::Transfers));
    assert!(app.shared_folders.configs().is_empty());
}

#[test]
fn transfer_access_and_detached_views_cannot_reset_project_shares() {
    let (mut app, ctx, control, _work) = mounted();
    let configs = app.shared_folders.configs();
    let generation = app.shared_folders.generation;
    app.bind_project_transfers();
    assert!(app.pending.is_none());
    app.transfers.reset();
    let tab = tool_windows::ToolTab::Panel(Panel::SharedFolders);
    app.reveal_panel(Panel::SharedFolders, &ctx);
    app.detach_tab(tab, &ctx);
    app.reattach_tool_tab(tab, &ctx);
    assert_eq!(app.shared_folders.generation, generation);
    assert_eq!(app.shared_folders.configs(), configs);
    assert!(app.shared_folders.rows[0].ready);
    assert!(!control.unmount.load(Ordering::Acquire));
    assert!(!control.cancel.load(Ordering::Acquire));
}

#[test]
fn prepared_projects_are_held_until_unsharing_and_failed_cleanup_releases_them() {
    let (mut app, ctx, _control, _work) = mounted();
    let id = app.workspace.begin_preparation();
    app.workspace.hold_prepared(
        id,
        workspace::Prepared {
            policy: workspace::PreparationPolicy::Given,
            response: Response::default(),
            remote: true,
            options: Default::default(),
            open_file: None,
            keep_buffers: false,
            approved_documents: vec![],
        },
    );
    app.guarded(Action::PreparedProject { id }, &ctx);
    assert!(app.pending.waiting_for_shares());
    assert!(app.workspace.prepared(id).is_some());
    finished(&mut app, Err("cleanup failed".into()));
    assert!(app.pending.is_none());
    assert!(app.workspace.prepared(id).is_none());
    assert!(app.workspace.as_ref().unwrap().connected);
}

#[test]
fn reopening_an_already_mounted_current_bookmark_does_not_restart_sharing() {
    let (mut app, ctx, control, _work) = mounted();
    let bookmark = app.bookmark_candidate().unwrap();
    let generation = app.shared_folders.generation;
    app.guarded(Action::Bookmark(Box::new(bookmark)), &ctx);
    assert!(app.pending.is_none());
    assert!(!control.unmount.load(Ordering::Acquire));
    assert!(!control.cancel.load(Ordering::Acquire));
    assert_eq!(app.shared_folders.generation, generation);
    assert!(app.project_bookmarks.restore_shares.is_none());
}

#[test]
fn forced_reset_cancels_work_but_preserves_its_cleanup_lifetime() {
    let (mut app, _ctx, control, work) = mounted();
    app.shared_folders.reset();
    assert!(control.cancel.load(Ordering::Acquire));
    assert!(app.shared_folders.configs().is_empty());
    assert!(app.shared_folders.pending());
    assert!(app.exit_readiness().pending());
    drop(work);
    assert!(!app.shared_folders.pending());
}

use super::*;

#[test]
fn active_shares_guard_transitions_and_forced_reset_cancels_owned_work() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    app.bind_project_transfers();
    let config = Config {
        source: "/chosen/source".into(),
        destination: "/chosen/mount".into(),
        ..Default::default()
    };
    app.transfers.sharing.restore(vec![config]);
    let control = Arc::new(Control::default());
    app.transfers.sharing.rows[0].running = Some(control.clone());
    let work = Pending::new(&app.transfers.sharing.pending);
    let changes = [
        ContextChange::Project,
        ContextChange::Host,
        ContextChange::Access,
        ContextChange::Disable,
        ContextChange::Quit,
    ];
    for change in changes {
        assert!(app.transfers.sharing.admit(change).is_err());
    }
    app.guarded(Action::Disconnect, &ctx);
    assert!(app.workspace.as_ref().unwrap().connected);
    app.guarded(Action::Quit, &ctx);
    assert!(!app.exit_pending());
    app.prepare_project(
        "/different/project".into(),
        Default::default(),
        true,
        false,
        None,
        &ctx,
    );
    assert!(!app.workspace.preparing());
    let candidate = app.workspace.begin_preparation();
    app.workspace.hold_prepared(
        candidate,
        workspace::Prepared {
            response: Response::default(),
            remote: true,
            options: Default::default(),
            open_file: None,
            keep_buffers: false,
            approved_documents: vec![],
        },
    );
    app.guarded(Action::PreparedProject { id: candidate }, &ctx);
    assert!(
        app.workspace.prepared(candidate).is_none(),
        "Rejected candidates must release their resources"
    );
    app.set_extension(crate::extensions::Feature::Transfers.id(), false);
    assert!(app.feature(crate::extensions::Feature::Transfers));
    app.transfers.reset();
    assert!(control.cancel.load(Ordering::Acquire));
    assert!(app.transfers.sharing.configs().is_empty());
    assert!(
        app.transfers.pending(),
        "Cleanup must retain its lifetime barrier"
    );
    for change in changes {
        assert!(
            app.transfers.sharing.admit(change).is_err(),
            "Removing a cancelled row must not admit context changes before cleanup"
        );
    }
    drop(work);
    assert!(!app.transfers.pending());
    for change in changes {
        assert!(app.transfers.sharing.admit(change).is_ok());
    }
}

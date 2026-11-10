//! Window session policy, queued command and guarded lifetime regressions.
use super::*;

#[test]
fn reattachment_accepts_both_user_modes_but_not_other_roots_or_policies() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx);
    let mut app = EditorApp::new(&cc, Startup::default());
    let mut workspace = crate::app::tests::remote_workspace();
    workspace.plan = sandbox::Plan {
        root: workspace.root.clone(),
        active: true,
        requested: true,
        prefix: vec!["sandbox".into(), "--".into()],
        ..Default::default()
    };
    app.workspace = Some(workspace);
    for sandboxed in [false, true] {
        let plan = app.terminal_plan(sandboxed);
        let mut info: rode_session::Info = serde_json::from_value(serde_json::json!({
            "id": "session", "name": "shell", "root": plan.root,
            "policy": service::fingerprint(&plan), "mode": plan.label(),
            "alive": true, "clients": 1, "created": 0, "exit_code": null, "persistent": false, "allocation": null
        }))
        .unwrap();
        assert!(app.compatible_session(&info));
        info.root = "/another-project".into();
        assert!(!app.compatible_session(&info));
        info.root = plan.root.clone();
        let mut changed = plan;
        changed.project_read_only = true;
        info.policy = service::fingerprint(&changed);
        assert!(!app.compatible_session(&info));
    }
    assert!(app.workspace.as_ref().unwrap().plan.active);
}

#[test]
fn queued_commands_keep_their_policy_and_never_fall_through_to_a_host_shell() {
    use crate::terminal::StreamControl;
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let mut workspace = crate::app::tests::remote_workspace();
    workspace.plan = sandbox::Plan {
        root: workspace.root.clone(),
        active: true,
        requested: true,
        prefix: vec!["sandbox".into(), "--".into()],
        ..Default::default()
    };
    app.workspace = Some(workspace);
    let project = app.session_plan();
    let host = app.terminal_plan(false);
    let (tx, rx) = std::sync::mpsc::channel();
    let (terminal, _stream) = Terminal::stream(&ctx, "host shell".into(), move |event| {
        if let StreamControl::Input(bytes) = event {
            let _ = tx.send(bytes);
        }
    });
    app.terminal = Some(terminal);
    app.sessions.active = Some(Meta {
        key: "host".into(),
        order: 0,
        name: "host shell".into(),
        native: None,
        policy: service::fingerprint(&host),
        mode: host.label().into(),
    });
    app.sessions.busy = true;
    app.queue_terminal_command("host-command".into(), host.clone(), &ctx);
    app.queue_terminal_command("project-command".into(), project.clone(), &ctx);
    // A queued command retains the plan captured at submission, even if
    // the workspace changes before it is considered for execution.
    app.workspace.as_mut().unwrap().plan = host;
    assert_eq!(
        service::fingerprint(&app.sessions.queued[1].plan),
        service::fingerprint(&project)
    );
    app.workspace.as_mut().unwrap().ready = false;
    app.sessions.busy = false;
    app.drain_terminal_commands(&ctx, None);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        b"host-command\n"
    );
    assert!(
        rx.try_recv().is_err(),
        "Project command entered the unrestricted shell"
    );
    assert!(app.sessions.queued.is_empty());
    assert!(!app.sessions.starting);
}

#[test]
fn a_failed_new_terminal_drops_queued_commands_without_an_automatic_retry() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let mut workspace = crate::app::tests::remote_workspace();
    workspace.plan.root = workspace.root.clone();
    app.workspace = Some(workspace);
    let plan = app.session_plan();
    let policy = service::fingerprint(&plan);
    let (terminal, stream) = Terminal::stream(&ctx, "Failed shell".into(), |_| {});
    app.terminal = Some(terminal);
    app.sessions.active = Some(Meta {
        key: "failed".into(),
        order: 0,
        name: "Failed shell".into(),
        native: None,
        policy: policy.clone(),
        mode: plan.label().into(),
    });
    app.sessions.queued.push_back(QueuedCommand {
        command: "must-not-retry".into(),
        plan,
    });
    stream.end();
    assert!(!app.terminal.as_ref().unwrap().is_alive());
    app.drain_terminal_commands(&ctx, Some(&policy));
    assert!(!app.sessions.starting);
    assert!(!app.sessions.busy);
    assert!(app.sessions.queued.is_empty());
    assert!(app.notice.as_ref().is_some_and(|(message, error, _)| {
        *error && message.contains("Remaining queued commands were not sent")
    }));
}

#[test]
fn quitting_waits_for_a_toggle_and_keeps_the_window_open_if_it_fails() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.sessions.persistence_pending = true;
    app.quit_terminals(&ctx);
    assert!(!app.quitting);
    assert!(app.dialog == Dialog::TerminalExit);
    app.handle_session(Job::Persistence, Err("Host connection lost".into()), &ctx);
    assert!(!app.quitting);
    assert!(!app.sessions.quit_waiting);
    assert!(app.dialog == Dialog::None);
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(message, error, _)| *error && message.contains("Host connection lost"))
    );
}

#[test]
fn quit_still_guards_unsaved_source_before_any_terminal_cleanup() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].text = "keep these edits".into();
    app.documents[0].modified = true;
    app.guarded(Action::Quit, &ctx);
    assert!(matches!(app.pending, Some(Action::Quit)));
    assert!(!app.quitting);
    assert!(!app.sessions.quit_waiting);
    assert_eq!(app.documents[0].text, "keep these edits");
}

#[test]
fn disconnect_waits_for_terminal_startup_and_persistence_changes() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace = Some(crate::app::tests::remote_workspace());
    for starting in [true, false] {
        app.sessions.starting = starting;
        app.sessions.persistence_pending = !starting;
        let generation = app.generation;
        app.disconnect_ssh(&ctx);
        assert!(app.workspace.as_ref().unwrap().connected);
        assert_eq!(app.generation, generation);
        assert!(!app.connection_cancel.load(Ordering::Acquire));
    }
}

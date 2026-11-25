//! Captured host identities and transfer authority cannot follow project selection.
use super::*;

fn app() -> (EditorApp, egui::Context) {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    (app, ctx)
}

#[test]
fn selected_monitor_never_follows_a_new_workspace_or_falls_back_locally() {
    let (mut app, _ctx) = app();
    app.initialize_host_tool(Panel::Monitor);
    let socket = app
        .host_tools
        .monitor_remote
        .as_ref()
        .unwrap()
        .socket
        .clone();
    app.workspace.set_fixture(None);
    app.initialize_host_tool(Panel::Monitor);
    assert_eq!(
        app.host_tools.monitor_remote.as_ref().unwrap().socket,
        socket
    );
    assert!(!app.monitor_host_ready());
    assert!(app.monitor_host_label().contains("test-host"));
    app.select_monitor_host(None);
    assert!(app.monitor_host_ready());
    assert_eq!(app.monitor_host_label(), "This computer");
}

#[test]
fn a_host_switch_drains_the_old_monitor_reply_before_admitting_the_new_host() {
    let (mut app, ctx) = app();
    app.initialize_host_tool(Panel::Monitor);
    let previous = app.host_tools.monitor_generation;
    app.host_tools.monitoring.set_active(true);
    app.host_tools.monitoring.monitor_busy = true;
    app.select_monitor_host(None);
    app.host_tools.monitoring.set_active(true);
    app.host_tools
        .tx
        .send(Reply::Monitor {
            generation: previous,
            cadence: Duration::from_secs(1),
            result: Err("Previous host disconnected".into()),
        })
        .unwrap();
    app.poll_host_tools(&ctx);
    assert!(!app.host_tools.monitoring.monitor_busy);
    assert!(app.host_tools.monitoring.monitor_error.is_empty());
    assert!(app.host_tools.monitoring.history.is_empty());
    assert!(app.host_tools.monitoring.monitor_last.is_none());
    assert!(app.host_tools.monitoring.active);

    app.host_tools.monitoring.monitor_busy = true;
    app.host_tools
        .tx
        .send(Reply::Monitor {
            generation: app.host_tools.monitor_generation,
            cadence: Duration::from_secs(1),
            result: Ok(crate::project::Response::Monitor(crate::project::Monitor {
                hostname: "selected-host".into(),
                ..Default::default()
            })),
        })
        .unwrap();
    app.poll_host_tools(&ctx);
    assert_eq!(app.host_tools.monitoring.history.len(), 1);
    assert_eq!(
        app.host_tools.monitoring.monitor.as_ref().unwrap().hostname,
        "selected-host"
    );
}

#[test]
fn project_transfer_authority_expires_but_explicit_host_authority_survives_project_changes() {
    let (mut app, _ctx) = app();
    app.initialize_host_tool(Panel::Transfers);
    assert!(app.transfer_host_endpoint().is_some());
    let remote = app.host_tools.transfer_remote.clone().unwrap();
    app.workers.invalidate_workspace();
    assert!(app.transfer_host_endpoint().is_none());
    app.bind_project_transfers();
    assert!(app.transfer_host_endpoint().is_some());
    app.host_tools.transfer.as_mut().unwrap().project_generation = None;
    app.workers.invalidate_workspace();
    app.invalidate_project_transfers();
    assert!(app.transfer_host_endpoint().is_some());
    app.workspace.set_fixture(None);
    assert!(app.transfer_host_endpoint().is_none());
    assert_eq!(
        app.host_tools.transfer_remote.as_ref().unwrap().socket,
        remote.socket
    );
}

#[test]
fn project_shares_cannot_adopt_the_transfer_browsers_host_or_policy() {
    let (mut app, _) = app();
    let project = app.project_shared_folder_endpoint().unwrap();
    app.bind_project_transfers();
    let other = Remote::new(crate::ssh::Profile {
        host: "another-host".into(),
        ..Default::default()
    })
    .unwrap();
    app.host_tools.transfer = Some(TransferHost {
        endpoint: crate::transfers::Endpoint::ssh(
            other,
            sandbox::Plan {
                root: "/outside-project".into(),
                ..Default::default()
            },
        ),
        project_generation: None,
    });
    let sharing = app.project_shared_folder_endpoint().unwrap();
    assert_eq!(
        sharing.remote().unwrap().socket,
        project.remote().unwrap().socket
    );
    assert_eq!(
        serde_json::to_value(sharing.plan().unwrap()).unwrap(),
        serde_json::to_value(project.plan().unwrap()).unwrap()
    );
    app.workspace.as_mut().unwrap().connected = false;
    assert!(app.project_shared_folder_endpoint().is_none());
    app.workspace.set_fixture(None);
    assert!(app.project_shared_folder_endpoint().is_none());
}

#[test]
fn late_host_preparation_cannot_replace_a_newer_project_policy() {
    let (mut app, ctx) = app();
    app.initialize_host_tool(Panel::Transfers);
    let generation = app.host_tools.transfer_generation;
    let remote = app.host_tools.transfer_remote.clone().unwrap();
    app.bind_project_transfers();
    app.host_tools
        .tx
        .send(Reply::Prepared {
            generation,
            remote: Box::new(remote),
            result: Ok(sandbox::Plan {
                root: "/stale".into(),
                ..Default::default()
            }),
        })
        .unwrap();
    app.poll_host_tools(&ctx);
    assert!(
        app.host_tools
            .transfer
            .as_ref()
            .unwrap()
            .project_generation
            .is_some()
    );
    assert_ne!(app.transfer_host_root().as_deref(), Some("/stale"));
}

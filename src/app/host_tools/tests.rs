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
    let (mut app, ctx) = app();
    app.initialize_host_tool(Panel::Monitor, &ctx);
    let socket = app
        .host_tools
        .monitor_remote
        .as_ref()
        .unwrap()
        .socket
        .clone();
    app.workspace.set_fixture(None);
    app.initialize_host_tool(Panel::Monitor, &ctx);
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
fn project_transfer_authority_expires_but_explicit_host_authority_survives_project_changes() {
    let (mut app, ctx) = app();
    app.initialize_host_tool(Panel::Transfers, &ctx);
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
fn late_host_preparation_cannot_replace_a_newer_project_policy() {
    let (mut app, ctx) = app();
    app.initialize_host_tool(Panel::Transfers, &ctx);
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

//! Headless tunnel-manager interactions and responsive layout regressions.
use super::*;

const LISTENER_ERROR: &str = "SSH login to relay login.example.org:8443 succeeded, but the relay refused tunnel listener 127.0.0.1:22022.\nSSH did not report why. Possible causes: the port is already in use, or forwarding is restricted on the relay.\nOn the relay, run: ss -ltnp 'sport = :22022' (use sudo to see all owners).\nAn sshd-session listener usually belongs to an existing SSH tunnel. Stop that tunnel at its source, or choose a different Relay port under Port forwarding.\nRODE will retry automatically.";

#[test]
fn new_tunnel_setup_allows_automatic_credentials_without_selecting_a_key_filename() {
    for remote in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        ui_probe::install_at(&ctx, path.clone());
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        if remote {
            app.workspace = Some(crate::app::tests::remote_workspace());
        }
        app.dialog = Dialog::Tunnels;
        app.tunnels.page = Page::New;
        app.tunnels.spec.relay_host = "relay.example.org".into();
        app.tunnels.spec.relay_user = "researcher".into();
        assert!(app.tunnels.spec.identity.is_empty() && app.tunnels.spec.agent_socket.is_empty());
        app.tunnels.spec.validate().unwrap();
        let hint = "Next: verify the relay’s host key.";
        for size in [egui::vec2(390., 540.), egui::vec2(900., 1000.)] {
            for _ in 0..3 {
                dialog_frame(&mut app, &ctx, size, vec![]);
            }
            let output = dialog_frame(&mut app, &ctx, size, vec![]);
            let (rect, clip) = text_bounds(&output, hint);
            assert!(ctx.content_rect().contains_rect(rect) && clip.contains_rect(rect));
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("id_ed25519"))));
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert!(
                state["buttons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|button| button["name"] == "tunnel-start" && button["enabled"] == true)
            );
        }
        // Automatic and explicit credentials enable Start without reading files,
        // inheriting an agent or dispatching network work during rendering.
        for (identity, agent) in [
            ("", ""),
            ("~/keys/relay", ""),
            ("", "/run/user/1000/agent.sock"),
        ] {
            app.tunnels.spec.identity = identity.into();
            app.tunnels.spec.agent_socket = agent.into();
            dialog_frame(&mut app, &ctx, egui::vec2(900., 1000.), vec![]);
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert!(
                state["buttons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|button| button["name"] == "tunnel-start" && button["enabled"] == true)
            );
            assert!(app.tunnels.pending.is_none() && app.tunnels.available.is_empty());
        }
        if let Some(remote) = app
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.remote.as_ref())
        {
            remote.cleanup();
        }
    }
}

#[test]
fn tunnel_errors_can_be_copied_without_starting_host_work() {
    for request_error in [true, false] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.dialog = Dialog::Tunnels;
        if request_error {
            app.tunnels.error = LISTENER_ERROR.into();
        } else {
            app.tunnels.available = vec![Info {
                id: "failed-tunnel".into(),
                hostname: "compute".into(),
                spec: Spec {
                    relay_host: "login.example.org".into(),
                    ..Default::default()
                },
                status: Status::Reconnecting,
                error: LISTENER_ERROR.into(),
                observed_key: String::new(),
                fingerprint: String::new(),
                attempts: 2,
            }];
        }
        let output = click(&mut app, &ctx, "Copy error");
        assert!(
            output
                .platform_output
                .commands
                .iter()
                .any(|command| matches!(
                    command, egui::OutputCommand::CopyText(text) if text == LISTENER_ERROR
                ))
        );
        assert!(app.tunnels.pending.is_none());
        assert!(app.dialog == Dialog::Tunnels);
        assert_eq!(app.tunnels.page, Page::Tunnels);
    }
}

#[test]
fn tunnel_host_selection_supports_local_work_and_never_falls_back_from_remote() {
    assert!(matches!(Host::for_workspace(None).unwrap(), Host::Local));
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace = Some(crate::app::tests::remote_workspace());
    let workspace = app.workspace.as_mut().unwrap();
    workspace.ready = false;
    workspace.plan.active = true;
    workspace.plan.root = workspace.root.clone();
    match Host::for_workspace(Some(workspace)).unwrap() {
        Host::Remote { remote, root } => {
            assert_eq!(root, workspace.root);
            assert_eq!(remote.socket, workspace.remote.as_ref().unwrap().socket);
        }
        Host::Local => panic!("Remote administration must stay on the remote host"),
    }
    workspace.connected = false;
    assert!(Host::for_workspace(Some(workspace)).is_err());
    app.tunnel_request(Control::Tunnels, &ctx);
    assert!(app.tunnels.pending.is_none());
    assert!(app.tunnels.error.contains("Reconnect SSH"));
    let workspace = app.workspace.as_mut().unwrap();
    workspace.remote.take().unwrap().cleanup();
    // Local host administration is available before project preparation and
    // independently of the project's sandbox policy.
    assert!(matches!(
        Host::for_workspace(Some(workspace)).unwrap(),
        Host::Local
    ));
}

#[test]
fn local_tunnel_manager_is_reachable_without_a_workspace_and_offers_return_details() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    assert!(app.workspace.is_none());
    let mut title_frame = |events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 700.),
                )),
                events,
                ..Default::default()
            },
            |ctx| app.titlebar(ctx),
        )
    };
    title_frame(vec![]);
    let output = title_frame(vec![]);
    let (rect, clip) = text_bounds(&output, "Tunnels");
    assert!(clip.contains_rect(rect));
    for pressed in [true, false] {
        title_frame(vec![
            egui::Event::PointerMoved(rect.center()),
            egui::Event::PointerButton {
                pos: rect.center(),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            },
        ]);
    }
    assert!(app.dialog == Dialog::Tunnels);
    click(&mut app, &ctx, "New tunnel");
    let output = dialog_frame(&mut app, &ctx, egui::vec2(900., 1000.), vec![]);
    text_bounds(&output, "This computer · local service");
    text_bounds(&output, "Credentials on this computer");
    text_bounds(&output, "Start tunnel");
    assert!(
        app.tunnels.pending.is_none(),
        "Painting must not start a service"
    );
    app.tunnels.available = vec![Info {
        id: "local-tunnel".into(),
        hostname: String::new(),
        spec: Spec {
            relay_host: "2001:db8::1".into(),
            relay_user: "relay-user".into(),
            relay_port: 2222,
            listen_port: 22022,
            ..Default::default()
        },
        status: Status::Connected,
        error: String::new(),
        observed_key: String::new(),
        fingerprint: String::new(),
        attempts: 1,
    }];
    click(&mut app, &ctx, "Tunnels (1)");
    let output = click(&mut app, &ctx, "Copy connection details");
    text_bounds(&output, "[2001:db8::1]:22022");
    let hostname = rode_session::tunnels::local_hostname().expect("Test host has a hostname");
    text_bounds(&output, &format!("{hostname}:22"));
    assert!(
        output
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(
                command, egui::OutputCommand::CopyText(text)
                    if text.contains("Host: 127.0.0.1\nPort: 22022")
                    && text.contains("relay-user@[2001:db8::1]:2222")
                    && text.contains("verify its host key")
            ))
    );
    assert!(app.dialog == Dialog::Tunnels);
    assert!(app.tunnels.pending.is_none());
    app.tunnels.relay_jump = "desktop-relay-alias".into();
    assert!(
        app.tunnels
            .connection_details(&app.tunnels.available[0])
            .contains("Jump / ProxyJump: desktop-relay-alias")
    );
}

#[test]
fn tunnel_addresses_name_the_service_host_and_support_older_remote_services() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let mut workspace = crate::app::tests::remote_workspace();
    let remote = workspace.remote.as_mut().unwrap();
    remote.profile.host = "researcher@compute-alias".into();
    remote.profile.jump = "relay.example.org".into();
    remote.profile.tunnel = Some(Box::new(ssh::TunnelRoute {
        port: 22022,
        host_key_alias: "compute-alias".into(),
    }));
    app.workspace = Some(workspace);
    app.dialog = Dialog::Tunnels;
    app.tunnels.available = vec![Info {
        id: "remote-tunnel".into(),
        hostname: "compute-07.cluster.example.org".into(),
        spec: Spec {
            relay_host: "login.example.org".into(),
            relay_user: "researcher".into(),
            relay_port: 2222,
            listen_port: 22022,
            ssh_port: 2223,
            ..Default::default()
        },
        status: Status::Connected,
        error: String::new(),
        observed_key: String::new(),
        fingerprint: String::new(),
        attempts: 1,
    }];
    for (reported, expected) in [
        (
            "compute-07.cluster.example.org",
            "compute-07.cluster.example.org:2223",
        ),
        ("", "compute-alias:2223"),
    ] {
        app.tunnels.available[0].hostname = reported.into();
        for _ in 0..3 {
            dialog_frame(&mut app, &ctx, egui::vec2(900., 1000.), vec![]);
        }
        let output = dialog_frame(&mut app, &ctx, egui::vec2(900., 1000.), vec![]);
        for label in ["Tunnel", "Destination", "login.example.org:22022", expected] {
            let (rect, clip) = text_bounds(&output, label);
            assert!(clip.contains_rect(rect), "Clipped endpoint {label}");
        }
        assert!(
            !output.shapes.iter().any(|shape| matches!(
                &shape.shape, egui::Shape::Text(text)
                    if text.galley.text().contains("Relay SSH")
                    || text.galley.text().contains("login.example.org:2222")
            )),
            "The card must show the tunnel listener port, not the relay login port"
        );
    }
    assert!(
        app.tunnels.pending.is_none(),
        "Rendering hostnames must not start network work"
    );
    app.workspace
        .as_ref()
        .unwrap()
        .remote
        .as_ref()
        .unwrap()
        .cleanup();
}

#[test]
fn tunnel_route_result_respects_dismissal_and_setup_navigation() {
    for (dialog, page) in [
        (Dialog::None, Page::Tunnels),
        (Dialog::Folder, Page::Tunnels),
        (Dialog::Tunnels, Page::New),
    ] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let (tx, rx) = mpsc::channel();
        app.profile.host = "current-compute".into();
        app.tunnels.generation = Some(app.generation);
        app.tunnels.pending = Some(rx);
        app.dialog = dialog;
        app.tunnels.page = page;
        app.tunnels.spec.name = "Unfinished setup".into();
        let expected = std::mem::discriminant(&app.dialog);
        tx.send(Ok(Outcome::Route(Box::new(Profile {
            host: "late-route".into(),
            ..Default::default()
        }))))
        .unwrap();
        app.tunnel_tick(&ctx);
        assert_eq!(app.profile.host, "current-compute");
        assert_eq!(std::mem::discriminant(&app.dialog), expected);
        assert_eq!(app.tunnels.page, page);
        assert_eq!(app.tunnels.spec.name, "Unfinished setup");
        assert!(app.tunnels.pending.is_none());
    }
}

#[test]
fn tunnel_results_from_an_old_workspace_cannot_replace_the_new_profile() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let (tx, rx) = mpsc::channel();
    app.profile.host = "current-compute".into();
    app.tunnels.generation = Some(app.generation);
    app.tunnels.pending = Some(rx);
    tx.send(Ok(Outcome::Route(Box::new(Profile {
        host: "old-compute".into(),
        ..Default::default()
    }))))
    .unwrap();
    app.generation += 1;
    app.tunnel_tick(&ctx);
    assert_eq!(app.profile.host, "current-compute");
    assert!(app.tunnels.pending.is_none());
    assert!(app.dialog == Dialog::None);
}

#[test]
fn tunnel_controls_render_without_starting_workers_and_hidden_manager_does_not_poll() {
    for width in [330., 500., 680.] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.workspace = Some(crate::app::tests::remote_workspace());
        app.tunnel_tick(&ctx);
        assert!(
            app.tunnels.pending.is_none(),
            "Hidden manager must not poll"
        );
        app.tunnels.available = vec![Info {
            id: "retained".into(),
            hostname: String::new(),
            spec: Spec {
                relay_host: "relay.example".into(),
                relay_user: "compute".into(),
                ..Default::default()
            },
            status: Status::HostKeyRequired,
            error: "Verify the relay before trusting its host key".into(),
            observed_key: String::new(),
            fingerprint: "SHA256:abcdefghijklmnopqrstuvABCDEFGHIJKLMNOPQRSTU".into(),
            attempts: 1,
        }];
        app.tunnels.spec.listen_on = ListenOn::AllInterfaces;
        for page in [Page::Tunnels, Page::Saved, Page::New] {
            app.tunnels.page = page;
            for frame in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 1400.),
                        )),
                        time: Some(frame as f64),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let available = ui.available_rect_before_wrap();
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                            let content = ui.scope(|ui| app.tunnels_dialog(ui, ctx)).response.rect;
                            assert!(
                                content.right() <= available.right() + 1.,
                                "Tunnel {page:?} overflows at {width}: {content:?} vs {available:?}"
                            );
                        });
                    },
                );
            }
        }
        assert!(
            app.tunnels.pending.is_none(),
            "Painting must not start a tunnel"
        );
        app.workspace
            .as_ref()
            .unwrap()
            .remote
            .as_ref()
            .unwrap()
            .cleanup();
    }
}

fn dialog_frame(
    app: &mut EditorApp,
    ctx: &egui::Context,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(ctx.input(|input| input.time) + 0.1),
            events,
            ..Default::default()
        },
        |ctx| app.dialogs(ctx),
    )
}
fn text_bounds(output: &egui::FullOutput, label: &str) -> (egui::Rect, egui::Rect) {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => Some((
                egui::Rect::from_min_size(text.pos, text.galley.size()),
                shape.clip_rect,
            )),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "Missing tunnel control {label}; text: {:?}",
                output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some(text.galley.text()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            )
        })
}
fn click(app: &mut EditorApp, ctx: &egui::Context, label: &str) -> egui::FullOutput {
    let size = egui::vec2(900., 1000.);
    dialog_frame(app, ctx, size, vec![]);
    let output = dialog_frame(app, ctx, size, vec![]);
    let (rect, clip) = text_bounds(&output, label);
    assert!(clip.contains_rect(rect), "Click target is clipped: {label}");
    let mut result = egui::FullOutput::default();
    for pressed in [true, false] {
        result = dialog_frame(
            app,
            ctx,
            size,
            vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
        );
    }
    result
}

#[test]
fn tunnel_setup_retains_edits_and_focus_across_tabs_and_status_updates() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace = Some(crate::app::tests::remote_workspace());
    app.dialog = Dialog::Tunnels;
    app.tunnels.generation = Some(app.generation);
    click(&mut app, &ctx, "New tunnel");
    assert_eq!(app.tunnels.page, Page::New);
    app.tunnel_tick(&ctx);
    assert!(
        app.tunnels.pending.is_none(),
        "Setup must not schedule status polls"
    );
    click(&mut app, &ctx, "Compute tunnel");
    let (tx, rx) = mpsc::channel();
    app.tunnels.pending = Some(rx); // An already running discovery must not disable fields.
    dialog_frame(
        &mut app,
        &ctx,
        egui::vec2(900., 1000.),
        vec![
            egui::Event::Key {
                key: Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            },
            egui::Event::Text("Research node".into()),
        ],
    );
    assert_eq!(app.tunnels.spec.name, "Research node");
    tx.send(Ok(Outcome::Listed(vec![]))).unwrap();
    app.tunnel_tick(&ctx);
    assert_eq!(app.tunnels.page, Page::New);
    assert!(app.tunnels.pending.is_none());
    click(&mut app, &ctx, "Tunnels (0)");
    click(&mut app, &ctx, "New tunnel");
    assert_eq!(app.tunnels.spec.name, "Research node");
    assert!(
        app.tunnels.pending.is_none(),
        "Tab navigation must not start a tunnel"
    );
    app.workspace
        .as_ref()
        .unwrap()
        .remote
        .as_ref()
        .unwrap()
        .cleanup();
}

#[test]
fn tunnel_modal_keeps_navigation_and_actions_visible_across_sizes_and_themes() {
    for theme in ["phosphor", "carbon", "studio", "afterglow"] {
        for scale in [0.75, 1., 1.5] {
            for size in [
                egui::vec2(390., 540.),
                egui::vec2(760., 700.),
                egui::vec2(1440., 1000.),
            ] {
                let ctx = egui::Context::default();
                let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                let mut app = EditorApp::new(&cc, Startup::default());
                app.select_theme(theme, &ctx);
                ctx.set_zoom_factor(scale);
                app.workspace = Some(crate::app::tests::remote_workspace());
                app.dialog = Dialog::Tunnels;
                app.tunnels.available = (0..8)
                    .map(|n| Info {
                        id: format!("retained-{n}"),
                        hostname: "compute-node-with-a-long-name.cluster.example.org".into(),
                        spec: Spec {
                            name: "Long compute node name for research and analysis".into(),
                            relay_host: format!("{}example.org", "long-relay-hostname.".repeat(6)),
                            relay_user: "researcher".into(),
                            ..Default::default()
                        },
                        status: match n {
                            0 => Status::Connected,
                            1 => Status::Reconnecting,
                            _ => Status::HostKeyRequired,
                        },
                        error: if n == 1 {
                            LISTENER_ERROR.into()
                        } else {
                            String::new()
                        },
                        observed_key: String::new(),
                        fingerprint: format!("SHA256:{}", "A".repeat(43)),
                        attempts: 1,
                    })
                    .collect();
                app.tunnels.stop = Some("retained-0".into());
                app.tunnels.spec.listen_on = ListenOn::AllInterfaces;
                app.save_tunnel_configuration(
                    Spec {
                        name: "Saved compute setup".into(),
                        relay_host: "relay.example.org".into(),
                        relay_user: "researcher".into(),
                        ..Default::default()
                    },
                    false,
                );
                for page in [Page::Tunnels, Page::Saved, Page::New] {
                    app.tunnels.page = page;
                    for _ in 0..3 {
                        dialog_frame(&mut app, &ctx, size, vec![]);
                    }
                    let output = dialog_frame(&mut app, &ctx, size, vec![]);
                    let screen = ctx.content_rect();
                    for label in ["New tunnel", "Tunnels (8)", "Saved (1)", "Close"]
                        .into_iter()
                        .chain((page == Page::New).then_some("Start tunnel"))
                    {
                        let (rect, clip) = text_bounds(&output, label);
                        assert!(
                            screen.contains_rect(rect) && clip.contains_rect(rect),
                            "{theme}/{page:?} at {size:?}, scale {scale}: {label} is clipped: {rect:?}, screen {screen:?}, clip {clip:?}"
                        );
                    }
                    for label in ["Close"]
                        .into_iter()
                        .chain((page == Page::New).then_some("Start tunnel"))
                        .chain((page == Page::New).then_some("Save configuration"))
                    {
                        let (rect, clip) = layout::tests::button_frame(&output, label);
                        assert!(
                            screen.contains_rect(rect) && clip.contains_rect(rect),
                            "Full {label} button clipped: {theme}/{page:?}/{size:?}/{scale}: {rect:?}"
                        );
                    }
                    // Scroll only the body: navigation and actions must stay put.
                    let (before, _) = text_bounds(&output, "Close");
                    let pos = screen.center();
                    dialog_frame(
                        &mut app,
                        &ctx,
                        size,
                        vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta: egui::vec2(0., -2000.),
                                modifiers: Default::default(),
                            },
                        ],
                    );
                    for _ in 0..4 {
                        dialog_frame(&mut app, &ctx, size, vec![]);
                    }
                    let output = dialog_frame(&mut app, &ctx, size, vec![]);
                    let (after, clip) = text_bounds(&output, "Close");
                    assert!(
                        (before.top() - after.top()).abs() <= 1.,
                        "Footer moved while scrolling: {theme}/{page:?}/{size:?}/{scale}: {before:?} -> {after:?}"
                    );
                    assert!(screen.contains_rect(after) && clip.contains_rect(after));
                }
                assert!(
                    app.tunnels.pending.is_none(),
                    "Rendering must not start host work"
                );
                app.workspace
                    .as_ref()
                    .unwrap()
                    .remote
                    .as_ref()
                    .unwrap()
                    .cleanup();
            }
        }
    }
}

#[derive(Default)]
struct TunnelStorage(HashMap<String, String>);
impl eframe::Storage for TunnelStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn flush(&mut self) {}
}

fn saved_spec() -> Spec {
    Spec {
        name: "Compute access".into(),
        relay_host: "relay.example.org".into(),
        relay_user: "researcher".into(),
        identity: "~/keys/compute-relay".into(),
        relay_key:
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ"
                .into(),
        ..Default::default()
    }
}

#[test]
fn saved_tunnel_setup_survives_restart_and_supports_load_rename_and_confirmed_deletion() {
    use crate::ssh::tunnels::STORAGE_KEY;
    for remote in [false, true] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        if remote {
            app.workspace = Some(crate::app::tests::remote_workspace());
        }
        app.dialog = Dialog::Tunnels;
        app.tunnels.page = Page::New;
        app.tunnels.spec = saved_spec();
        app.tunnels.relay_jump = "desktop-relay-alias".into();
        click(&mut app, &ctx, "Save configuration");
        let id = app.tunnels.saved_id.clone().unwrap();
        assert_eq!(app.tunnels.saved.entries.len(), 1);
        assert_eq!(app.tunnels.saved.entries[&id].spec, saved_spec());
        assert!(app.tunnels.pending.is_none() && app.tunnels.available.is_empty());
        let mut storage = TunnelStorage::default();
        eframe::App::save(&mut app, &mut storage);
        assert!(storage.0.contains_key(STORAGE_KEY));

        let mut cc = eframe::CreationContext::_new_kittest(ctx.clone());
        cc.storage = Some(&storage);
        let mut reopened = EditorApp::new(&cc, Startup::default());
        if remote {
            reopened.workspace = app.workspace.take();
        }
        reopened.tunnel_tick(&ctx); // Workspace generation changes retain the library.
        reopened.dialog = Dialog::Tunnels;
        reopened.tunnels.page = Page::Saved;
        click(&mut reopened, &ctx, "Load setup");
        assert_eq!(reopened.tunnels.page, Page::New);
        assert_eq!(reopened.tunnels.spec, saved_spec());
        assert_eq!(reopened.tunnels.relay_jump, "desktop-relay-alias");
        assert!(reopened.tunnels.pending.is_none());
        reopened.tunnels.spec.name = "Renamed setup".into();
        reopened.tunnels.spec.listen_port = 22444;
        click(&mut reopened, &ctx, "Update configuration");
        assert_eq!(reopened.tunnels.saved_id.as_deref(), Some(id.as_str()));
        assert_eq!(reopened.tunnels.saved.entries.len(), 1);
        assert_eq!(reopened.tunnels.saved.entries[&id].spec.listen_port, 22444);
        click(&mut reopened, &ctx, "Saved (1)");
        click(&mut reopened, &ctx, "Delete…");
        click(&mut reopened, &ctx, "Cancel");
        assert!(reopened.tunnels.saved.entries.contains_key(&id));
        click(&mut reopened, &ctx, "Delete…");
        click(&mut reopened, &ctx, "Delete configuration");
        assert!(reopened.tunnels.saved.entries.is_empty());
        assert!(reopened.tunnels.saved_id.is_none());
        assert_eq!(reopened.tunnels.spec.name, "Renamed setup");
        assert!(reopened.tunnels.pending.is_none());
        eframe::App::save(&mut reopened, &mut storage);
        assert_eq!(storage.0[STORAGE_KEY], "{}");
        if let Some(remote) = reopened.workspace.as_ref().and_then(|w| w.remote.as_ref()) {
            remote.cleanup();
        }
    }
}

#[test]
fn saving_a_running_tunnel_keeps_only_its_settings_and_never_trusts_an_observed_key() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.dialog = Dialog::Tunnels;
    app.tunnels.available = vec![Info {
        id: "retained".into(),
        hostname: "compute".into(),
        spec: Spec {
            relay_key: String::new(),
            ..saved_spec()
        },
        status: Status::HostKeyRequired,
        error: String::new(),
        observed_key: saved_spec().relay_key,
        fingerprint: "SHA256:fixture".into(),
        attempts: 1,
    }];
    click(&mut app, &ctx, "Save configuration");
    let entry = app.tunnels.saved.entries.values().next().unwrap();
    assert!(
        entry.spec.relay_key.is_empty(),
        "Saving is not relay trust confirmation"
    );
    assert_eq!(entry.spec, app.tunnels.available[0].spec);
    assert!(app.tunnels.pending.is_none());
    let id = app.tunnels.saved.entries.keys().next().unwrap().clone();
    app.delete_tunnel_configuration(&id);
    assert_eq!(app.tunnels.available[0].id, "retained");
    assert!(app.tunnels.pending.is_none());
}

#[test]
fn saved_tunnel_loads_are_bound_to_the_selected_host_account_port_and_config() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("widgets.json");
    let ctx = egui::Context::default();
    ui_probe::install_at(&ctx, path.clone());
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace = Some(crate::app::tests::remote_workspace());
    app.save_tunnel_configuration(saved_spec(), false);
    let id = app.tunnels.saved.entries.keys().next().unwrap().clone();
    let profile = app
        .workspace
        .as_ref()
        .unwrap()
        .remote
        .as_ref()
        .unwrap()
        .profile
        .clone();
    for field in ["host", "user", "port", "config"] {
        let remote = app.workspace.as_mut().unwrap().remote.as_mut().unwrap();
        remote.profile = profile.clone();
        match field {
            "host" => remote.profile.host = "other-host".into(),
            "user" => remote.profile.user = "other-user".into(),
            "port" => remote.profile.port = "2222".into(),
            "config" => remote.profile.config = "~/.ssh/other-config".into(),
            _ => unreachable!(),
        }
        app.dialog = Dialog::Tunnels;
        app.tunnels.page = Page::Saved;
        dialog_frame(&mut app, &ctx, egui::vec2(900., 1000.), vec![]);
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(
            state["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|button| button["name"] == format!("tunnel-saved-load:{id}")
                    && button["enabled"] == false)
        );
        let before = app.tunnels.spec.clone();
        app.load_tunnel_configuration(&id);
        assert_eq!(app.tunnels.page, Page::Saved);
        assert_eq!(app.tunnels.spec, before);
        assert!(app.tunnels.error.contains("before loading"));
        assert!(app.tunnels.pending.is_none());
    }
    app.workspace
        .as_mut()
        .unwrap()
        .remote
        .as_mut()
        .unwrap()
        .profile = profile;
    app.generation += 1;
    app.dialog = Dialog::None;
    app.tunnel_tick(&ctx);
    assert!(app.tunnels.saved.entries.contains_key(&id));
    app.load_tunnel_configuration(&id);
    assert_eq!(app.tunnels.spec, saved_spec());
    app.workspace
        .as_ref()
        .unwrap()
        .remote
        .as_ref()
        .unwrap()
        .cleanup();
}

#[test]
fn malformed_saved_tunnels_are_preserved_and_cannot_be_overwritten_by_an_empty_window() {
    let mut storage = TunnelStorage(HashMap::from([(
        crate::ssh::tunnels::STORAGE_KEY.into(),
        "malformed".into(),
    )]));
    let mut warnings = vec![];
    let mut tunnels = Tunnels::load(Some(&storage), &mut warnings);
    assert_eq!(warnings.len(), 1);
    assert!(tunnels.saved.error.is_some());
    tunnels.store(&mut storage);
    assert_eq!(storage.0[crate::ssh::tunnels::STORAGE_KEY], "malformed");
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx);
    let mut app = EditorApp::new(&cc, Startup::default());
    app.tunnels = tunnels;
    app.save_tunnel_configuration(saved_spec(), true);
    assert!(app.tunnels.saved.entries.is_empty());
    assert!(app.tunnels.error.contains("Repair saved tunnel data"));
    app.tunnels.store(&mut storage);
    assert_eq!(storage.0[crate::ssh::tunnels::STORAGE_KEY], "malformed");
}

#[test]
fn network_listener_choice_survives_saving_and_offers_direct_connection_details() {
    for remote in [false, true] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        if remote {
            app.workspace = Some(crate::app::tests::remote_workspace());
        }
        app.dialog = Dialog::Tunnels;
        app.tunnels.page = Page::New;
        app.tunnels.spec = saved_spec();
        assert_eq!(app.tunnels.spec.listen_on, ListenOn::Loopback);
        click(&mut app, &ctx, "Localhost only");
        click(&mut app, &ctx, "All network interfaces");
        assert_eq!(app.tunnels.spec.listen_on, ListenOn::AllInterfaces);
        click(&mut app, &ctx, "Save configuration");
        let id = app.tunnels.saved_id.clone().unwrap();
        let mut storage = TunnelStorage::default();
        app.tunnels.store(&mut storage);
        let mut warnings = vec![];
        let restored = Tunnels::load(Some(&storage), &mut warnings);
        assert!(warnings.is_empty());
        assert_eq!(
            restored.saved.entries[&id].spec.listen_on,
            ListenOn::AllInterfaces
        );
        app.tunnels.spec.listen_on = ListenOn::Loopback;
        app.load_tunnel_configuration(&id);
        assert_eq!(app.tunnels.spec.listen_on, ListenOn::AllInterfaces);
        app.tunnels.available = vec![Info {
            id: "fixture".into(),
            hostname: "compute".into(),
            spec: app.tunnels.spec.clone(),
            status: Status::Connected,
            error: String::new(),
            observed_key: String::new(),
            fingerprint: String::new(),
            attempts: 1,
        }];
        app.tunnels.page = Page::Tunnels;
        let output = click(&mut app, &ctx, "Copy connection details");
        assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text.contains("Host: relay.example.org\nPort: 22022\nJump / ProxyJump: leave blank") && text.contains("Host: 127.0.0.1\nPort: 22022") && text.contains("verify its host key"))));
        assert!(app.tunnels.pending.is_none());
        if let Some(remote) = app
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.remote.as_ref())
        {
            remote.cleanup();
        }
    }
}

#[test]
fn older_services_cannot_silently_ignore_a_requested_network_listener() {
    let old_spec: Spec = serde_json::from_value(serde_json::json!({
        "relay_host": "relay.example.org", "relay_user": "researcher"
    }))
    .unwrap();
    let reply = rode_session::Response::Tunnel(Box::new(Info {
        id: "fixture".into(),
        hostname: "compute".into(),
        spec: old_spec,
        status: Status::Connecting,
        error: String::new(),
        observed_key: String::new(),
        fingerprint: String::new(),
        attempts: 1,
    }));
    check_started_scope(&reply, ListenOn::Loopback).unwrap();
    let message = check_started_scope(&reply, ListenOn::AllInterfaces)
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("service did not apply") && message.contains("Update/restart"),
        "{message}"
    );
}

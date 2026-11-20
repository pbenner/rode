use super::*;

fn app() -> (EditorApp, egui::Context) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    app.reveal_panel(Panel::Transfers, &ctx);
    for (side, path) in [(0, "/local"), (1, "/remote")] {
        app.transfers.panes[side].browser.loaded(files::Response {
            path: path.into(),
            entries: vec![Entry {
                name: "file.txt".into(),
                path: format!("{path}/file.txt"),
                dir: false,
            }],
            ..Default::default()
        });
    }
    (app, ctx)
}

#[test]
fn extension_visibility_and_stale_listings_preserve_workspace_authority() {
    let (mut app, ctx) = app();
    let root = app.workspace.as_ref().unwrap().root.clone();
    assert!(app.command_visible(Command::Panel(Panel::Transfers)));
    let generation = app.transfers.generation;
    app.set_extension(crate::extensions::Feature::Transfers.id(), false);
    assert!(!app.command_visible(Command::Panel(Panel::Transfers)));
    app.transfers
        .send
        .send(Event::Listed {
            generation,
            side: 1,
            token: 0,
            path: "/unrelated".into(),
            result: Ok(Box::new(files::Response {
                path: "/unrelated".into(),
                ..Default::default()
            })),
        })
        .unwrap();
    app.transfer_tick(&ctx);
    assert!(app.transfers.panes[1].browser.directory.is_empty());
    assert_eq!(app.workspace.as_ref().unwrap().root, root);
    app.set_extension(crate::extensions::Feature::Transfers.id(), true);
    assert!(app.command_visible(Command::Panel(Panel::Transfers)));
    app.workspace.set_fixture(None);
    assert!(app.command_visible(Command::Panel(Panel::Transfers)));
}

#[test]
fn queue_admission_enforces_the_total_path_budget_for_every_enqueue() {
    let (mut app, _) = app();
    let task = |paths| Task {
        job: Job {
            from: Endpoint::local(),
            to: Endpoint::local(),
            paths,
            directory: "/target".into(),
            protected: Arc::default(),
        },
        label: "Copy".into(),
        _work: app.vaults.work(),
    };
    for _ in 0..2 {
        assert!(app.transfers.enqueue(task(vec!["x".repeat(2048); 4096])));
    }
    assert!(!app.transfers.enqueue(task(vec!["x".into()])));
    assert_eq!(app.transfers.queue.len(), 2);
    assert!(app.transfers.error.is_some());
}

#[test]
fn reset_hides_old_conflicts_and_late_copy_completion_cannot_restore_history() {
    let (mut app, ctx) = app();
    let (answer, _receive) = mpsc::sync_channel(1);
    app.transfers.rename = "old name".into();
    app.transfers.remaining = true;
    app.transfers.running = Some(Running {
        id: 42,
        generation: app.transfers.generation,
        job: Job {
            from: Endpoint::local(),
            to: Endpoint::local(),
            paths: vec!["/old/file".into()],
            directory: "/old-target".into(),
            protected: Arc::default(),
        },
        label: "Old copy".into(),
        cancel: Arc::default(),
        progress: Arc::new(Mutex::new(Progress {
            conflict: Some(copy::Conflict {
                path: "/old-target/file".into(),
                regular: true,
                answer,
            }),
            ..Default::default()
        })),
    });
    app.transfers.reset();
    assert!(
        app.transfers.running.is_some(),
        "Cleanup still owns the running slot"
    );
    let output = ctx.run(Default::default(), |ctx| app.transfer_windows(ctx));
    assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::Shape::Text(text) if text.galley.text() == "File already exists")));
    app.transfers
        .send
        .send(Event::Finished {
            id: 42,
            result: Err("Old error".into()),
        })
        .unwrap();
    app.transfer_tick(&ctx);
    assert!(app.transfers.running.is_none());
    assert!(app.transfers.history.is_empty());
    assert!(app.transfers.rename.is_empty());
    assert!(!app.transfers.remaining);
}

#[test]
fn normal_exit_waits_for_transfer_cleanup() {
    let (mut app, ctx) = app();
    let pending = Pending::new(&app.transfers.pending);
    app.request_exit(&ctx);
    assert!(app.exit.waiting());
    assert!(!app.exit.closing());
    drop(pending);
    app.request_exit(&ctx);
    assert!(app.exit.closing());
}

#[test]
fn compact_and_scaled_browsers_keep_separate_nonoverlapping_controls() {
    for (width, height, scale) in [(1380., 900., 1.), (850., 560., 1.), (850., 560., 1.5)] {
        let (mut app, ctx) = app();
        ctx.set_pixels_per_point(scale);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("layout.json");
        ui_probe::install_at(&ctx, path.clone());
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, height),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    app.navigation.begin_layout();
                    app.titlebar(ctx);
                    app.workspace_banner(ctx);
                    app.activity(ctx);
                    egui::SidePanel::left("fixture-explorer")
                        .exact_width(250.)
                        .show(ctx, |_| {});
                    app.workspace_content(ctx);
                },
            );
        }
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let buttons = snapshot["buttons"].as_array().unwrap();
        let rect = |name: &str| {
            buttons
                .iter()
                .find(|button| button["name"] == name)
                .unwrap_or_else(|| panic!("Missing {name} at {width}x{height}/{scale}: {snapshot}"))
                ["rect"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect::<Vec<_>>()
        };
        let local = rect("transfer-path:0");
        let remote = rect("transfer-path:1");
        assert!(local[2] <= remote[0], "Browsers overlap: {snapshot}");
        for (side, copy) in [(0, "transfer-upload"), (1, "transfer-download")] {
            let path = rect(&format!("transfer-path:{side}"));
            let filter = rect(&format!("transfer-filter:{side}"));
            let drop = rect(&format!("transfer-drop:{side}"));
            let action = rect(copy);
            assert!(
                path[3] <= filter[1] && filter[3] <= drop[1] && drop[3] <= action[1],
                "Browser rows overlap the copy action: {snapshot}"
            );
            assert!(
                drop[3] - drop[1] >= 30.,
                "No usable file list at {width}x{height}: {snapshot}"
            );
        }
        for control in [
            local,
            remote,
            rect("transfer-upload"),
            rect("transfer-download"),
        ] {
            assert!(
                control[0] >= 0.
                    && control[1] >= 0.
                    && control[2] <= width as f64
                    && control[3] <= height as f64,
                "Control outside viewport: {snapshot}"
            );
        }
        assert!(buttons.iter().any(|b| b["name"] == "transfer-idle"));
    }
}

#[test]
fn drag_snapshots_reject_same_side_stale_listings_and_changed_workspaces() {
    let (mut app, _) = app();
    let drag = drag::Files {
        side: 0,
        generation: app.transfers.generation,
        token: app.transfers.panes[0].browser.token,
        directory: "/local".into(),
        paths: vec!["/local/file.txt".into()],
    };
    assert!(drag.valid(&app.transfers, 1));
    assert!(!drag.valid(&app.transfers, 0));
    app.transfers.panes[0].browser.token += 1;
    assert!(!drag.valid(&app.transfers, 1));
    app.transfers.panes[0].browser.token -= 1;
    app.transfers.panes[1].browser.location = "/typed destination".into();
    assert!(!drag.valid(&app.transfers, 1));
    app.transfers.panes[1].browser.location = "/remote".into();
    let mut unknown = drag.clone();
    unknown.paths.push("/not-listed".into());
    assert!(!unknown.valid(&app.transfers, 1));
    app.transfers.reset();
    assert!(!drag.valid(&app.transfers, 1));
}

#[test]
fn desktop_drops_require_real_absolute_paths_and_listed_destinations() {
    let file = |path: Option<&str>| egui::DroppedFile {
        path: path.map(PathBuf::from),
        ..Default::default()
    };
    assert_eq!(
        drag::desktop_paths(&[file(Some("/local/日本.txt"))]).unwrap(),
        vec!["/local/日本.txt"]
    );
    assert!(drag::desktop_paths(&[file(None)]).is_err());
    assert!(drag::desktop_paths(&[file(Some("relative.txt"))]).is_err());
    let (mut app, ctx) = app();
    app.queue_transfer_to(0, vec!["/local/file.txt".into()], "/unlisted".into(), &ctx);
    assert!(app.transfers.queue.is_empty());
    assert!(app.transfers.running.is_none());
    assert!(
        app.transfers
            .error
            .as_ref()
            .unwrap()
            .contains("destination folder changed")
    );
}

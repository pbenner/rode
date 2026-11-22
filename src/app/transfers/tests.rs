use super::*;

fn app() -> (EditorApp, egui::Context) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    app.reveal_panel(Panel::Transfers, &ctx);
    for (side, path) in [(Side::Local, "/local"), (Side::Remote, "/remote")] {
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
    app.transfers.panes[Side::Remote].view = file_browser::View::Icons;
    app.set_extension(crate::extensions::Feature::Transfers.id(), false);
    assert_eq!(
        app.transfers.panes[Side::Remote].view,
        file_browser::View::Icons
    );
    assert!(!app.command_visible(Command::Panel(Panel::Transfers)));
    app.transfers
        .send
        .send(Event::Listed {
            generation,
            side: Side::Remote,
            token: 0,
            path: "/unrelated".into(),
            result: Ok(Box::new(files::Response {
                path: "/unrelated".into(),
                ..Default::default()
            })),
        })
        .unwrap();
    app.transfer_tick(&ctx);
    assert!(
        app.transfers.panes[Side::Remote]
            .browser
            .directory
            .is_empty()
    );
    assert_eq!(app.workspace.as_ref().unwrap().root, root);
    app.set_extension(crate::extensions::Feature::Transfers.id(), true);
    assert!(app.command_visible(Command::Panel(Panel::Transfers)));
    app.workspace.set_fixture(None);
    assert!(app.command_visible(Command::Panel(Panel::Transfers)));
}

#[test]
fn moving_the_transfer_view_preserves_listings_selection_and_running_copy() {
    let (mut app, ctx) = app();
    app.transfers.panes[Side::Local].view = file_browser::View::Icons;
    app.transfers.panes[Side::Local].query = "file".into();
    app.transfers.panes[Side::Local]
        .selected
        .insert("/local/file.txt".into());
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(Progress::default()));
    let generation = app.transfers.generation;
    app.transfers.running = Some(Running {
        id: 42,
        generation,
        job: Job {
            from: Endpoint::local(),
            to: Endpoint::local(),
            paths: vec!["/local/file.txt".into()],
            directory: "/remote".into(),
            protected: Arc::default(),
        },
        label: "Copy".into(),
        cancel: cancel.clone(),
        progress: progress.clone(),
    });
    for _ in 0..2 {
        app.detach_tab(areas::Area::Transfers.tab().unwrap(), &ctx);
        app.reattach_tool_tab(ToolTab::Panel(Panel::Transfers), &ctx);
        assert_eq!(app.transfers.generation, generation);
        assert_eq!(app.transfers.panes[Side::Local].browser.directory, "/local");
        assert_eq!(
            app.transfers.panes[Side::Remote].browser.directory,
            "/remote"
        );
        assert_eq!(
            app.transfers.panes[Side::Local].view,
            file_browser::View::Icons
        );
        assert_eq!(app.transfers.panes[Side::Local].query, "file");
        assert!(
            app.transfers.panes[Side::Local]
                .selected
                .contains("/local/file.txt")
        );
        let running = app.transfers.running.as_ref().unwrap();
        assert_eq!(running.id, 42);
        assert!(Arc::ptr_eq(&running.progress, &progress));
        assert!(Arc::ptr_eq(&running.cancel, &cancel));
        assert!(!cancel.load(Ordering::Acquire));
    }
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
    for (width, height, scale, view) in [
        (1380., 900., 1., file_browser::View::List),
        (850., 560., 1., file_browser::View::List),
        (850., 560., 1.5, file_browser::View::List),
        (1380., 900., 1., file_browser::View::Icons),
        (850., 560., 1., file_browser::View::Icons),
        (850., 560., 1.5, file_browser::View::Icons),
    ] {
        let (mut app, ctx) = app();
        for pane in app.transfers.panes.iter_mut() {
            pane.view = view;
        }
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
                    app.update_window_title(ctx);
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
        let divider = rect("transfer-browser-resize");
        assert!(local[2] <= divider[0] && divider[2] <= remote[0]);
        for (side, copy) in [
            (Side::Local, "transfer-upload"),
            (Side::Remote, "transfer-download"),
        ] {
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
            rect("transfer-view-list:0"),
            rect("transfer-view-icons:0"),
            rect("transfer-view-list:1"),
            rect("transfer-view-icons:1"),
        ] {
            assert!(
                control[0] >= 0.
                    && control[1] >= 0.
                    && control[2] <= width as f64
                    && control[3] <= height as f64,
                "Control outside viewport: {snapshot}"
            );
        }
        if let Some(queue) = buttons
            .iter()
            .find(|b| b["name"] == "transfer-queue-resize")
        {
            let top = queue["rect"][1].as_f64().unwrap();
            assert!(rect("transfer-upload")[3] <= top && rect("transfer-download")[3] <= top);
        }
        assert!(buttons.iter().any(|b| b["name"] == "transfer-idle"));
    }
}

#[test]
fn drag_snapshots_reject_same_side_stale_listings_and_changed_workspaces() {
    let (mut app, _) = app();
    let drag = drag::Files {
        side: Side::Local,
        generation: app.transfers.generation,
        token: app.transfers.panes[Side::Local].browser.token,
        directory: "/local".into(),
        paths: vec!["/local/file.txt".into()],
    };
    assert!(drag.valid(&app.transfers, Side::Remote));
    assert!(!drag.valid(&app.transfers, Side::Local));
    app.transfers.panes[Side::Local].browser.token += 1;
    assert!(!drag.valid(&app.transfers, Side::Remote));
    app.transfers.panes[Side::Local].browser.token -= 1;
    app.transfers.panes[Side::Remote].browser.location = "/typed destination".into();
    assert!(!drag.valid(&app.transfers, Side::Remote));
    app.transfers.panes[Side::Remote].browser.location = "/remote".into();
    let mut unknown = drag.clone();
    unknown.paths.push("/not-listed".into());
    assert!(!unknown.valid(&app.transfers, Side::Remote));
    app.transfers.reset();
    assert!(!drag.valid(&app.transfers, Side::Remote));
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
    app.queue_transfer_to(
        Side::Local,
        vec!["/local/file.txt".into()],
        "/unlisted".into(),
        &ctx,
    );
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

#[test]
fn dividers_resize_by_keyboard_and_drag_reset_and_survive_compact_windows() {
    let (mut app, ctx) = app();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("layout.json");
    ui_probe::install_at(&ctx, path.clone());
    let mut time = 0.;
    let mut render = |width, height, events| {
        // Separate unrelated gestures while keeping the deliberate pair of
        // clicks within egui's double-click delay (which tracks clicks globally).
        time += 0.1;
        let _ = ctx.run(
            egui::RawInput {
                time: Some(time),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, height),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.transfers_ui(ui, ctx));
            },
        );
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap()
    };
    let rect = |state: &serde_json::Value, name: &str| {
        let control = state["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|button| button["name"] == name)
            .unwrap_or_else(|| panic!("Missing {name}: {state}"));
        let r = &control["rect"];
        egui::Rect::from_min_max(
            egui::pos2(r[0].as_f64().unwrap() as f32, r[1].as_f64().unwrap() as f32),
            egui::pos2(r[2].as_f64().unwrap() as f32, r[3].as_f64().unwrap() as f32),
        )
    };
    let pointer = |pos, pressed| egui::Event::PointerButton {
        pos,
        pressed,
        button: egui::PointerButton::Primary,
        modifiers: egui::Modifiers::NONE,
    };
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    render(1100., 800., vec![]);
    let initial = render(1100., 800., vec![]);
    for (name, arrow, delta) in [
        (
            "transfer-browser-resize",
            egui::Key::ArrowRight,
            egui::vec2(60., 0.),
        ),
        (
            "transfer-queue-resize",
            egui::Key::ArrowUp,
            egui::vec2(0., -60.),
        ),
    ] {
        let baseline = rect(&initial, name).center();
        // Clicking a grip must make its keyboard controls available immediately.
        render(
            1100.,
            800.,
            vec![egui::Event::PointerMoved(baseline), pointer(baseline, true)],
        );
        let focused = render(1100., 800., vec![pointer(baseline, false)]);
        assert!(
            focused["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["name"] == name && b["focused"] == true)
        );
        render(1100., 800., vec![key(arrow), key(arrow)]);
        let enlarged = render(1100., 800., vec![]);
        assert_eq!(rect(&enlarged, name).center(), baseline + delta * (2. / 3.));
        render(1100., 800., vec![key(egui::Key::Home)]);
        let reset = render(1100., 800., vec![]);
        assert_eq!(rect(&reset, name).center(), baseline);
        render(
            1100.,
            800.,
            vec![egui::Event::PointerMoved(baseline), pointer(baseline, true)],
        );
        let moved = baseline + delta;
        render(1100., 800., vec![egui::Event::PointerMoved(moved)]);
        render(1100., 800., vec![pointer(moved, false)]);
        let dragged = render(1100., 800., vec![]);
        assert_eq!(rect(&dragged, name).center(), moved);
        // The queue grip disappears in compact mode; neither preference is reset.
        let compact = render(850., 400., vec![]);
        assert!(
            !compact["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["name"] == "transfer-queue-resize")
        );
        render(1100., 800., vec![]);
        let restored = render(1100., 800., vec![]);
        assert!((rect(&restored, name).center() - moved).length() <= 1.);
        // Double-click resets exactly as Home does, after a genuine drag.
        let grip = rect(&restored, name).center();
        for _ in 0..2 {
            render(
                1100.,
                800.,
                vec![egui::Event::PointerMoved(grip), pointer(grip, true)],
            );
            render(1100., 800., vec![pointer(grip, false)]);
        }
        let reset = render(1100., 800., vec![]);
        assert!(
            (rect(&reset, name).center() - baseline).length() <= 1.,
            "{name}: double-click reset to {:?}, expected {baseline:?}",
            rect(&reset, name).center()
        );
    }
}

#[test]
fn completion_refresh_does_not_submit_a_folder_path_that_is_still_being_typed() {
    let (mut app, ctx) = app();
    let pane = &mut app.transfers.panes[Side::Local];
    pane.browser.needs_load = true;
    pane.browser.location = "/unfinished-folder".into();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.transfers_ui(ui, ctx));
    });
    let pane = &app.transfers.panes[Side::Local];
    assert_eq!(pane.browser.directory, "/local");
    assert_eq!(pane.browser.location, "/unfinished-folder");
    assert!(pane.browser.needs_load);
    assert!(!pane.browser.loading);
}

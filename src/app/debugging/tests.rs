//! Debugger UI lifecycle and stale-source regressions using deterministic controller fixtures.

use super::*;

#[test]
fn source_guards_and_panel_do_not_start_a_process() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.toggle_panel(Panel::Debug, &ctx);
    assert!(app.debugger.client.is_none());
    app.workspace = Some(crate::app::tests::remote_workspace());
    app.new_file();
    app.documents[0].text = "keep this unsaved source".into();
    app.documents[0].modified = true;
    app.start_debugger(&ctx);
    assert!(app.debugger.client.is_none());
    assert!(app.documents[0].dirty());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .0
            .contains("Save modified sources")
    );
    app.set_extension("rode.debugger", false);
    app.start_debugger(&ctx);
    assert!(app.debugger.client.is_none());
    assert!(app.notice.as_ref().unwrap().0.contains("Enable Debugger"));
}
#[test]
fn closing_waits_for_debug_cleanup_then_completes_without_an_event() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let flag = Arc::new(AtomicBool::new(true));
    app.debugger.cleanups.push(flag.clone());
    app.quit_terminals(&ctx);
    assert!(!app.quitting);
    assert!(app.session_exit_pending());
    flag.store(false, Ordering::Release);
    app.session_tick(&ctx);
    assert!(app.quitting);
}
/// Reproducible render-only profile, without a display or installed debugger.
/// Explicitly ignored: reports timings rather than asserting machine speed.
#[test]
#[ignore = "manual debugger UI performance profile"]
fn debugger_frame_profile() {
    use serde_json::json;
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let mut settings = debugger::Settings {
        watches: (0..32).map(|i| format!("value_{i}")).collect(),
        ..Default::default()
    };
    for i in 0..128 {
        settings.breakpoints.insert(
            format!("/project/source_{i}.rs"),
            (1..=8)
                .map(|line| debugger::Breakpoint {
                    line,
                    condition: "condition > 0".repeat(2),
                    ..Default::default()
                })
                .collect(),
        );
    }
    settings.config.extra = json!({"padding":"x".repeat(32*1024)});
    settings.validate().unwrap();
    app.workbench.save_debugger_settings(settings.clone());
    let mut client = Client::default();
    client.settings = settings;
    client.phase = Phase::Paused;
    client.frames = vec![json!({"id":1,"name":"main","line":3})];
    client.frame = Some(1);
    client.watch_values = vec!["42".into(); 32];
    for id in 1..=8 {
        client
            .scopes
            .push(json!({"name":format!("Scope {id}"),"variablesReference":id,"expensive":false}));
        client.variables.insert(id,(0..200).map(|i|json!({"name":format!("var_{i}"),"value":"x".repeat(256),"variablesReference":0})).collect());
    }
    client.log(&"example debugger output\n".repeat(8000));
    app.debugger.client = Some(client);
    let mut sample = Vec::new();
    for i in 0..110 {
        let started = Instant::now();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(390., 900.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.debugger_ui(ui, ctx));
            },
        );
        if i >= 10 {
            sample.push(started.elapsed().as_secs_f64() * 1000.);
        }
    }
    sample.sort_by(f64::total_cmp);
    println!(
        "Debugger dense view: median {:.3} ms, p95 {:.3} ms (100 warmed frames, 1,600 variables / 1,024 breakpoints)",
        sample[50], sample[95]
    );
    app.stop_debugger();
}

#[test]
fn invalid_configuration_preserves_the_last_saved_settings() {
    let mut workbench = Workbench::default();
    let saved = debugger::Settings {
        watches: vec!["answer".into()],
        ..Default::default()
    };
    workbench.save_debugger_settings(saved.clone());
    workbench
        .edit_debugger_settings()
        .watches
        .push("\0".repeat(5000));
    assert!(workbench.commit_debugger_settings().is_err());
    assert_eq!(workbench.debugger_settings(), &saved);
}
#[test]
fn delayed_source_navigation_waits_for_old_reads_and_cancels_on_resume() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].path = Some("/new.rs".into());
    let selected = app.documents[0].id;
    app.new_file();
    let unchanged = app.active;
    let mut client = Client::default();
    client.phase = Phase::Paused;
    app.debugger.pending_jump = Some(navigation::Target::new(
        "/new.rs".into(),
        1,
        1,
        client.inspection_epoch(),
    ));
    app.debugger.source_loading = Some("/old.rs".into());
    app.opening.insert("/old.rs".into());
    app.resolve_debug_source(&client, &ctx);
    assert_eq!(app.active, unchanged);
    app.opening.clear();
    app.resolve_debug_source(&client, &ctx);
    assert_eq!(app.documents[app.active].id, selected);
    app.active = unchanged;
    app.debugger.pending_jump = Some(navigation::Target::new(
        "/new.rs".into(),
        1,
        1,
        client.inspection_epoch(),
    ));
    client.phase = Phase::Running;
    app.resolve_debug_source(&client, &ctx);
    assert_eq!(app.active, unchanged);
    assert!(app.debugger.pending_jump.is_none());
}
#[test]
fn configuration_draft_keeps_incomplete_json_without_reparsing_on_draw() {
    let mut draft = configuration::Draft::default();
    draft.load(&debugger::Config::default());
    draft.text = "{".into();
    draft.parse();
    assert!(draft.validated().is_err());
    assert_eq!(draft.text, "{");
    draft.text = "{\"backend\":\"python\",\"program\":\"main.py\"}".into();
    draft.parse();
    assert_eq!(draft.validated().unwrap().program, "main.py");
}

#[test]
fn deleted_breakpoints_do_not_consume_the_source_limit() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx);
    let mut app = EditorApp::new(&cc, Startup::default());
    for i in 0..140 {
        let path = format!("/project/file-{i}.rs");
        app.toggle_breakpoint(path.clone(), 1);
        app.toggle_breakpoint(path, 1);
    }
    assert!(app.workbench.debugger_settings().breakpoints.is_empty());
}

#[test]
fn closed_terminal_views_release_retained_debug_streams() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let (terminal, stream) = Terminal::stream(&ctx, "debug test".into(), |_| {});
    let alive = terminal.alive.clone();
    app.add_terminal_view(terminal, "debug-view".into(), "debug test".into());
    app.debugger
        .streams
        .insert(1, ("debug-view".into(), stream));
    app.debugger.client = Some(Client::default());
    app.remove_view("debug-view");
    app.poll_debugger(&ctx);
    assert!(app.debugger.streams.is_empty());
    assert!(!alive.load(Ordering::Acquire));
}

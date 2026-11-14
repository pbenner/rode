//! Workbench regressions for responsive panels, captured settings and versioned results.

use super::*;

#[test]
fn sandbox_backend_selector_changes_the_saved_options() {
    let ctx = egui::Context::default();
    let mut options = sandbox::Options {
        enabled: true,
        ..Default::default()
    };
    let frame = |options: &mut sandbox::Options, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., 400.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| sandbox_backend_ui(ui, options));
            },
        )
    };
    let position = |output: &egui::FullOutput, label: &str| {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() / 2.)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing selector label: {label}"))
    };
    let click = |options: &mut sandbox::Options, pos| {
        let _ = frame(
            options,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        frame(
            options,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        )
    };
    let _ = frame(&mut options, vec![]);
    let output = frame(&mut options, vec![]);
    let _ = click(&mut options, position(&output, "bwrap"));
    let output = frame(&mut options, vec![]);
    let _ = click(&mut options, position(&output, "Landlock"));
    assert_eq!(options.backend, sandbox::Backend::Landlock);
    let saved = serde_json::to_string(&options).unwrap();
    assert_eq!(
        serde_json::from_str::<sandbox::Options>(&saved).unwrap(),
        options
    );
}

fn panel_text(app: &mut EditorApp, ctx: &egui::Context) -> String {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1380., 900.),
            )),
            ..Default::default()
        },
        |ctx| {
            app.workbench.surface = Surface::Tools;
            app.workspace_content(ctx);
        },
    )
    .shapes
    .iter()
    .filter_map(|shape| match &shape.shape {
        egui::Shape::Text(text) => Some(text.galley.text()),
        _ => None,
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn app(root: &std::path::Path) -> (EditorApp, egui::Context) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace.set_fixture(Some(Workspace {
        root: root.to_str().unwrap().into(),
        remote: None,
        connected: true,
        ready: true,
        plan: sandbox::Plan {
            root: root.to_str().unwrap().into(),
            ..Default::default()
        },
        sandbox_options: Default::default(),
    }));
    (app, ctx)
}
#[test]
fn tool_pages_fit_compact_and_detached_widths() {
    let fixture = tempfile::tempdir().unwrap();
    // Exercise wrapping independently of the invoking user's home/TMPDIR length.
    let root = fixture.path().join(format!(
        "project with spaces and Unicode ユニコード-{}",
        "long-path-".repeat(12)
    ));
    std::fs::create_dir(&root).unwrap();
    for (theme, width) in [
        ("phosphor", 360.),
        ("phosphor", 680.),
        ("phosphor", 1440.),
        ("afterglow", 360.),
        ("afterglow", 680.),
        ("afterglow", 1440.),
        ("studio", 360.),
        ("studio", 680.),
        ("studio", 1440.),
        ("studio-neutral", 360.),
        ("studio-neutral", 680.),
        ("studio-neutral", 1440.),
        ("studio-hacker", 360.),
        ("studio-hacker", 680.),
        ("studio-hacker", 1440.),
    ] {
        for scale in [0.75, 1., 1.25, 1.5] {
            for panel in [
                Panel::Git,
                Panel::Monitor,
                Panel::Problems,
                Panel::Environment,
                Panel::Tasks,
                Panel::Settings,
                Panel::Debug,
            ] {
                let (mut app, ctx) = app(&root);
                app.themes.select(theme, &ctx).unwrap();
                ctx.set_zoom_factor(scale);
                app.workbench.git.git_root = root.to_string_lossy().into();
                app.workbench.git.branch = "main".into();
                app.workbench.draft.sandbox.read_only = vec![format!(
                    "/data/ユニコード/{}measurements.csv",
                    "long-directory/".repeat(20)
                )];
                app.workbench.draft.sandbox.read_write = vec!["~/results".into()];
                let tabs = if panel == Panel::Git {
                    vec![
                        GitTab::Changes,
                        GitTab::History,
                        GitTab::Branches,
                        GitTab::Stash,
                        GitTab::Remotes,
                        GitTab::Tags,
                        GitTab::Worktrees,
                        GitTab::Submodules,
                        GitTab::Settings,
                    ]
                } else {
                    vec![GitTab::Changes]
                };
                let categories = if panel == Panel::Settings {
                    SettingsSection::ALL.to_vec()
                } else {
                    vec![SettingsSection::CodeTools]
                };
                for (tab, category) in tabs
                    .iter()
                    .flat_map(|tab| categories.iter().map(move |category| (*tab, *category)))
                {
                    app.workbench.git.git_tab = tab;
                    app.workbench.settings_section = category;
                    for _ in 0..3 {
                        let mut bounds = None;
                        let output = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, 700.),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default().show(ctx, |ui| {
                                    let available = ui.available_rect_before_wrap();
                                    let content = ui
                                        .scope(|ui| app.panel_content(panel, ui, ctx))
                                        .response
                                        .rect;
                                    bounds = Some((content, available));
                                });
                            },
                        );
                        let (content, available) = bounds.unwrap();
                        assert!(
                            content.right() <= available.right() + 1.,
                            "{theme}: {panel:?}/{tab:?}/{category:?} overflows at {width}, scale {scale}: {content:?} vs {available:?}; text: {:?}",
                            output
                                .shapes
                                .iter()
                                .filter_map(|shape| match &shape.shape {
                                    egui::Shape::Text(text)
                                        if text.pos.x + text.galley.size().x
                                            > available.right() - 12. =>
                                        Some((text.galley.text(), text.pos, text.galley.size())),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                        );
                        layout::tests::assert_button_alignment(
                            &output,
                            app.themes.active().palette,
                            ctx.pixels_per_point(),
                            &format!(
                                "{theme}/{panel:?}/{tab:?}/{category:?} at {width}, scale {scale}"
                            ),
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn opening_a_non_python_project_does_not_probe_python_and_disable_invalidates_tools() {
    use crate::extensions::Feature;
    let root = tempfile::tempdir().unwrap();
    let (mut app, ctx) = app(root.path());
    app.load_project_tools(&ctx);
    assert!(!app.workbench.python.env_busy);
    app.new_file();
    app.documents[0].text = "keep dirty".into();
    let cancel = app.extensions.cancel.clone();
    let epoch = app.workbench.epoch;
    app.set_extension(Feature::Monitor.id(), false);
    assert!(cancel.load(Ordering::Acquire));
    assert!(app.workbench.epoch > epoch);
    app.toggle_panel(Panel::Monitor, &ctx);
    assert_ne!(app.workbench.panel, Some(Panel::Monitor));
    assert_eq!(app.documents[0].text, "keep dirty");
    assert!(app.documents[0].dirty());
    app.set_extension(Feature::Python.id(), false);
    app.refresh_environment(false, &ctx);
    assert!(!app.workbench.python.env_busy);
    assert_eq!(app.terminal_python_selection(), "off");
}
#[test]
fn monitor_samples_without_a_project_and_stops_scheduling_when_hidden() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    assert!(app.workspace.is_none());
    assert!(app.monitor_ready());
    assert!(!app.project_ready());
    app.workbench_tick(&ctx);
    assert!(!app.workbench.monitoring.monitor_busy);
    app.toggle_panel(Panel::Monitor, &ctx);
    app.workbench_tick(&ctx);
    assert!(app.workbench.monitoring.monitor_busy);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.workbench.monitoring.monitor_busy && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.poll(&ctx);
    }
    assert!(
        !app.workbench.monitoring.monitor_busy,
        "Host sampling did not complete"
    );
    assert!(
        app.workbench.monitoring.monitor_error.is_empty(),
        "{}",
        app.workbench.monitoring.monitor_error
    );
    let sample = app.workbench.monitoring.monitor.as_ref().unwrap();
    assert!(!sample.cpus.is_empty());
    assert!(sample.memory_total > 0);
    assert!(sample.disk_total > 0);
    assert!(app.workspace.is_none());
    assert!(app.terminal.is_none());
    assert!(app.workbench.preferences.is_empty());
    let labels = panel_text(&mut app, &ctx);
    assert!(labels.contains("Host overview"), "{labels}");
    assert!(labels.contains("LOCAL HOST"), "{labels}");
    assert!(!labels.contains("Open a project"), "{labels}");

    // Make another sample due, then close the panel before scheduling it.
    let due = Instant::now() - Duration::from_secs(31);
    app.workbench.monitoring.monitor_last = Some(due);
    app.navigation.zoom = Some(navigation::Pane::Editor);
    app.workbench_tick(&ctx);
    assert!(!app.workbench.monitoring.monitor_busy);
    assert_eq!(app.workbench.monitoring.monitor_last, Some(due));
    app.navigation.zoom = None;
    app.toggle_panel(Panel::Monitor, &ctx);
    app.workbench_tick(&ctx);
    assert!(!app.workbench.monitoring.monitor_busy);
    assert_eq!(app.workbench.monitoring.monitor_last, Some(due));
}
#[test]
fn remote_monitor_waits_for_authentication_and_ignores_stale_host_results() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let remote = Remote::new(Profile {
        host: "monitor.invalid".into(),
        ..Default::default()
    })
    .unwrap();
    app.workspace.set_fixture(Some(Workspace {
        root: "/not-yet-open".into(),
        remote: Some(remote),
        connected: false,
        ready: false,
        plan: Default::default(),
        sandbox_options: Default::default(),
    }));
    app.workbench.panel = Some(Panel::Monitor);
    assert!(!app.monitor_ready());
    app.workbench_tick(&ctx);
    assert!(!app.workbench.monitoring.monitor_busy);
    assert!(app.workbench.monitoring.monitor.is_none());

    let labels = panel_text(&mut app, &ctx);
    assert!(labels.contains("Connect or reconnect"), "{labels}");
    assert!(labels.contains("monitor.invalid"), "{labels}");
    assert!(!labels.contains("Host overview"), "{labels}");

    app.workspace.as_mut().unwrap().connected = true;
    assert!(app.monitor_ready());
    assert!(!app.project_ready());
    app.workbench.monitoring.monitor_busy = true;
    app.workbench.monitoring.record(
        project::Monitor {
            hostname: "old-host".into(),
            ..Default::default()
        },
        Instant::now(),
        Duration::from_secs(2),
    );
    app.workbench.reset_monitor();
    let old_generation = app.workers.generation();
    app.workers.invalidate_workspace();
    app.workers
        .workspace_at(&ctx, old_generation)
        .send(WorkspaceEvent::Tool {
            job: Box::new(ToolJob::Monitor),
            result: Box::new(Ok(project::Response::Monitor(project::Monitor {
                hostname: "old-host".into(),
                ..Default::default()
            }))),
        })
        .unwrap();
    app.poll(&ctx);
    assert!(!app.workbench.monitoring.monitor_busy);
    assert!(app.workbench.monitoring.monitor.is_none());
    assert!(app.workbench.monitoring.history.is_empty());
    assert!(app.workbench.monitoring.monitor_last.is_none());
}
#[test]
fn live_checks_skip_unbound_buffers_even_with_a_plain_text_adapter() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, ctx) = app(root.path());
    let extensions = root.path().join("extensions");
    std::fs::create_dir(&extensions).unwrap();
    std::fs::write(
        extensions.join("text.json"),
        serde_json::json!({
            "api":1,"id":"test.text","name":"Text checker","version":"1.0","description":"Unbound buffer fixture",
            "tool":{"program":"false","languages":["text"],"check":true}
        })
        .to_string(),
    )
    .unwrap();
    app.extensions.registry = crate::extensions::Registry::load(&extensions, Default::default());
    app.extensions.registry.set_enabled("test.text", true);
    assert!(
        app.extensions
            .registry
            .provider("text", false, true)
            .is_some()
    );
    app.workbench.prefs.live_checks = true;
    app.new_file();
    app.workbench.code_tools.observed = Some((
        app.documents[0].id,
        0,
        Instant::now() - Duration::from_secs(1),
    ));
    app.workbench_tick(&ctx);
    assert!(app.workbench.code_tools.checking.is_empty());
    assert!(!app.documents[0].saving);
}
#[test]
fn format_save_result_cannot_replace_newer_edits_or_publish_stale_diagnostics() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("main.py");
    std::fs::write(&path, "value=0\n").unwrap();
    let (mut app, ctx) = app(root.path());
    app.new_file();
    let id = app.documents[0].id;
    let document = &mut app.documents[0];
    document.path = Some(path.to_str().unwrap().into());
    document.saved_text = "value=0\n".into();
    document.text = "value=2\n".into();
    document.set_dirty_fixture(true);
    document.set_version_fixture(2);
    document.saving = true;
    app.handle_tool(
        ToolJob::Analyze {
            tool: "test-tool".into(),
            id,
            version: 1,
            epoch: 0,
            original: "value=1\n".into(),
            path: path.to_str().unwrap().into(),
            save: Some(Some(files::digest(b"value=0\n"))),
            manual: false,
        },
        Ok(project::Response::Analysis(project::Analysis {
            text: "value = 1\n".into(),
            diagnostics: vec![Diagnostic {
                code: "stale".into(),
                ..Default::default()
            }],
            ..Default::default()
        })),
        &ctx,
    );
    let started = Instant::now();
    while app.documents[0].saving && started.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
        app.poll(&ctx);
    }
    assert!(!app.documents[0].saving);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "value=1\n");
    assert_eq!(app.documents[0].text, "value=2\n");
    assert!(app.documents[0].dirty());
    assert!(app.document_issues(&app.documents[0]).is_empty());
}
#[test]
fn fresh_formatter_result_updates_buffer_and_diagnostics_without_saving_it() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, ctx) = app(root.path());
    app.new_file();
    app.documents[0].text = "value=1\n".into();
    let id = app.documents[0].id;
    app.handle_tool(
        ToolJob::Analyze {
            tool: "test-tool".into(),
            id,
            version: 0,
            epoch: 0,
            original: "value=1\n".into(),
            path: "main.py".into(),
            save: None,
            manual: true,
        },
        Ok(project::Response::Analysis(project::Analysis {
            text: "value = 1\n".into(),
            diagnostics: vec![Diagnostic {
                code: "test".into(),
                ..Default::default()
            }],
            ..Default::default()
        })),
        &ctx,
    );
    assert_eq!(app.documents[0].text, "value = 1\n");
    assert_eq!(app.documents[0].version(), 1);
    assert!(app.documents[0].dirty());
    assert_eq!(app.document_issues(&app.documents[0]).len(), 1);
    assert!(!root.path().join("main.py").exists());
    app.documents[0].advance_version_fixture();
    assert!(app.document_issues(&app.documents[0]).is_empty());
}
#[test]
fn applying_a_local_project_policy_keeps_unsaved_buffers() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, ctx) = app(root.path());
    app.load_project_tools(&ctx);
    app.workbench.draft.latex.program = "unfinished-tool".into();
    app.workbench.settings_section = SettingsSection::Sandbox;
    app.new_file();
    app.documents[0].text = "keep unsaved work".into();
    let id = app.documents[0].id;
    app.commit_project(
        files::Response {
            path: root.path().to_str().unwrap().into(),
            project: Some(sandbox::Plan {
                root: root.path().to_str().unwrap().into(),
                ..Default::default()
            }),
            ..Default::default()
        },
        false,
        Default::default(),
        true,
        None,
        &ctx,
    );
    assert_eq!(app.documents[0].id, id);
    assert_eq!(app.documents[0].text, "keep unsaved work");
    assert!(app.documents[0].dirty());
    assert_eq!(app.workbench.draft.latex.program, "unfinished-tool");
    assert_ne!(app.workbench.prefs.latex.program, "unfinished-tool");
    assert_eq!(app.workbench.settings_section, SettingsSection::Sandbox);
    assert!(app.settings_draft_for("/another-project", false).is_none());
}
#[test]
fn missing_one_formatter_does_not_disable_other_languages() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, ctx) = app(root.path());
    app.new_file();
    let id = app.documents[0].id;
    let job = |path: &str, epoch| ToolJob::Analyze {
        tool: crate::languages::detect(Some(path))
            .unwrap()
            .check_tool()
            .unwrap()
            .into(),
        id,
        version: 0,
        epoch,
        original: String::new(),
        path: path.into(),
        save: None,
        manual: false,
    };
    app.handle_tool(
        job("main.py", app.workbench.epoch),
        Err("Ruff is unavailable".into()),
        &ctx,
    );
    assert!(app.workbench.code_tools.unavailable_tools.contains("ruff"));
    assert!(!app.workbench.code_tools.unavailable_tools.contains("gofmt"));
    app.handle_tool(
        job("main.go", app.workbench.epoch),
        Ok(project::Response::Analysis(Default::default())),
        &ctx,
    );
    assert!(app.workbench.code_tools.unavailable_tools.contains("ruff"));
    app.handle_tool(
        job("main.py", app.workbench.epoch),
        Ok(project::Response::Analysis(Default::default())),
        &ctx,
    );
    assert!(!app.workbench.code_tools.unavailable_tools.contains("ruff"));
}
#[derive(Default)]
struct Storage {
    data: HashMap<String, String>,
    flushes: usize,
}
impl eframe::Storage for Storage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.data.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.data.insert(key.into(), value);
    }
    fn flush(&mut self) {
        self.flushes += 1;
    }
}
#[test]
fn project_settings_flush_immediately_and_restore_per_project_values() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = app(root.path());
    app.workbench.key = "ssh:test:/project".into();
    app.workbench.prefs.python = "/project/.venv/bin/python".into();
    app.workbench.prefs.sandbox.project_read_only = true;
    app.workbench
        .prefs
        .sandbox
        .overrides
        .insert("~/.cargo".into(), sandbox::Access::Hidden);
    app.workbench.save_preferences();
    let mut storage = Storage::default();
    app.persist_project_preferences(Some(&mut storage));
    assert_eq!(storage.flushes, 1);
    app.persist_project_preferences(Some(&mut storage));
    assert_eq!(storage.flushes, 1);
    let restored = Workbench::load(Some(&storage));
    let prefs = &restored.preferences["ssh:test:/project"];
    assert_eq!(prefs.python, "/project/.venv/bin/python");
    assert!(prefs.sandbox.project_read_only);
    assert_eq!(prefs.sandbox.overrides["~/.cargo"], sandbox::Access::Hidden);
}

/// Drive the real settings form, including its category selector and scoped actions.
fn settings_frame(
    app: &mut EditorApp,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            time: Some(ctx.input(|input| input.time) + 0.1),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(760., 700.),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.settings_ui(ui, ctx));
        },
    )
}

#[test]
fn settings_text_undo_stays_with_its_category() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, ctx) = app(root.path());
    app.workbench.draft.ruff = "/original/ruff".into();
    app.workbench.draft.marp.theme_css = "slides.css".into();
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
    };
    click_settings(&mut app, &ctx, "/original/ruff");
    settings_frame(
        &mut app,
        &ctx,
        vec![key(egui::Key::A), egui::Event::Text("/changed/ruff".into())],
    );
    assert_eq!(app.workbench.draft.ruff, "/changed/ruff");
    click_settings(&mut app, &ctx, "Presentations •");
    click_settings(&mut app, &ctx, "slides.css");
    settings_frame(&mut app, &ctx, vec![key(egui::Key::Z)]);
    assert_eq!(
        app.workbench.draft.marp.theme_css, "slides.css",
        "Undo must not copy another category's field into this one"
    );
    click_settings(&mut app, &ctx, "Code tools •");
    click_settings(&mut app, &ctx, "/changed/ruff");
    settings_frame(&mut app, &ctx, vec![key(egui::Key::Z)]);
    assert_eq!(
        app.workbench.draft.ruff, "/original/ruff",
        "Returning to a category must retain its own undo history"
    );
}

fn click_settings(app: &mut EditorApp, ctx: &egui::Context, label: &str) {
    settings_frame(app, ctx, vec![]);
    let output = settings_frame(app, ctx, vec![]);
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.pos + text.galley.size() / 2.)
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "Missing settings control: {label}; visible text: {:?}",
                output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some(text.galley.text()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            )
        });
    for pressed in [true, false] {
        settings_frame(
            app,
            ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
        );
    }
}

#[test]
fn settings_categories_keep_drafts_and_save_only_validated_selected_fields() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, ctx) = app(root.path());
    app.workbench.key = "settings-fixture".into();
    let saved = serde_json::to_value(&app.workbench.prefs).unwrap();
    // Other forms may contain incomplete edits. Saving code settings must not
    // publish them or take an unrelated environment, debugger or tool setting.
    app.workbench.draft.latex.program.clear();
    app.workbench.draft.marp.program = "another-marp".into();
    app.workbench.draft.sandbox.enabled = !app.workbench.prefs.sandbox.enabled;
    app.workbench.draft.monitor_interval = 25;
    app.workbench.draft.python = "/unfinished/python".into();
    app.workbench.draft.slurm_queue = "another-queue".into();
    click_settings(&mut app, &ctx, "Format supported files on save");
    assert!(!app.workbench.draft.format_on_save);
    assert!(app.workbench.prefs.format_on_save);
    click_settings(&mut app, &ctx, "LaTeX •");
    assert_eq!(app.workbench.settings_section, SettingsSection::Latex);
    click_settings(&mut app, &ctx, "Save changes");
    assert_eq!(
        serde_json::to_value(&app.workbench.prefs).unwrap(),
        saved,
        "An invalid LaTeX draft must not be applied"
    );
    click_settings(&mut app, &ctx, "Code tools •");
    assert!(
        !app.workbench.draft.format_on_save,
        "Changing category lost an edit"
    );
    click_settings(&mut app, &ctx, "Save changes");
    let mut expected = saved;
    expected["format_on_save"] = false.into();
    assert_eq!(
        serde_json::to_value(&app.workbench.prefs).unwrap(),
        expected
    );
    assert_eq!(
        serde_json::to_value(&app.workbench.preferences["settings-fixture"]).unwrap(),
        expected
    );
    assert!(app.workbench.draft.latex.program.is_empty());
    assert_eq!(app.workbench.draft.monitor_interval, 25);
    click_settings(&mut app, &ctx, "Monitoring •");
    click_settings(&mut app, &ctx, "Discard changes");
    assert_eq!(
        app.workbench.draft.monitor_interval,
        app.workbench.prefs.monitor_interval
    );
    assert!(
        app.workbench.draft.latex.program.is_empty(),
        "Discard must be category-scoped too"
    );
    click_settings(&mut app, &ctx, "LaTeX •");
    app.workbench.draft.latex.program = "xelatex".into();
    click_settings(&mut app, &ctx, "Save changes");
    assert_eq!(app.workbench.prefs.latex.program, "xelatex");
    assert_eq!(app.workbench.draft.marp.program, "another-marp");
    assert_ne!(app.workbench.draft.sandbox, app.workbench.prefs.sandbox);
    assert!(
        !app.workspace.preparing(),
        "Saving tool settings must not start a sandbox probe"
    );
}

#[test]
fn settings_links_select_their_category_without_closing_the_tab_or_losing_edits() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, ctx) = app(root.path());
    app.workbench.surface = Surface::Tools;
    app.workbench.panel = Some(Panel::Settings);
    app.workbench.draft.ruff = "/unfinished/ruff".into();
    for category in [
        SettingsSection::Latex,
        SettingsSection::Presentations,
        SettingsSection::Presentations,
    ] {
        app.reveal_settings(category, &ctx);
        assert_eq!(app.workbench.settings_section, category);
        assert_eq!(app.workbench.panel, Some(Panel::Settings));
        assert_eq!(app.workbench.surface, Surface::Tools);
        assert_eq!(app.workbench.draft.ruff, "/unfinished/ruff");
    }
}

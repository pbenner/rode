//! Notebook editing, layout and kernel lifecycle regressions.

use super::*;

#[test]
fn next_cell_focus_survives_old_output_shrinking_before_it_is_rendered() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_notebook();
    let document = app.documents[0].id;
    let view = app.documents[0].notebook_mut().unwrap();
    let first = view.active;
    view.book
        .apply_output(
            first,
            0,
            Some(serde_json::json!({
                "output_type":"stream", "name":"stdout", "text":"Old output\n".repeat(1000)
            })),
            None,
            false,
        )
        .unwrap();
    view.book.cells.extend((0..20).map(|_| Cell::new("code")));
    let target = view.book.cells[1].uid;
    let frame = |app: &mut EditorApp| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 600.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.notebook_ui(ui, ctx));
            },
        )
    };
    frame(&mut app);
    frame(&mut app);
    let view = app.documents[0].notebook_mut().unwrap();
    assert!(view.cells[&first].height > 5000.);
    view.focus_next_cell();
    view.book.cells[0].clear_outputs();
    for _ in 0..4 {
        frame(&mut app);
    }
    let view = app.documents[0].notebook().unwrap();
    assert_eq!(view.active, target);
    assert_eq!(view.focus, None);
    assert!(ctx.memory(|memory| memory.has_focus(egui::Id::new((
        "notebook-source",
        document,
        target
    )))));
}

#[test]
fn rejected_source_edit_with_shift_enter_keeps_source_and_focus_in_place() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_notebook();
    let view = app.documents[0].notebook_mut().unwrap();
    view.book.cells[0].change_kind("raw");
    view.book.cells[0].set_source("keep this source".into());
    view.book.cells.push(Cell::new("code"));
    let first = view.active;
    let before = view.book.serialize().unwrap();
    let frame = |app: &mut EditorApp, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 600.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.notebook_ui(ui, ctx));
            },
        )
    };
    frame(&mut app, vec![]);
    frame(
        &mut app,
        vec![
            egui::Event::Paste("🦀".repeat(notebook::MAX_SOURCE / 4 + 1)),
            egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            },
        ],
    );
    let view = app.documents[0].notebook().unwrap();
    assert_eq!(view.active, first);
    assert_eq!(view.book.serialize().unwrap(), before);
    assert_eq!(view.error, "Cell source exceeds 256 KiB.");
    assert!(view.kernel.is_none());
}

#[test]
fn shift_enter_renders_and_focuses_the_next_cell_even_beyond_the_viewport() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_notebook();
    let view = app.documents[0].notebook_mut().unwrap();
    view.book.cells[0].change_kind("markdown");
    view.book.cells[0].set_source("# First section".into());
    let first = view.book.cells[0].uid;
    view.cells.entry(first).or_default().previewing = false;
    let mut raw = Cell::new("raw");
    raw.set_source("Raw line\n".repeat(100));
    let middle = raw.uid;
    let mut last = Cell::new("markdown");
    last.set_source("# Last section".into());
    let final_uid = last.uid;
    view.book.cells.extend([raw, last]);
    let before = view.book.serialize().unwrap();
    let frame = |app: &mut EditorApp, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 600.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.notebook_ui(ui, ctx));
            },
        )
    };
    let enter = |modifiers| {
        vec![egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }]
    };
    let document = app.documents[0].id;
    frame(&mut app, vec![]);
    frame(&mut app, vec![]);
    for expected in [middle, final_uid] {
        frame(&mut app, enter(egui::Modifiers::SHIFT));
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        let view = app.documents[0].notebook().unwrap();
        assert_eq!(view.active, expected);
        assert!(ctx.memory(|memory| memory.has_focus(egui::Id::new((
            "notebook-source",
            document,
            expected
        )))));
        assert!(!view.cells[&expected].previewing);
    }
    frame(&mut app, enter(egui::Modifiers::SHIFT));
    frame(&mut app, vec![]);
    let view = app.documents[0].notebook().unwrap();
    assert_eq!(view.active, final_uid);
    assert!(view.cells[&first].previewing && view.cells[&final_uid].previewing);
    assert!(
        view.kernel.is_none(),
        "Markdown/raw traversal must not start a kernel"
    );
    assert_eq!(view.book.serialize().unwrap(), before);
}

#[test]
fn output_paging_keeps_all_saved_records_and_reveals_the_rest_on_request() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_notebook();
    let view = app.documents[0].notebook_mut().unwrap();
    view.focus = None;
    let uid = view.book.cells[0].uid;
    for row in 0..80 {
        view.book
            .apply_output(
                uid,
                0,
                Some(serde_json::json!({
                    "output_type":"display_data", "data":{"text/plain":format!("Output row {row}")}
                })),
                None,
                false,
            )
            .unwrap();
    }
    let before = view.book.serialize().unwrap();
    let frame = |app: &mut EditorApp, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900., 5000.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.notebook_ui(ui, ctx));
            },
        )
    };
    frame(&mut app, vec![]);
    let output = frame(&mut app, vec![]);
    let more = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Show more outputs (16 remaining)" => {
                Some(text)
            }
            _ => None,
        })
        .expect("More outputs must be reachable");
    assert_eq!(
        app.documents[0].notebook().unwrap().cells[&uid].output_limit,
        64
    );
    let point = more.pos + more.galley.size() / 2.;
    for pressed in [true, false] {
        frame(
            &mut app,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    let output = frame(&mut app, vec![]);
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::Shape::Text(text) if text.galley.text() == "Output row 79"
    )));
    let view = app.documents[0].notebook().unwrap();
    assert_eq!(view.cells[&uid].output_limit, 128);
    assert_eq!(view.book.serialize().unwrap(), before);
}

#[test]
fn markdown_previews_are_compact_and_keep_html_source_through_editing() {
    for theme in ["atelier-dark", "atelier-light"] {
        for width in [650., 1280.] {
            for scale in [1., 1.25, 2.] {
                let ctx = egui::Context::default();
                let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                let mut app = EditorApp::new(
                    &cc,
                    Startup {
                        theme: Some(theme.into()),
                        ..Default::default()
                    },
                );
                ctx.set_pixels_per_point(scale);
                app.new_notebook();
                let view = app.documents[0].notebook_mut().unwrap();
                let source = "## A section\n\n<p>A short <strong>HTML paragraph</strong>.</p>";
                view.book.cells[0].change_kind("markdown");
                view.book.cells[0].set_source(source.into());
                view.focus = None;
                let uid = view.book.cells[0].uid;
                let mut next = Cell::new("code");
                next.set_source("answer = 42".into());
                view.book.cells.push(next);
                let original = view.book.serialize().unwrap();
                let frame = |app: &mut EditorApp, events| {
                    ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 800.),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                app.notebook_ui(ui, ctx);
                            });
                        },
                    )
                };
                frame(&mut app, vec![]);
                let output = frame(&mut app, vec![]);
                let view = app.documents[0].notebook().unwrap();
                assert!(
                    view.cells[&uid].height < 125.,
                    "{theme}/{width}/{scale}: {}",
                    view.cells[&uid].height
                );
                assert_eq!(view.book.cells[0].source, source);
                assert_eq!(view.book.serialize().unwrap(), original);
                let text: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some(text),
                        _ => None,
                    })
                    .collect();
                assert!(
                    text.iter()
                        .any(|text| text.galley.text() == "HTML paragraph")
                );
                assert!(
                    !text
                        .iter()
                        .any(|text| text.galley.text().contains("<strong>"))
                );
                let edit = text
                    .iter()
                    .find(|text| text.galley.text() == "Edit")
                    .unwrap();
                assert!(!text.iter().any(|text| text.galley.text() == "Cell…"));
                let edit_rect = egui::Rect::from_min_size(edit.pos, edit.galley.size());
                assert!(
                    output.shapes.iter().any(|shape| matches!(&shape.shape,
                        egui::Shape::Rect(rect) if rect.rect.contains_rect(edit_rect)
                            && rect.rect.width() < 100. && rect.rect.height() < 35.
                            && rect.fill != egui::Color32::TRANSPARENT
                    )),
                    "Edit must have a visible button background: {theme}/{width}/{scale}"
                );
                let point = edit.pos + edit.galley.size() / 2.;
                frame(
                    &mut app,
                    vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
                frame(
                    &mut app,
                    vec![egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
                let editing = frame(&mut app, vec![]);
                for label in ["markdown", "Preview", "Up", "Down", "Delete"] {
                    assert!(
                        editing.shapes.iter().any(|shape| matches!(&shape.shape,
                            egui::Shape::Text(text) if text.galley.text() == label
                        )),
                        "Missing editing control {label}: {theme}/{width}/{scale}"
                    );
                }
                let view = app.documents[0].notebook().unwrap();
                assert!(!view.cells[&uid].previewing);
                assert_eq!(view.book.cells[0].source, source);
                assert_eq!(view.book.serialize().unwrap(), original);
            }
        }
    }
}

#[test]
fn cell_sources_expand_to_fit_and_wheel_scrolls_the_notebook() {
    let multiline = (0..80)
        .map(|line| format!("# Source line {line}\n"))
        .collect::<String>();
    let wrapped = "A long source line should wrap inside the cell. ".repeat(100);
    for (kind, source) in [
        ("code", &multiline),
        ("markdown", &wrapped),
        ("raw", &multiline),
    ] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_notebook();
        let view = app.documents[0].notebook_mut().unwrap();
        let cell = &mut view.book.cells[0];
        cell.change_kind(kind);
        cell.set_source(source.clone());
        let uid = cell.uid;
        view.cells.entry(uid).or_default().previewing = false;
        view.focus = None;
        let mut next = Cell::new("code");
        next.set_source("next_cell = True".into());
        view.book.cells.push(next);

        let frame = |app: &mut EditorApp, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800., 700.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.notebook_ui(ui, ctx);
                    });
                },
            )
        };
        frame(&mut app, vec![]);
        let output = frame(&mut app, vec![]);
        let source_text = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == source => Some(text),
                _ => None,
            })
            .expect("cell source was rendered");
        assert!(source_text.galley.size().y > 320.);
        let height = app.documents[0].notebook().unwrap().cells[&uid].height;
        assert!(
            height >= source_text.galley.size().y,
            "{kind} cell clips its source: cell={height}, text={}",
            source_text.galley.size().y
        );
        let has_header = |output: &egui::FullOutput| {
            output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Cell 1"))
        };
        assert!(has_header(&output));
        let point = source_text.pos + egui::vec2(20., 40.);
        frame(
            &mut app,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., -400.),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let mut output = frame(&mut app, vec![]);
        for _ in 0..12 {
            output = frame(&mut app, vec![]);
        }
        assert!(
            !has_header(&output),
            "Scrolling over {kind} source left the cell header fixed"
        );
        assert_eq!(
            app.documents[0].notebook().unwrap().book.cells[0].source,
            *source
        );

        let view = app.documents[0].notebook_mut().unwrap();
        view.book.cells[0].set_source("short source".into());
        view.reveal = true;
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        assert!(
            app.documents[0].notebook().unwrap().cells[&uid].height < 320.,
            "Shortened {kind} cell did not shrink"
        );
    }
}

#[test]
fn python_notebooks_default_to_the_environment_without_rewriting_saved_metadata() {
    for (language, saved, expected) in [
        ("python", "old-venv", "python3"),
        ("julia", "julia-1.12", "julia-1.12"),
    ] {
        let mut book = Notebook::default();
        book.set_kernel(saved).unwrap();
        book.data["metadata"]["language_info"] = serde_json::json!({"name": language});
        let original = book.serialize().unwrap();
        let view = View::new(
            book,
            Arc::new(markdown::Images::default()),
            Shutdowns::default(),
        );
        assert_eq!(view.kernel_name, expected);
        assert_eq!(view.book.serialize().unwrap(), original);
        assert!(view.kernel.is_none());
    }
}
#[test]
fn a_disconnected_notebook_cannot_start_a_local_kernel_or_read_external_images() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_notebook();
    app.documents[0].fixture_content().path = Some("/remote/work.ipynb".into());
    let first = app.documents[0].notebook().unwrap().active;
    app.documents[0]
        .notebook_mut()
        .unwrap()
        .book
        .cells
        .push(Cell::new("code"));
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    app.detach_ssh(documents::Retention::Keep, &ctx);
    app.execute_command(Command::NotebookRun, &ctx);
    assert!(app.documents[0].notebook().unwrap().kernel.is_none());
    assert_eq!(
        app.documents[0].notebook().unwrap().active,
        first,
        "Rejected execution must not advance"
    );
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .0
            .contains("disconnected notebook")
    );
    let view = app.documents[0].notebook().unwrap();
    app.notebook_picture_jobs(
        view,
        vec![PictureJob {
            target: Target::Markdown {
                uid: 1,
                token: 1,
                uri: "outside.png".into(),
            },
            encoded: None,
            path: "/remote/outside.png".into(),
            cancel: Arc::new(AtomicBool::new(false)),
        }],
        &ctx,
    );
    let result = view.rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        result
            .result
            .err()
            .unwrap()
            .contains("Reconnect or Save As")
    );
}

#[test]
fn clearing_outputs_forgets_display_targets_only_for_that_cell() {
    let mut view = View::new(
        Notebook::default(),
        Arc::new(markdown::Images::default()),
        Shutdowns::default(),
    );
    let uid = view.book.cells[0].uid;
    view.display_ids.insert("clear".into(), vec![(uid, 0, 0)]);
    view.display_ids
        .insert("other".into(), vec![(uid + 100, 0, 0)]);
    view.drop_displays(uid);
    assert!(!view.display_ids.contains_key("clear"));
    assert!(view.display_ids.contains_key("other"));
}
#[test]
fn kernel_startup_failure_preserves_saved_outputs_and_a_clean_buffer() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join(".venv/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let python = bin.join("python");
    std::fs::write(&python,"#!/bin/sh\nprintf '%s\\n' '{\"kind\":\"error\",\"message\":\"missing optional packages\"}'\n").unwrap();
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_notebook();
    let book = Notebook::parse(include_str!("../../../tests/fixtures/notebook.ipynb")).unwrap();
    let text = book.serialize().unwrap();
    app.documents[0].fixture_content().text = text.clone();
    app.documents[0].fixture_content().saved_text = text.clone();
    app.documents[0].set_dirty_fixture(false);
    app.documents[0].replace_notebook(View::new(
        book,
        app.markdown_images.clone(),
        app.notebook_shutdowns.clone(),
    ));
    app.workspace.set_fixture(Some(Workspace {
        root: root.path().to_string_lossy().into(),
        remote: None,
        ready: true,
        connected: true,
        plan: sandbox::prepare_with_helper(
            root.path().to_str().unwrap(),
            &Default::default(),
            &std::env::var_os("RODE_TEST_SESSION_HELPER")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_exe()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join("rode-sessiond")
                }),
        )
        .unwrap(),
        sandbox_options: Default::default(),
    }));
    app.run_notebook(true, &ctx);
    let start = Instant::now();
    while app.documents[0].notebook().unwrap().error.is_empty() {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
        app.poll_notebooks(&ctx);
    }
    assert_eq!(app.documents[0].text, text);
    assert!(!app.documents[0].dirty());
    assert_eq!(
        app.documents[0].notebook().unwrap().book.cells[2].outputs()[0]["text"],
        json!(["42\n"])
    );
}
#[test]
fn missing_slurm_target_preserves_outputs_and_never_starts_a_host_kernel() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let queue = root.path().join("queue");
    std::fs::write(&queue, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&queue, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_notebook();
    let book = Notebook::parse(include_str!("../../../tests/fixtures/notebook.ipynb")).unwrap();
    let text = book.serialize().unwrap();
    app.documents[0].fixture_content().text = text.clone();
    app.documents[0].fixture_content().saved_text = text.clone();
    let mut view = View::new(
        book,
        app.markdown_images.clone(),
        app.notebook_shutdowns.clone(),
    );
    app.documents[0].set_dirty_fixture(false);
    let target = crate::slurm::Target {
        id: "4242".into(),
        name: "expired".into(),
        queue: queue.to_string_lossy().into(),
        runner: "/bin/false".into(),
    };
    view.set_execution_target(Some(target.clone()));
    app.documents[0].replace_notebook(view);
    app.workspace.set_fixture(Some(Workspace {
        root: root.path().to_string_lossy().into(),
        remote: None,
        ready: true,
        connected: true,
        plan: sandbox::prepare_with_helper(
            root.path().to_str().unwrap(),
            &Default::default(),
            &std::env::var_os("RODE_TEST_SESSION_HELPER")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_exe()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join("rode-sessiond")
                }),
        )
        .unwrap(),
        sandbox_options: Default::default(),
    }));
    app.run_notebook(true, &ctx);
    let start = Instant::now();
    while app.documents[0].notebook().unwrap().error.is_empty() {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
        app.poll_notebooks(&ctx);
    }
    let view = app.documents[0].notebook_mut().unwrap();
    assert!(view.kernel.is_none());
    assert_eq!(view.execution_target, Some(target));
    assert!(
        view.error.contains("no longer your running job"),
        "{}",
        view.error
    );
    view.set_execution_target(None);
    assert!(view.error.is_empty());
    assert!(!view.uses_slurm());
    assert_eq!(app.documents[0].text, text);
    assert!(!app.documents[0].dirty());
}

#[test]
fn enter_in_notebook_find_reveals_and_selects_a_matching_cell() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_notebook();
    let book = Notebook::parse(include_str!("../../../tests/fixtures/notebook.ipynb")).unwrap();
    let expected = book.cells[1].uid;
    app.documents[0].fixture_content().text = book.serialize().unwrap();
    app.documents[0].replace_notebook(View::new(
        book,
        app.markdown_images.clone(),
        app.notebook_shutdowns.clone(),
    ));
    app.editor.show_find = true;
    app.editor.find = "counter = 40".into();
    app.editor.find_focus = true;
    let frame = |app: &mut EditorApp, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200., 800.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                app.editor(ctx);
            },
        )
    };
    frame(&mut app, vec![]);
    frame(
        &mut app,
        vec![egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
    );
    assert_eq!(app.documents[0].notebook().unwrap().active, expected);
}

#[test]
fn unchanged_notebook_reload_keeps_view_state_and_source_identity() {
    let mut docs = documents::Documents::default();
    let images = Arc::new(markdown::Images::default());
    let shutdowns = notebook_view::Shutdowns::default();
    let book = crate::notebook::Notebook::default();
    let response = Response {
        path: "book.ipynb".into(),
        text: book.serialize().unwrap(),
        revision: "r1".into(),
        notebook: Some(Ok(book)),
        ..Default::default()
    };
    docs.open(response.clone(), &images, &shutdowns);
    let doc = docs.active_mut().unwrap();
    doc.notebook_mut().unwrap().error = "retained view state".into();
    doc.reload(response, &images, &shutdowns);
    assert_eq!(doc.notebook().unwrap().error, "retained view state");
    assert_eq!(doc.version(), 0);
}

#[test]
fn toolbar_is_compact_and_expanded_settings_leave_a_separated_cell_viewport() {
    for theme in ["atelier-dark", "atelier-light"] {
        for width in [320., 650., 1100.] {
            for scale in [1., 1.5, 2.] {
                let ctx = egui::Context::default();
                let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                let mut app = EditorApp::new(
                    &cc,
                    Startup {
                        theme: Some(theme.into()),
                        ..Default::default()
                    },
                );
                ctx.set_pixels_per_point(scale);
                app.new_notebook();
                let mut view = app.documents[0].take_notebook().unwrap();
                let mut closed_height = 0.;
                for expanded in [false, true] {
                    view.settings_open = expanded;
                    for _ in 0..3 {
                        let mut body = egui::Rect::NOTHING;
                        let mut top = 0.;
                        let output = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, 560.),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default().show(ctx, |ui| {
                                    top = ui.cursor().top();
                                    app.notebook_toolbar(ui, ctx, 0, &mut view);
                                    body = ui.available_rect_before_wrap();
                                });
                            },
                        );
                        assert!(
                            body.height() >= 100.,
                            "{theme}/{width}/{scale}, expanded={expanded}: {body:?}"
                        );
                        if !expanded {
                            closed_height = body.top() - top;
                            assert!(
                                closed_height < if width >= 1100. { 65. } else { 155. },
                                "Header too tall: {theme}/{width}/{scale}: {closed_height}"
                            );
                        } else {
                            assert!(body.top() - top <= closed_height + 260.);
                        }
                        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                            egui::Shape::Rect(rect) if rect.fill == app.themes.active().palette.panel
                                && rect.rect.width() >= width - 20. && (rect.rect.top() - top).abs() < 1.
                        )), "Toolbar surface must span the cell viewport");
                        assert!(
                            output.shapes.iter().any(|shape| matches!(&shape.shape,
                                egui::Shape::LineSegment { points, stroke } if stroke.width == 1.5
                                    && points[0].y < body.top() && points[0].y > top
                                    && (points[1].x - points[0].x) >= width - 20.
                            )),
                            "Missing full-width toolbar boundary: {theme}/{width}/{scale}"
                        );
                    }
                }
                assert!(view.kernel.is_none());
            }
        }
    }
}

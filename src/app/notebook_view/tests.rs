//! Notebook editing, layout and kernel lifecycle regressions.

use super::*;

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
        let view = app.documents[0].notebook.as_mut().unwrap();
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
        let height = app.documents[0].notebook.as_ref().unwrap().cells[&uid].height;
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
            app.documents[0].notebook.as_ref().unwrap().book.cells[0].source,
            *source
        );

        let view = app.documents[0].notebook.as_mut().unwrap();
        view.book.cells[0].set_source("short source".into());
        view.reveal = true;
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        assert!(
            app.documents[0].notebook.as_ref().unwrap().cells[&uid].height < 320.,
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
    app.documents[0].path = Some("/remote/work.ipynb".into());
    app.workspace = Some(crate::app::tests::remote_workspace());
    app.detach_ssh(&ctx);
    app.run_notebook(false, &ctx);
    assert!(app.documents[0].notebook.as_ref().unwrap().kernel.is_none());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .0
            .contains("disconnected notebook")
    );
    let view = app.documents[0].notebook.as_ref().unwrap();
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
    app.documents[0].text = text.clone();
    app.documents[0].saved_text = text.clone();
    app.documents[0].modified = false;
    app.documents[0].notebook = Some(View::new(
        book,
        app.markdown_images.clone(),
        app.notebook_shutdowns.clone(),
    ));
    app.workspace = Some(Workspace {
        root: root.path().to_string_lossy().into(),
        remote: None,
        ready: true,
        connected: true,
        plan: sandbox::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap(),
        sandbox_options: Default::default(),
    });
    app.run_notebook(true, &ctx);
    let start = Instant::now();
    while app.documents[0].notebook.as_ref().unwrap().error.is_empty() {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
        app.poll_notebooks(&ctx);
    }
    assert_eq!(app.documents[0].text, text);
    assert!(!app.documents[0].dirty());
    assert_eq!(
        app.documents[0].notebook.as_ref().unwrap().book.cells[2].outputs()[0]["text"],
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
    app.documents[0].text = text.clone();
    app.documents[0].saved_text = text.clone();
    let mut view = View::new(
        book,
        app.markdown_images.clone(),
        app.notebook_shutdowns.clone(),
    );
    app.documents[0].modified = false;
    let target = crate::slurm::Target {
        id: "4242".into(),
        name: "expired".into(),
        queue: queue.to_string_lossy().into(),
        runner: "/bin/false".into(),
    };
    view.set_execution_target(Some(target.clone()));
    app.documents[0].notebook = Some(view);
    app.workspace = Some(Workspace {
        root: root.path().to_string_lossy().into(),
        remote: None,
        ready: true,
        connected: true,
        plan: sandbox::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap(),
        sandbox_options: Default::default(),
    });
    app.run_notebook(true, &ctx);
    let start = Instant::now();
    while app.documents[0].notebook.as_ref().unwrap().error.is_empty() {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
        app.poll_notebooks(&ctx);
    }
    let view = app.documents[0].notebook.as_mut().unwrap();
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
    app.documents[0].text = book.serialize().unwrap();
    app.documents[0].notebook = Some(View::new(
        book,
        app.markdown_images.clone(),
        app.notebook_shutdowns.clone(),
    ));
    app.show_find = true;
    app.find = "counter = 40".into();
    app.find_focus = true;
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
    assert_eq!(app.documents[0].notebook.as_ref().unwrap().active, expected);
}

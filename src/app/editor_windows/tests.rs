use super::*;

fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        modifiers,
        physical_key: None,
        pressed: true,
        repeat: false,
    }
}

fn child_frame(
    app: &mut EditorApp,
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    events: Vec<egui::Event>,
    close: bool,
) -> egui::FullOutput {
    let mut input = egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000., 700.),
        )),
        events,
        ..Default::default()
    };
    let info = input.viewports.entry(viewport).or_default();
    info.parent = Some(egui::ViewportId::ROOT);
    info.focused = Some(true);
    if close {
        info.events.push(egui::ViewportEvent::Close);
    }
    ctx.run(input, |ctx| app.editor_window_ui(ctx))
}

#[test]
fn returning_a_native_editor_preserves_same_frame_and_queued_source_edits() {
    for handoff in [false, true] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let viewport = app.allocate_editor_window();
        app.in_editor(viewport, |app| {
            for path in ["first.txt", "second.txt"] {
                app.new_file();
                app.documents.active_mut().unwrap().acknowledge_save(
                    path.into(),
                    "".into(),
                    "revision".into(),
                );
            }
            app.focus_editor(&ctx);
        });
        child_frame(&mut app, &ctx, viewport, vec![], false);
        child_frame(&mut app, &ctx, viewport, vec![], false);
        let mut events = vec![egui::Event::Text("before".into())];
        if handoff {
            events.extend([
                key(Key::Tab, egui::Modifiers::CTRL),
                egui::Event::Text("after".into()),
            ]);
        }
        let output = child_frame(&mut app, &ctx, viewport, events, true);
        assert!(
            output.viewport_output[&viewport]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
        );
        if handoff {
            assert!(
                app.editors.contains(viewport),
                "Return must wait for replay"
            );
        }
        for _ in 0..8 {
            if !app.editors.contains(viewport) {
                break;
            }
            child_frame(&mut app, &ctx, viewport, vec![], false);
        }
        assert!(!app.editors.contains(viewport));
        assert_eq!(app.documents[1].text, "before");
        assert_eq!(app.documents[0].text, if handoff { "after" } else { "" });
        assert!(app.documents[1].dirty());
        for document in app.documents.iter() {
            assert_eq!(
                app.documents.owner(document.id),
                Some(egui::ViewportId::ROOT)
            );
        }
        assert!(!app.editor_input_pending(&ctx));
        assert!(app.ssh_master.is_none());
        assert!(app.terminal.is_none());
    }
}

#[test]
fn moving_tabs_keeps_independent_selection_and_returns_pending_reads_to_root() {
    let mut documents = documents::Documents::default();
    let first = documents.new_file();
    let second = documents.new_file();
    let child = egui::ViewportId::from_hash_of("child");
    assert!(documents.move_to(second, child));
    assert_eq!(documents.selected_id(), Some(first));
    documents.set_viewport(child);
    assert_eq!(documents.selected_id(), Some(second));
    let third = documents.new_file();
    documents.cycle(1);
    assert_eq!(documents.selected_id(), Some(second));
    assert!(documents.begin_open("pending.rs".into()));
    assert!(!documents.begin_open("pending.rs".into()));
    documents.reattach(child);
    documents.set_viewport(egui::ViewportId::ROOT);
    assert_eq!(documents.len(), 3);
    assert_eq!(documents.selected_id(), Some(second));
    assert_eq!(
        documents.finish_open("pending.rs"),
        Some(egui::ViewportId::ROOT)
    );
    documents.close(second).unwrap();
    assert_eq!(documents.selected_id(), Some(third));
    documents.cycle(1);
    assert_eq!(documents.selected_id(), Some(first));
}

#[test]
fn prepared_project_waits_for_same_frame_source_input_in_another_editor() {
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let child_events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let events = child_events.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let mut input = egui::RawInput {
            viewport_id: viewport.ids.this,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 700.),
            )),
            events: std::mem::take(&mut *events.borrow_mut()),
            ..Default::default()
        };
        let info = input.viewports.entry(viewport.ids.this).or_default();
        info.parent = Some(viewport.ids.parent);
        info.focused = Some(true);
        let _ = ctx.run(input, |ctx| (viewport.viewport_ui_cb)(ctx));
    });
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let viewport = app.allocate_editor_window();
    app.in_editor(viewport, |app| {
        app.new_file();
        app.documents.active_mut().unwrap().acknowledge_save(
            "source.txt".into(),
            "".into(),
            "revision".into(),
        );
        app.focus_editor(&ctx);
    });
    let mut desktop = eframe::Frame::_new_kittest();
    let frame = |app: &mut EditorApp, desktop: &mut eframe::Frame| {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 700.),
                )),
                focused: false,
                ..Default::default()
            },
            |ctx| eframe::App::update(app, ctx, desktop),
        );
    };
    frame(&mut app, &mut desktop);
    frame(&mut app, &mut desktop);
    let project = tempfile::tempdir().unwrap();
    let token = app.workspace.begin_preparation();
    let response = files::local(&Request::Prepare {
        path: project.path().to_string_lossy().into(),
        sandbox: Default::default(),
    })
    .unwrap();
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Prepare {
                token,
                remote: false,
                options: Default::default(),
                keep_buffers: false,
                documents_at_start: app.documents.versions(),
                restore_saved: None,
                open_file: None,
            },
            result: Box::new(Ok(response)),
        })
        .unwrap();
    child_events
        .borrow_mut()
        .push(egui::Event::Text("typed with the reply".into()));
    frame(&mut app, &mut desktop);
    assert!(
        app.workspace.is_none(),
        "Project changed before the child source could publish its edit"
    );
    assert_eq!(app.documents[0].text, "typed with the reply");
    assert!(app.documents[0].dirty());
    assert!(matches!(app.pending.as_ref(), Some(Action::PreparedProject { id }) if *id == token));
}

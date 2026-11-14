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
fn frame(app: &mut EditorApp, ctx: &egui::Context, events: Vec<egui::Event>) {
    let _ = input_and_close(app, ctx, events, false);
}

/// Exercise input, native TextEdit and exit admission in the production frame order.
fn input_and_close(
    app: &mut EditorApp,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    close: bool,
) -> egui::FullOutput {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000., 700.),
        )),
        events,
        ..Default::default()
    };
    if close {
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
    }
    let mut frame = eframe::Frame::_new_kittest();
    ctx.run(input, |ctx| eframe::App::update(app, ctx, &mut frame))
}

#[test]
fn same_frame_source_input_reaches_the_dirty_guard_before_native_close() {
    for style in [Style::Standard, Style::Emacs, Style::Vim] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].acknowledge_save("source.txt".into(), "".into(), "rev".into());
        app.navigation.bindings.select_style(style);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let text = if style == Style::Vim {
            "iaccepted edit"
        } else {
            "accepted edit"
        };
        let output = input_and_close(&mut app, &ctx, vec![egui::Event::Text(text.into())], true);
        assert_eq!(app.documents[0].text, "accepted edit", "{style:?}");
        assert!(app.documents[0].dirty());
        assert!(matches!(app.pending.as_ref(), Some(Action::Quit)));
        assert!(!app.exit.closing());
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::CancelClose))
        );
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Close))
        );
    }
}

#[test]
fn native_close_preserves_edits_on_both_sides_of_a_queued_document_handoff() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    for path in ["first.txt", "second.txt"] {
        app.new_file();
        app.documents
            .active_mut()
            .unwrap()
            .acknowledge_save(path.into(), "".into(), "rev".into());
    }
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    input_and_close(
        &mut app,
        &ctx,
        vec![
            egui::Event::Text("before".into()),
            key(Key::Tab, egui::Modifiers::CTRL),
            egui::Event::Text("after".into()),
        ],
        true,
    );
    assert!(app.input.pending());
    assert!(
        !app.pending.modal(),
        "A quit modal must not prevent source replay"
    );
    for _ in 0..8 {
        frame(&mut app, &ctx, vec![]);
        if !app.input.pending() {
            break;
        }
    }
    assert_eq!(app.documents[0].text, "after");
    assert_eq!(app.documents[1].text, "before");
    assert!(!app.input.pending());
    assert!(
        matches!(app.pending.as_ref(), Some(Action::Quit)),
        "The single native close intent was lost"
    );
    assert!(!app.exit.closing());
}

#[test]
fn native_close_waits_for_queued_saves_and_keeps_edits_newer_than_the_saved_snapshot() {
    for trailing_edit in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.txt");
        std::fs::write(&path, "").unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].acknowledge_save(
            path.to_string_lossy().into(),
            "".into(),
            files::digest(b""),
        );
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let mut events = vec![
            egui::Event::Text("submitted ".into()),
            key(Key::S, egui::Modifiers::COMMAND),
        ];
        if trailing_edit {
            events.push(egui::Event::Text("newer".into()));
        }
        input_and_close(&mut app, &ctx, events, true);
        assert!(app.input.pending());
        assert!(!app.pending.modal());
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.input.pending()
            || app.documents[0].saving
            || (!trailing_edit && !app.exit.closing())
        {
            frame(&mut app, &ctx, vec![]);
            assert!(
                Instant::now() < deadline,
                "Save/close stalled: {:?}",
                app.notice.as_ref().map(|n| &n.0)
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "submitted ");
        assert_eq!(app.exit.closing(), !trailing_edit);
        assert_eq!(app.pending.is_some(), trailing_edit);
        if trailing_edit {
            assert_eq!(app.documents[0].text, "submitted newer");
        }
    }
}

#[test]
fn save_after_batched_source_edits_uses_the_text_applied_by_the_native_editor() {
    for style in [navigation::Style::Standard, navigation::Style::Emacs] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.txt");
        std::fs::write(&path, "original").unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].fixture_content().text = "original".into();
        app.documents[0].acknowledge_save(
            path.to_string_lossy().into(),
            "original".into(),
            files::digest(b"original"),
        );
        app.navigation.bindings.select_style(style);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let mut events = vec![egui::Event::Text("changed ".into())];
        if style == navigation::Style::Emacs {
            events.push(key(Key::X, egui::Modifiers::CTRL));
        }
        events.push(key(
            Key::S,
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        ));
        frame(&mut app, &ctx, events);
        let expected = app.documents[0].text.clone();
        assert!(
            expected.contains("changed "),
            "Native edit was not applied: {style:?}"
        );
        let until = Instant::now() + Duration::from_secs(5);
        while app.input.pending() || app.documents[0].dirty() || app.documents[0].saving {
            frame(&mut app, &ctx, vec![]);
            assert!(
                Instant::now() < until,
                "Save did not finish: {style:?}, {:?}",
                app.notice.as_ref().map(|n| &n.0)
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            expected,
            "{style:?}"
        );
    }
}

#[test]
fn a_style_change_cancels_an_incomplete_prefix_instead_of_consuming_new_source_text() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.navigation.bindings.select_style(Style::Emacs);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![key(Key::X, egui::Modifiers::CTRL)]);
    app.navigation.bindings.select_style(Style::Standard);
    frame(
        &mut app,
        &ctx,
        vec![
            key(Key::S, egui::Modifiers::NONE),
            egui::Event::Text("s".into()),
        ],
    );
    assert!(app.dialog == Dialog::None);
    assert_eq!(app.documents[0].text, "s");
}

#[test]
fn a_native_clipboard_shortcut_waits_for_earlier_source_edits_even_after_modifier_release() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let binding = Command::Extensions.shortcut().unwrap();
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::Text("keep this edit".into()),
            egui::Event::Cut,
            egui::Event::Key {
                key: binding.logical_key,
                physical_key: None,
                modifiers: binding.modifiers,
                pressed: false,
                repeat: false,
            },
        ],
    );
    assert!(app.dialog == Dialog::None);
    assert_eq!(app.documents[0].text, "keep this edit");
    assert!(app.input.pending());
    frame(&mut app, &ctx, vec![]);
    assert!(app.dialog == Dialog::Extensions);
    assert_eq!(app.documents[0].text, "keep this edit");
    assert!(!app.input.pending());
}

#[test]
fn a_tab_command_preserves_following_input_and_uses_the_destination_keymap() {
    for style in [Style::Standard, Style::Emacs, Style::Vim] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].fixture_content().text = "first".into();
        app.new_file();
        app.documents[1].fixture_content().text = "second".into();
        app.navigation.bindings.select_style(style);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        frame(
            &mut app,
            &ctx,
            vec![
                key(Key::Tab, egui::Modifiers::CTRL),
                egui::Event::Text(
                    if style == Style::Vim {
                        "ityped "
                    } else {
                        "typed "
                    }
                    .into(),
                ),
            ],
        );
        for _ in 0..8 {
            frame(&mut app, &ctx, vec![]);
            if !app.input.pending() {
                break;
            }
        }
        assert_eq!(app.documents.active_index(), 0);
        assert_eq!(app.documents[0].text, "typed first", "{style:?}");
        assert_eq!(app.documents[1].text, "second", "{style:?}");
        assert!(!app.input.pending());
    }
}

#[test]
fn new_and_clean_close_commands_preserve_following_source_input() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].fixture_content().text = "original".into();
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    frame(
        &mut app,
        &ctx,
        vec![
            key(Key::N, egui::Modifiers::COMMAND),
            egui::Event::Text("new buffer".into()),
        ],
    );
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.documents.len(), 2);
    assert_eq!(app.documents[1].text, "new buffer");
    assert_eq!(app.documents[0].text, "original");
    app.documents[1].acknowledge_save("clean.txt".into(), "new buffer".into(), "rev".into());
    frame(
        &mut app,
        &ctx,
        vec![
            key(Key::W, egui::Modifiers::COMMAND),
            egui::Event::Text("returned ".into()),
        ],
    );
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.documents.len(), 1);
    assert_eq!(app.documents[0].text, "returned original");
    assert!(!app.input.pending());
}

#[test]
fn source_edits_on_each_side_of_a_tab_switch_stay_with_their_document() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].fixture_content().text = "first".into();
    app.new_file();
    app.documents[1].fixture_content().text = "second".into();
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::Text("before ".into()),
            key(Key::Tab, egui::Modifiers::CTRL),
            egui::Event::Text("after ".into()),
        ],
    );
    for _ in 0..8 {
        frame(&mut app, &ctx, vec![]);
        if !app.input.pending() {
            break;
        }
    }
    assert_eq!(app.documents.active_index(), 0);
    assert_eq!(app.documents[0].text, "after first");
    assert_eq!(app.documents[1].text, "before second");
    assert!(!app.input.pending());
}

#[test]
fn replay_requires_native_application_and_rejects_a_changed_context() {
    let ctx = egui::Context::default();
    let scope = Scope::new(&ctx, 1, navigation::Style::Standard);
    let mut queue = Replay::default();
    queue.bind(scope);
    queue.defer(key(Key::S, egui::Modifiers::COMMAND));
    let _ = ctx.run(Default::default(), |ctx| {
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(("document", 1u64))));
        queue.replay(ctx, Some(scope));
        assert!(!ctx.input(|i| i.key_pressed(Key::S)));
        queue.acknowledge(ctx, 2); // Another widget cannot acknowledge our edit.
        queue.replay(ctx, Some(scope));
        assert!(!ctx.input(|i| i.key_pressed(Key::S)));
        queue.acknowledge(ctx, 1);
        queue.replay(ctx, Some(scope));
        assert!(ctx.input(|i| i.key_pressed(Key::S)));
    });
    for changed in [
        None,
        Some(Scope::new(&ctx, 2, scope.style)),
        Some(Scope::new(&ctx, 1, navigation::Style::Vim)),
    ] {
        queue.bind(scope);
        queue.defer(egui::Event::Text("must stay in old context".into()));
        queue.acknowledge(&ctx, 1);
        let _ = ctx.run(Default::default(), |ctx| {
            queue.replay(ctx, changed);
            assert!(!queue.pending());
            assert!(!ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_)))));
        });
    }
}

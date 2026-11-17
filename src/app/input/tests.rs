use super::*;

#[test]
fn discovery_handoff_keeps_batched_source_edit_before_save() {
    for already_open in [true, false] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        let render = |app: &mut EditorApp, events| {
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1380., 900.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    app.route_input(ctx);
                    app.editor(ctx);
                    app.navigation_finish(ctx);
                },
            );
        };
        render(&mut app, vec![]);
        app.start_keyboard_controls(false, &ctx);
        let code = keyboard::snapshot(&ctx)["hints"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["label"] == "Source editor")
            .unwrap()["code"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut events = vec![];
        if !already_open {
            render(&mut app, vec![key(Key::Escape, egui::Modifiers::NONE)]);
            render(&mut app, vec![]);
            events.extend([
                key(Key::Space, egui::Modifiers::CTRL),
                key(Key::A, egui::Modifiers::NONE),
                egui::Event::Text("a".into()),
            ]);
        }
        for c in code.chars() {
            events.push(key(
                Key::from_name(&c.to_ascii_uppercase().to_string()).unwrap(),
                egui::Modifiers::NONE,
            ));
            events.push(egui::Event::Text(c.to_string()));
        }
        events.extend([
            egui::Event::Text("edit".into()),
            key(Key::S, egui::Modifiers::COMMAND),
        ]);
        render(&mut app, events);
        for _ in 0..8 {
            if app.documents.active().unwrap().text == "edit" {
                break;
            }
            render(&mut app, vec![]);
        }
        assert_eq!(app.documents.active().unwrap().text, "edit");
        assert!(
            app.dialog == Dialog::None,
            "Save must wait for the native edit acknowledgement"
        );
        render(&mut app, vec![]);
        assert!(matches!(app.dialog, Dialog::Save(_)));
    }
}

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

#[test]
fn word_wrap_consumes_its_alt_text_without_losing_edits_before_or_after_it() {
    // Some backends emit the Alt chord's printable text; others suppress it.
    // In either case, subsequent text in the packet still belongs to the editor.
    for emits_alt_text in [false, true] {
        for style in [Style::Standard, Style::Emacs, Style::Vim] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.new_file();
            app.navigation.bindings.select_style(style);
            frame(&mut app, &ctx, vec![]);
            frame(&mut app, &ctx, vec![]);
            if style == Style::Vim {
                frame(&mut app, &ctx, vec![egui::Event::Text("i".into())]);
            }
            let mut events = vec![
                egui::Event::Text("before ".into()),
                key(Key::Z, egui::Modifiers::ALT),
            ];
            if emits_alt_text {
                events.push(egui::Event::Text("z".into()));
            }
            events.push(egui::Event::Text("after".into()));
            frame(&mut app, &ctx, events);
            for _ in 0..8 {
                frame(&mut app, &ctx, vec![]);
            }
            assert!(app.word_wrap, "{style:?}");
            assert_eq!(app.documents[0].text, "before after", "{style:?}");
        }
    }
}

#[test]
fn editor_focus_requested_by_a_closing_control_survives_its_pointer_release() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let id = app.documents[0].id;
    let widget = egui::Id::new(("document", id));
    let pointer = |pressed| egui::Event::PointerButton {
        pos: egui::pos2(900., 500.),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut app, &ctx, vec![pointer(true)]);
    app.focus_editor(&ctx);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 700.),
            )),
            events: vec![pointer(false)],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert_eq!(app.take_editor_focus(ui, ctx), None);
            });
        },
    );
    frame(&mut app, &ctx, vec![]);
    assert!(ctx.memory(|memory| memory.has_focus(widget)));
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
    assert!(app.editor.input.pending());
    assert!(
        !app.pending.modal(),
        "A quit modal must not prevent source replay"
    );
    for _ in 0..8 {
        frame(&mut app, &ctx, vec![]);
        if !app.editor.input.pending() {
            break;
        }
    }
    assert_eq!(app.documents[0].text, "after");
    assert_eq!(app.documents[1].text, "before");
    assert!(!app.editor.input.pending());
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
        assert!(app.editor.input.pending());
        assert!(!app.pending.modal());
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.editor.input.pending()
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
        while app.editor.input.pending() || app.documents[0].dirty() || app.documents[0].saving {
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
fn markdown_preview_keeps_following_commands_within_their_admitted_context() {
    for edit_first in [false, true] {
        for barrier in ["none", "pane", "document", "style", "modal", "focus"] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.new_file();
            app.documents[0].acknowledge_save("README.md".into(), "".into(), "revision".into());
            frame(&mut app, &ctx, vec![]);
            frame(&mut app, &ctx, vec![]);
            let mut events = vec![];
            if edit_first {
                events.push(egui::Event::Text("# Source\n".into()));
            }
            events.extend([
                key(Key::R, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT),
                key(Key::P, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT),
            ]);
            frame(&mut app, &ctx, events);
            for _ in 0..4 {
                if app.documents[0].markdown.as_ref().unwrap().showing {
                    break;
                }
                frame(&mut app, &ctx, vec![]);
            }
            assert!(app.documents[0].markdown.as_ref().unwrap().showing);
            assert!(app.editor.input.pending());
            // Intervene after the preview rendered, before its queued command
            // is replayed. Only this same document/view/style/pane may accept it.
            match barrier {
                "pane" => app.navigation.active = navigation::Pane::Explorer,
                "document" => app.new_file(),
                "style" => app.navigation.bindings.select_style(Style::Emacs),
                "modal" => app.set_dialog(Dialog::Shortcuts),
                "focus" => {
                    let _ = ctx.run(
                        egui::RawInput {
                            focused: false,
                            ..Default::default()
                        },
                        |ctx| app.route_input(ctx),
                    );
                }
                _ => {}
            }
            for _ in 0..4 {
                frame(&mut app, &ctx, vec![]);
            }
            assert_eq!(
                app.documents[0].text,
                if edit_first { "# Source\n" } else { "" }
            );
            if barrier == "none" {
                assert!(
                    app.dialog == Dialog::Commands,
                    "The preview discarded the palette shortcut"
                );
            } else if barrier == "modal" {
                assert!(
                    app.dialog == Dialog::Shortcuts,
                    "A queued shortcut replaced the new modal"
                );
            } else {
                assert!(
                    app.dialog == Dialog::None,
                    "Replay crossed the {barrier} barrier"
                );
            }
            assert!(!app.editor.input.pending());
        }
    }
}

#[test]
fn markdown_return_undo_and_save_preserve_native_input_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("README.md");
    let original = "# Original\n";
    std::fs::write(&path, original).unwrap();
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].fixture_content().text = original.into();
    app.documents[0].acknowledge_save(
        path.to_string_lossy().into(),
        original.into(),
        files::digest(original.as_bytes()),
    );
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![egui::Event::Text("changed ".into())]);
    let changed = app.documents[0].text.clone();
    assert_ne!(changed, original);
    // Simulate the acknowledged save made while previewing the edited source.
    std::fs::write(&path, &changed).unwrap();
    app.documents[0].acknowledge_save(
        path.to_string_lossy().into(),
        changed.clone(),
        files::digest(changed.as_bytes()),
    );
    app.toggle_markdown(&ctx);
    frame(&mut app, &ctx, vec![]);
    assert!(app.documents[0].markdown.as_ref().unwrap().showing);
    frame(
        &mut app,
        &ctx,
        vec![
            key(Key::R, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT),
            key(Key::Z, egui::Modifiers::COMMAND),
            key(Key::S, egui::Modifiers::COMMAND),
        ],
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.editor.input.pending() || app.documents[0].saving {
        frame(&mut app, &ctx, vec![]);
        assert!(Instant::now() < deadline, "Preview return/save stalled");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.documents[0].markdown.as_ref().unwrap().showing);
    assert_eq!(
        app.documents[0].text, original,
        "Undo must retain its history"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original,
        "Save must follow the native undo, including when returning from preview"
    );
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
    assert!(app.editor.input.pending());
    frame(&mut app, &ctx, vec![]);
    assert!(app.dialog == Dialog::Extensions);
    assert_eq!(app.documents[0].text, "keep this edit");
    assert!(!app.editor.input.pending());
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
            if !app.editor.input.pending() {
                break;
            }
        }
        assert_eq!(app.documents.active_index(), 0);
        assert_eq!(app.documents[0].text, "typed first", "{style:?}");
        assert_eq!(app.documents[1].text, "second", "{style:?}");
        assert!(!app.editor.input.pending());
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
    assert!(!app.editor.input.pending());
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
        if !app.editor.input.pending() {
            break;
        }
    }
    assert_eq!(app.documents.active_index(), 0);
    assert_eq!(app.documents[0].text, "after first");
    assert_eq!(app.documents[1].text, "before second");
    assert!(!app.editor.input.pending());
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

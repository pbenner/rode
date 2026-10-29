use super::*;
#[test]
fn defaults_avoid_alt_and_require_the_prefix_for_typing_keys() {
    let bindings = bindings::Bindings::load(None);
    assert!(!bindings.leader.modifiers.alt);
    assert!(
        bindings
            .leader
            .matches(Key::Space, egui::Modifiers::CTRL, false)
    );
    assert_eq!(bindings.action(Key::H, egui::Modifiers::NONE, false), None);
    assert_eq!(
        bindings.action(Key::H, egui::Modifiers::NONE, true),
        Some(NavAction::Left)
    );
    assert_eq!(
        bindings.action(Key::H, egui::Modifiers::SHIFT, true),
        Some(NavAction::Narrower)
    );
    assert!(bindings.help().all(|(_, keys)| !keys.contains("Alt")));
}
#[test]
fn leader_consumes_text_and_leaves_shell_keys_alone() {
    let ctx = egui::Context::default();
    let mut state = State::load(None);
    let _ = ctx.run(
        egui::RawInput {
            events: vec![
                egui::Event::Key {
                    key: Key::Space,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL,
                },
                egui::Event::Key {
                    key: Key::H,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Text("h".into()),
            ],
            ..Default::default()
        },
        |ctx| {
            assert_eq!(state.input(ctx), vec![NavAction::Left]);
            assert!(ctx.input(|i| i.events.is_empty()));
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            assert!(state.input(ctx).is_empty());
            assert_eq!(ctx.input(|i| i.events.len()), 1);
        },
    );
}
#[test]
fn list_clamps_and_resets_when_query_changes() {
    let mut list = List::default();
    list.update("file", 3, 1);
    assert_eq!(list.index, 1);
    list.update("file", 3, 90);
    assert_eq!(list.index, 2);
    list.update("file", 0, 1);
    assert_eq!(list.index, 0);
    list.update("theme", 2, 0);
    assert_eq!(list.index, 0);
}

#[test]
fn directions_and_zoom_restore_the_existing_layout_without_closing_documents() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].text = "keep unsaved text".into();
    app.navigation.rects[Pane::Explorer.index()] = Some(egui::Rect::from_min_max(
        egui::pos2(0., 0.),
        egui::pos2(200., 600.),
    ));
    app.navigation.rects[Pane::Editor.index()] = Some(egui::Rect::from_min_max(
        egui::pos2(200., 0.),
        egui::pos2(900., 600.),
    ));
    app.navigate(NavAction::Left, &ctx);
    assert_eq!(app.navigation.active, Pane::Explorer);
    app.navigate(NavAction::Zoom, &ctx);
    assert_eq!(app.navigation.zoom, Some(Pane::Explorer));
    app.navigate(NavAction::Right, &ctx);
    assert_eq!(app.navigation.active, Pane::Editor);
    assert_eq!(app.navigation.zoom, None);
    app.navigate(NavAction::Previous, &ctx);
    assert_eq!(app.navigation.active, Pane::Explorer);
    app.navigate(NavAction::Hide, &ctx);
    assert!(!app.show_explorer);
    assert_eq!(app.navigation.active, Pane::Editor);
    assert_eq!(app.documents[0].text, "keep unsaved text");
    assert!(app.documents[0].dirty());
}

#[test]
fn modal_guards_cancel_the_prefix_and_preserve_dialog_input() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.navigation.leader = Some(Instant::now());
    app.set_dialog(Dialog::Folder);
    let _ = ctx.run(
        egui::RawInput {
            events: vec![
                egui::Event::Key {
                    key: Key::H,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Text("h".into()),
            ],
            ..Default::default()
        },
        |ctx| {
            app.navigation_shortcuts(ctx);
            assert!(app.navigation.leader.is_none());
            assert_eq!(ctx.input(|i| i.events.len()), 2);
        },
    );
}

#[test]
fn emacs_prefix_handles_native_cut_events_and_saves_without_consuming_shell_cut() {
    let ctx = egui::Context::default();
    let mut state = State::load(None);
    state.bindings.select_style(Style::Emacs);
    let _ = ctx.run(
        egui::RawInput {
            events: vec![
                egui::Event::Cut,
                egui::Event::Key {
                    key: Key::X,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                },
            ],
            ..Default::default()
        },
        |ctx| {
            assert!(state.input(ctx).is_empty());
            assert!(state.leader.is_some());
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::S,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        },
        |ctx| {
            assert_eq!(state.input(ctx), vec![NavAction::Save]);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Cut],
            ..Default::default()
        },
        |ctx| {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("terminal-input")));
            assert!(state.input(ctx).is_empty());
            assert!(ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Cut))));
        },
    );
}

//! Pane focus, leader routing and modal/terminal input-boundary regressions.

use super::*;

#[test]
fn tool_focus_is_consumed_only_by_its_requested_native_window() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let target = egui::ViewportId::from_hash_of("individual-tool-window");
    app.navigation.request_focus(Pane::Tools, target);
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.pane_ui(ui, Pane::Tools));
    });
    assert_eq!(app.navigation.pending, Some(Pane::Tools));
    let mut input = egui::RawInput {
        viewport_id: target,
        ..Default::default()
    };
    input.viewports.insert(target, Default::default());
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.pane_ui(ui, Pane::Tools));
    });
    assert_eq!(app.navigation.pending, None);
    assert_eq!(app.navigation.viewports[Pane::Tools.index()], Some(target));
    assert_eq!(
        ctx.memory(|memory| memory.focused()),
        Some(Pane::Tools.id().with(target))
    );
}
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

#[test]
fn batched_emacs_edits_finish_before_the_native_save_prefix() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].text = "alpha beta\ngamma delta\n".into();
    app.navigation.bindings.select_style(Style::Emacs);
    let frame = |app: &mut EditorApp, events: Vec<egui::Event>| {
        let modifiers = if events.iter().any(|event| matches!(event, egui::Event::Cut)) {
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND
        } else {
            egui::Modifiers::NONE
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 700.),
                )),
                events,
                modifiers,
                ..Default::default()
            },
            |ctx| {
                app.route_input(ctx);
                app.editor(ctx);
            },
        );
    };
    frame(&mut app, vec![]);
    frame(&mut app, vec![]);
    let key = |key, pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
    };
    let mut events: Vec<_> = [
        Key::A,
        Key::Space,
        Key::F,
        Key::F,
        Key::W,
        Key::Y,
        Key::A,
        Key::K,
        Key::Y,
        Key::A,
        Key::D,
    ]
    .into_iter()
    .map(|k| key(k, true))
    .collect();
    events.extend([
        egui::Event::Text("A".into()),
        egui::Event::Cut,
        key(Key::S, true),
    ]);
    frame(&mut app, events);
    for _ in 0..20 {
        frame(&mut app, vec![]);
    }
    assert_eq!(app.documents[0].text, "Alpha beta\ngamma delta\n");
    assert!(
        matches!(app.dialog, Dialog::Save(_)),
        "The save prefix must still open Save As"
    );
}

//! Pane focus, leader routing and modal/terminal input-boundary regressions.

use super::*;

#[test]
fn a_floating_layer_click_does_not_select_the_pane_underneath_it() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.navigation.used = true;
    app.navigation.active = Pane::Explorer;
    let point = egui::pos2(300., 100.);
    for pressed in [None, Some(true), Some(false)] {
        let events = pressed.map_or_else(Vec::new, |pressed| {
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        });
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500., 300.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::SidePanel::left("overlay-focus-explorer")
                    .exact_width(200.)
                    .frame(layout::pane_frame(egui::Color32::BLACK))
                    .show(ctx, |ui| app.pane_ui(ui, Pane::Explorer));
                egui::CentralPanel::default()
                    .frame(layout::pane_frame(egui::Color32::BLACK))
                    .show(ctx, |ui| app.pane_ui(ui, Pane::Editor));
                egui::Area::new(egui::Id::new("floating-pane-control"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(egui::pos2(250., 75.))
                    // Keep sizing estimates from relocating the click target.
                    .constrain(false)
                    .show(ctx, |ui| {
                        let response = ui.add_sized([100., 50.], egui::Button::new("Overlay"));
                        assert!(response.rect.contains(point));
                    });
            },
        );
        assert_eq!(
            app.navigation.active,
            Pane::Explorer,
            "Floating control selected the editor underneath (pressed: {pressed:?}, layer: {:?})",
            ctx.layer_id_at(point)
        );
    }
}

#[test]
fn a_resize_gutter_does_not_steal_focus_for_the_adjacent_pane() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.navigation.used = true;
    app.navigation.active = Pane::Explorer;
    let resize_id = egui::Id::new("focused-resize-gutter");
    for _ in 0..3 {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500., 300.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::SidePanel::left("focus-claim-explorer")
                    .exact_width(200.)
                    .frame(layout::pane_frame(egui::Color32::BLACK))
                    .show(ctx, |ui| app.pane_ui(ui, Pane::Explorer));
                egui::CentralPanel::default()
                    .frame(layout::pane_frame(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        let response = ui.interact(
                            egui::Rect::from_center_size(
                                egui::pos2(200., 150.),
                                egui::vec2(8., 100.),
                            ),
                            resize_id,
                            Sense::focusable_noninteractive(),
                        );
                        response.request_focus();
                        app.pane_ui(ui, Pane::Editor)
                    });
            },
        );
        assert_eq!(
            app.navigation.active,
            Pane::Explorer,
            "Resize gutter selected the neighbouring editor"
        );
    }
}

#[test]
fn inactive_native_windows_do_not_duplicate_the_active_pane_outline() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.navigation.used = true;
    app.navigation.active = Pane::Tools;
    let child = egui::ViewportId::from_hash_of("other-pane-window");
    let color = app.themes.active().palette.accent.gamma_multiply(0.6);
    for (viewport, focused) in [
        (egui::ViewportId::ROOT, true),
        (child, false),
        (child, true),
        (egui::ViewportId::ROOT, false),
        (egui::ViewportId::ROOT, true),
    ] {
        let mut input = egui::RawInput {
            viewport_id: viewport,
            focused,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400., 300.),
            )),
            ..Default::default()
        };
        input.viewports.insert(
            viewport,
            egui::ViewportInfo {
                focused: Some(focused),
                ..Default::default()
            },
        );
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(layout::pane_frame(egui::Color32::BLACK))
                .show(ctx, |ui| app.pane_ui(ui, Pane::Tools));
        });
        let painted = output.shapes.iter().any(
            |shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.stroke.color == color),
        );
        assert_eq!(
            painted, focused,
            "Incorrect native focus outline in {viewport:?}"
        );
    }
}

#[test]
fn explorer_focus_outline_clears_toolbar_controls_across_themes_and_scales() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pane-layout.json");
    for theme in crate::theme::Theme::builtins() {
        for scale in [0.75, 1., 1.25, 2.] {
            for width in [180., 250., 450.] {
                let ctx = egui::Context::default();
                let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                let mut app = EditorApp::new(
                    &cc,
                    Startup {
                        theme: Some(theme.id.clone()),
                        ..Default::default()
                    },
                );
                ctx.set_pixels_per_point(scale);
                ui_probe::install_at(&ctx, path.clone());
                app.set_explorer_visible(egui::ViewportId::ROOT, true);
                app.navigation.active = Pane::Explorer;
                app.navigation.explorer_width = Some(width);
                let mut unfocused = None;
                for focused in [false, true] {
                    app.navigation.used = focused;
                    let mut output = None;
                    for _ in 0..3 {
                        output = Some(ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(1000., 700.),
                                )),
                                ..Default::default()
                            },
                            |ctx| app.explorer(ctx),
                        ));
                    }
                    let state: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    let control = |name| {
                        let button = state["buttons"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|button| button["name"] == name)
                            .unwrap_or_else(|| panic!("Missing {name}"));
                        let r: [f32; 4] = serde_json::from_value(button["rect"].clone()).unwrap();
                        egui::Rect::from_min_max(egui::pos2(r[0], r[1]), egui::pos2(r[2], r[3]))
                    };
                    let controls = [
                        control("explorer-new-document"),
                        control("explorer-refresh-expanded"),
                    ];
                    let output = output.unwrap();
                    let title = layout::tests::text_bounds(&output, "Explorer");
                    let outer = app.navigation.native_rects[&egui::ViewportId::ROOT]
                        [Pane::Explorer.index()]
                    .unwrap();
                    assert_eq!(outer.left_top(), egui::Pos2::ZERO, "Inset pane boundary");
                    let safe = outer.shrink2(egui::vec2(11.5, 7.5));
                    for rect in controls {
                        assert!(
                            safe.contains_rect(rect) && title.right() < rect.left(),
                            "{} at width {width}, scale {scale}: toolbar {rect:?} overlaps title {title:?} or outline {outer:?}",
                            theme.id,
                        );
                        assert!(
                            rect.width() >= 24. && rect.height() >= 24.,
                            "Cramped toolbar target: {rect:?}"
                        );
                    }
                    for pair in controls.windows(2) {
                        assert!(
                            pair[0].right() + 3.5 <= pair[1].left(),
                            "Toolbar targets touch: {pair:?}"
                        );
                        assert!(
                            (pair[0].center().y - pair[1].center().y).abs() < 0.5,
                            "Misaligned toolbar targets: {pair:?}"
                        );
                    }
                    if focused {
                        assert_eq!(Some(controls), unfocused, "Focus changed toolbar geometry");
                        assert!(
                            output.shapes.iter().any(|shape| {
                                matches!(&shape.shape, egui::Shape::Rect(rect)
                                if outer.contains_rect(rect.rect)
                                    && rect.rect.left() > outer.left()
                                    && rect.rect.right() < outer.right()
                                    && rect.rect.top() > outer.top()
                                    && rect.rect.bottom() < outer.bottom()
                                    && rect.stroke.color
                                    == app.themes.active().palette.accent.gamma_multiply(0.6))
                            }),
                            "Missing Explorer focus outline"
                        );
                    } else {
                        unfocused = Some(controls);
                    }
                }
            }
        }
    }
}

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
    app.set_explorer_visible(egui::ViewportId::ROOT, true);
    app.documents[0].fixture_content().text = "keep unsaved text".into();
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
    assert!(app.presentation.sidebar.is_none());
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
    app.documents[0].fixture_content().text = "alpha beta\ngamma delta\n".into();
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

#[test]
fn an_armed_leader_survives_editor_focus_restoration() {
    struct StoredLeader;
    impl eframe::Storage for StoredLeader {
        fn get_string(&self, name: &str) -> Option<String> {
            (name == "keyboard.leader").then(|| "Ctrl+Shift+F12".into())
        }
        fn set_string(&mut self, _: &str, _: String) {}
        fn flush(&mut self) {}
    }
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.navigation.bindings = bindings::Bindings::load(Some(&StoredLeader));
    let render = |app: &mut EditorApp, events| {
        let _ = ctx.run(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(850., 560.),
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
    render(&mut app, vec![]);
    let document = app.documents.active().unwrap().id;
    let target = egui::Id::new(("document", document));
    ctx.memory_mut(|m| m.surrender_focus(target));
    app.navigation
        .request_focus(Pane::Editor, egui::ViewportId::ROOT);
    render(
        &mut app,
        vec![egui::Event::Key {
            key: Key::F12,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        }],
    );
    assert!(app.navigation.leader.is_some());
    assert!(ctx.memory(|m| m.has_focus(target)));
    render(&mut app, vec![]);
    assert!(
        app.navigation.leader.is_some(),
        "Restoring the same editor focus discarded an accepted global prefix"
    );
    render(
        &mut app,
        vec![
            egui::Event::Key {
                key: Key::F,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("f".into()),
        ],
    );
    assert!(app.navigation.leader.is_none());
    assert_eq!(app.navigation.active, Pane::Explorer);
    assert!(
        app.documents.active().unwrap().text.is_empty(),
        "The leader selector leaked into the source buffer"
    );
}

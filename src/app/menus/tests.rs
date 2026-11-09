use super::*;

fn frame(
    app: &mut EditorApp,
    ctx: &egui::Context,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ctx| app.titlebar(ctx),
    )
}

fn control(path: &std::path::Path, name: &str) -> egui::Rect {
    let state: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let button = state["buttons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|button| button["name"] == name)
        .unwrap_or_else(|| panic!("Missing {name}: {state}"));
    assert_eq!(button["enabled"], true, "{name}: {state}");
    let [left, top, right, bottom]: [f32; 4] =
        serde_json::from_value(button["rect"].clone()).unwrap();
    egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
}

fn click(app: &mut EditorApp, ctx: &egui::Context, size: egui::Vec2, rect: egui::Rect) {
    let point = rect.center();
    for pressed in [true, false] {
        frame(
            app,
            ctx,
            size,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
        );
    }
    for _ in 0..3 {
        frame(app, ctx, size, vec![]);
    }
}

fn escape(app: &mut EditorApp, ctx: &egui::Context, size: egui::Vec2) {
    for pressed in [true, false] {
        frame(
            app,
            ctx,
            size,
            vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
    }
    assert!(!egui::Popup::is_any_open(ctx));
    for _ in 0..3 {
        frame(app, ctx, size, vec![]);
    }
}

fn first_menu_row(path: &std::path::Path) -> egui::Rect {
    let state: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let name = state["buttons"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|button| {
            let name = button["name"].as_str()?;
            (name.starts_with("menu-item:") || name.starts_with("submenu:")).then_some(name)
        })
        .expect("No visible menu rows");
    control(path, name)
}

fn scroll_to_end(app: &mut EditorApp, ctx: &egui::Context, size: egui::Vec2, row: egui::Rect) {
    // Use the solid scrollbar gutter so scrolling does not hover another submenu.
    let point = egui::pos2(row.right() + 5., row.center().y);
    for _ in 0..4 {
        frame(
            app,
            ctx,
            size,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., -2000.),
                    modifiers: Default::default(),
                },
            ],
        );
    }
}

#[test]
fn file_menu_aligns_shortcuts_and_opens_the_existing_project_dialog() {
    for theme in ["phosphor", "studio-neutral"] {
        for scale in [1., 1.5] {
            let directory = tempfile::tempdir().unwrap();
            let state = directory.path().join("ui.json");
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(
                &cc,
                Startup {
                    theme: Some(theme.into()),
                    ..Default::default()
                },
            );
            ctx.set_zoom_factor(scale);
            ctx.style_mut(|style| style.animation_time = 0.);
            ui_probe::install_at(&ctx, state.clone());
            let size = egui::vec2(1380., 1500.);
            frame(&mut app, &ctx, size, vec![]);
            click(&mut app, &ctx, size, control(&state, "menu-file"));
            let output = frame(&mut app, &ctx, size, vec![]);
            let mut left = None::<f32>;
            let mut right = None::<f32>;
            for (label, shortcut) in [
                ("New file", "Ctrl+N"),
                ("Open file…", "Ctrl+P"),
                ("Open project folder…", "Ctrl+Shift+O"),
                ("Save", "Ctrl+S"),
            ] {
                let label = layout::tests::text_bounds(&output, label);
                let hint = layout::tests::text_bounds(&output, shortcut);
                assert!(
                    label.right() + 12. <= hint.left(),
                    "{theme}, {scale}: {label:?} overlaps {hint:?}"
                );
                assert!((label.center().y - hint.center().y).abs() < 1.);
                assert!((label.left() - *left.get_or_insert(label.left())).abs() < 1.);
                assert!((hint.right() - *right.get_or_insert(hint.right())).abs() < 1.);
            }
            click(
                &mut app,
                &ctx,
                size,
                control(&state, "menu-item:Open project folder…"),
            );
            assert!(app.dialog == Dialog::Folder);
            assert!(!egui::Popup::is_any_open(&ctx));
            assert!(app.pending.is_none() && !app.preparing);
        }
    }
}

#[test]
fn compact_menus_scroll_to_last_actions_and_keep_view_toggles_open() {
    for theme in ["phosphor", "studio-neutral"] {
        for scale in [1., 1.5, 2.] {
            let directory = tempfile::tempdir().unwrap();
            let state = directory.path().join("ui.json");
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(
                &cc,
                Startup {
                    theme: Some(theme.into()),
                    ..Default::default()
                },
            );
            ctx.set_zoom_factor(scale);
            ctx.style_mut(|style| style.animation_time = 0.);
            ui_probe::install_at(&ctx, state.clone());
            let size = egui::vec2(800., 560.);
            frame(&mut app, &ctx, size, vec![]);
            click(&mut app, &ctx, size, control(&state, "menu-file"));
            let first = control(&state, "menu-item:New file");
            scroll_to_end(&mut app, &ctx, size, first);
            let quit = control(&state, "menu-item:Quit");
            assert!(
                ctx.content_rect().contains_rect(quit),
                "{theme}, {scale}: {quit:?}"
            );
            click(&mut app, &ctx, size, control(&state, "menu-view"));
            let scale_menu = control(&state, "submenu:Interface scale: 100%");
            for _ in 0..4 {
                frame(
                    &mut app,
                    &ctx,
                    size,
                    vec![egui::Event::PointerMoved(scale_menu.center())],
                );
            }
            let larger = control(&state, "interface-larger");
            scroll_to_end(&mut app, &ctx, size, larger);
            let reset = control(&state, "interface-reset");
            assert!(
                ctx.content_rect().contains_rect(reset),
                "{theme}, {scale}: {reset:?}"
            );
            escape(&mut app, &ctx, size);
            click(&mut app, &ctx, size, control(&state, "menu-view"));
            scroll_to_end(&mut app, &ctx, size, first_menu_row(&state));
            let original = app.show_hidden;
            click(
                &mut app,
                &ctx,
                size,
                control(&state, "menu-item:Show hidden files"),
            );
            assert_ne!(app.show_hidden, original);
            assert!(app.tree_dirty);
            assert!(egui::Popup::is_any_open(&ctx));
            escape(&mut app, &ctx, size);
        }
    }
}

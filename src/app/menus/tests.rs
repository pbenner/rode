use super::*;

#[test]
fn disabling_features_removes_their_activity_menu_and_palette_entries_and_reenabling_restores_them()
{
    use crate::extensions::Feature;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("visibility.json");
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    ui_probe::install_at(&ctx, path.clone());
    app.dialog_input = "Stocks: Market charts and watchlist".into();
    for enabled in [false, true] {
        for feature in [
            Feature::Stocks,
            Feature::Git,
            Feature::Monitor,
            Feature::Agents,
            Feature::Debugger,
            Feature::Languages,
            Feature::Notebooks,
            Feature::Marp,
            Feature::Binary,
        ] {
            app.set_extension(feature.id(), enabled);
        }
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1380., 1600.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    app.titlebar(ctx);
                    app.activity(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.menu_commands(ui, ctx, Command::PANELS);
                        app.menu_commands(
                            ui,
                            ctx,
                            &[Command::NewNotebook, Command::MarpNew, Command::Hex],
                        );
                        app.command_dialog(ui, ctx);
                    });
                },
            );
        }
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let names: HashSet<_> = snapshot["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        for name in [
            "activity:Stocks",
            "activity:System monitor",
            "menu-item:Stocks",
            "menu-item:New Jupyter notebook",
            "menu-item:New Marp presentation",
            "command-match:Stocks: Market charts and watchlist",
            "menu-debug",
            "menu-code",
        ] {
            assert_eq!(
                names.contains(name),
                enabled,
                "{name}, enabled={enabled}: {snapshot}"
            );
        }
        for name in [
            "activity:Projects",
            "activity:Connections",
            "activity:App settings",
            "activity:Extensions…",
            "menu-file",
            "menu-view",
            "menu-item:Problems",
        ] {
            assert!(names.contains(name), "{name}: {snapshot}");
        }
    }
}

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

fn scroll_menu(
    app: &mut EditorApp,
    ctx: &egui::Context,
    size: egui::Vec2,
    row: egui::Rect,
    end: bool,
) {
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
                    delta: egui::vec2(0., if end { -2000. } else { 2000. }),
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
            assert!(app.pending.is_none() && !app.workspace.preparing());
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
            scroll_menu(&mut app, &ctx, size, first, true);
            let quit = control(&state, "menu-item:Quit");
            assert!(
                ctx.content_rect().contains_rect(quit),
                "{theme}, {scale}: {quit:?}"
            );
            click(&mut app, &ctx, size, control(&state, "menu-view"));
            // Opening another compact menu can retain a scroll offset. Reveal
            // its first actions before following a submenu's settling bounds.
            scroll_menu(&mut app, &ctx, size, first_menu_row(&state), false);
            for _ in 0..4 {
                let scale_menu = control(&state, "submenu:Interface scale: 100%");
                frame(
                    &mut app,
                    &ctx,
                    size,
                    vec![egui::Event::PointerMoved(scale_menu.center())],
                );
            }
            let larger = control(&state, "interface-larger");
            scroll_menu(&mut app, &ctx, size, larger, true);
            let reset = control(&state, "interface-reset");
            assert!(
                ctx.content_rect().contains_rect(reset),
                "{theme}, {scale}: {reset:?}"
            );
            escape(&mut app, &ctx, size);
            click(&mut app, &ctx, size, control(&state, "menu-view"));
            scroll_menu(&mut app, &ctx, size, first_menu_row(&state), true);
            let original = app.show_hidden;
            click(
                &mut app,
                &ctx,
                size,
                control(&state, "menu-item:Show hidden files"),
            );
            assert_ne!(app.show_hidden, original);
            assert!(app.explorer_state.tree_dirty);
            assert!(egui::Popup::is_any_open(&ctx));
            escape(&mut app, &ctx, size);
        }
    }
}

#[test]
fn opening_the_debugger_panel_does_not_advertise_or_start_its_f5_action() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("ui.json");
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    ctx.style_mut(|style| style.animation_time = 0.);
    ui_probe::install_at(&ctx, state.clone());
    let size = egui::vec2(1380., 1000.);
    frame(&mut app, &ctx, size, vec![]);
    click(&mut app, &ctx, size, control(&state, "menu-view"));
    click(
        &mut app,
        &ctx,
        size,
        control(&state, "submenu:Workspace panels"),
    );
    let output = frame(&mut app, &ctx, size, vec![]);
    assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::Shape::Text(text) if text.galley.text() == "F5"
    )));
    app.notice = None;
    click(&mut app, &ctx, size, control(&state, "menu-item:Debugger"));
    assert_eq!(app.workbench.panel, Some(Panel::Debug));
    assert!(app.debugger.client.is_none() && app.notice.is_none());
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::F5,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        },
        |ctx| app.shortcuts(ctx),
    );
    assert!(app.notice.as_ref().is_some_and(
        |(message, _, _)| message == "Open a connected project to debug its programs."
    ));
    assert!(app.debugger.client.is_none());
}

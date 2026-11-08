use super::*;

fn frame(
    app: &mut EditorApp,
    ctx: &egui::Context,
    state: &std::path::Path,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> serde_json::Value {
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ctx| {
            app.dialogs(ctx);
            app.notice_ui(ctx);
        },
    );
    serde_json::from_slice(&std::fs::read(state).unwrap()).unwrap()
}

fn control(state: &serde_json::Value, name: &str) -> egui::Rect {
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

#[test]
fn project_validation_and_actions_stay_visible_when_rules_scroll_on_small_windows() {
    for theme in ["phosphor", "studio-neutral"] {
        for scale in [1., 1.5] {
            for remote in [false, true] {
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
                if remote {
                    app.workspace = Some(crate::app::tests::remote_workspace());
                }
                app.set_dialog(Dialog::Folder);
                app.folder_needs_load = false;
                app.folder_loading = false;
                app.folder_sandbox.enabled = true;
                app.notice = Some((
                    "An existing project notification must not cover the folder dialog's actions."
                        .repeat(3),
                    true,
                    Instant::now(),
                ));
                app.dialog_input = "/work/a-long-project-path/selected-project".into();
                app.folder_policy_path.clone_from(&app.dialog_input);
                app.folder_entries = (0..50)
                    .map(|index| Entry {
                        path: format!("{}/child-{index}", app.dialog_input),
                        name: format!("child-{index}"),
                        dir: true,
                    })
                    .collect();
                let initial_size = egui::vec2(1380., 1000.);
                for _ in 0..3 {
                    frame(&mut app, &ctx, &state, initial_size, vec![]);
                }
                let snapshot = frame(&mut app, &ctx, &state, initial_size, vec![]);
                let point = control(&snapshot, "project-dialog-options").center();
                let pointer = |point, pressed| egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                };
                frame(
                    &mut app,
                    &ctx,
                    &state,
                    initial_size,
                    vec![egui::Event::PointerMoved(point), pointer(point, true)],
                );
                frame(
                    &mut app,
                    &ctx,
                    &state,
                    initial_size,
                    vec![pointer(point, false)],
                );
                let snapshot = frame(&mut app, &ctx, &state, initial_size, vec![]);
                control(&snapshot, "project-dialog-options-open");
                app.folder_sandbox.read_only = vec!["relative/path".into()];
                let point = control(&snapshot, "project-dialog-open").center();
                frame(
                    &mut app,
                    &ctx,
                    &state,
                    initial_size,
                    vec![egui::Event::PointerMoved(point), pointer(point, true)],
                );
                frame(
                    &mut app,
                    &ctx,
                    &state,
                    initial_size,
                    vec![pointer(point, false)],
                );
                assert!(app.dialog == Dialog::Folder && app.folder_error.is_some());
                for size in [
                    egui::vec2(1380., 900.),
                    egui::vec2(850., 560.),
                    egui::vec2(650., 480.),
                ] {
                    for _ in 0..3 {
                        frame(&mut app, &ctx, &state, size, vec![]);
                    }
                    let snapshot = frame(&mut app, &ctx, &state, size, vec![]);
                    let open = control(&snapshot, "project-dialog-open");
                    let cancel = control(&snapshot, "project-dialog-cancel");
                    let path = control(&snapshot, "project-dialog-path");
                    let error = control(&snapshot, "project-dialog-error");
                    let viewport = ctx.content_rect().shrink(8.);
                    for rect in [open, cancel, path, error] {
                        assert!(
                            viewport.contains_rect(rect),
                            "{theme}, scale {scale}, remote {remote}, {size:?}: {rect:?} outside {viewport:?}"
                        );
                    }
                    assert!(cancel.right() < open.left());
                    assert!((open.center().y - cancel.center().y).abs() <= 1.);
                    assert!(path.bottom() < open.top());
                    assert!(path.bottom() <= error.top() && error.bottom() < open.top());
                    assert_eq!(
                        ctx.layer_id_at(open.center()).unwrap().order,
                        egui::Order::Foreground,
                        "A background notice covers Open project"
                    );
                    assert_eq!(
                        ctx.layer_id_at(cancel.center()).unwrap().order,
                        egui::Order::Foreground,
                        "A background notice covers Cancel"
                    );
                    assert_eq!(
                        app.dialog_input,
                        "/work/a-long-project-path/selected-project"
                    );
                    assert_eq!(app.folder_remote, remote);
                    assert!(app.pending.is_none() && !app.preparing);
                }
                let size = egui::vec2(650., 480.);
                let snapshot = frame(&mut app, &ctx, &state, size, vec![]);
                let point = control(&snapshot, "project-dialog-cancel").center();
                frame(
                    &mut app,
                    &ctx,
                    &state,
                    size,
                    vec![egui::Event::PointerMoved(point), pointer(point, true)],
                );
                frame(&mut app, &ctx, &state, size, vec![pointer(point, false)]);
                assert!(app.dialog == Dialog::None);
                assert!(app.pending.is_none() && !app.preparing);
                assert!(app.notice.is_some(), "Browsing must retain the notice");
            }
        }
    }
}

#[test]
fn themed_folder_lists_reveal_the_last_row_without_clipping_its_label() {
    for theme in ["phosphor", "studio-neutral", "studio"] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _app = EditorApp::new(
            &cc,
            Startup {
                theme: Some(theme.into()),
                ..Default::default()
            },
        );
        let entries: Vec<_> = (0..50)
            .map(|index| Entry {
                path: format!("/project/child-{index}"),
                name: format!("child-{index}"),
                dir: true,
            })
            .collect();
        let mut list = navigation::List::default();
        let mut output = egui::FullOutput::default();
        for _ in 0..4 {
            output = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(300., 120.),
                        Layout::top_down(Align::Min),
                        |ui| {
                            file_browser::rows(
                                ui,
                                "folders",
                                &entries,
                                false,
                                &mut list,
                                "fixture",
                                (0, false),
                                Some("/project/child-49"),
                                true,
                            );
                        },
                    );
                });
            });
        }
        let (bounds, clip) = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text().ends_with("child-49") => Some((
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                    shape.clip_rect,
                )),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{theme}: the final folder was not rendered"));
        assert!(
            clip.contains_rect(bounds),
            "{theme}: {bounds:?} outside {clip:?}"
        );
    }
}

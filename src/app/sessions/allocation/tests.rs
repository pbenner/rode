//! Allocation mode selection, admission and captured policy stay independent of running shells.
use super::*;

fn app(ctx: &egui::Context, requested: bool, active: bool) -> EditorApp {
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let mut workspace = crate::app::tests::remote_workspace();
    workspace.remote = None;
    workspace.plan.requested = requested;
    workspace.plan.active = active;
    workspace.plan.prefix = vec!["sandbox-fixture".into(), "--".into()];
    workspace.plan.reason = if requested && !active {
        "Fixture sandbox unavailable".into()
    } else {
        String::new()
    };
    app.workspace.set_fixture(Some(workspace));
    app
}

#[test]
fn slurm_remembers_its_own_mode_without_bypassing_an_unavailable_sandbox() {
    for (requested, active) in [(false, false), (true, false), (true, true)] {
        for terminal_mode in [false, true] {
            for slurm_mode in [false, true] {
                let ctx = egui::Context::default();
                let mut app = app(&ctx, requested, active);
                app.workbench.save_terminal_sandbox(terminal_mode);
                app.workbench.save_slurm_sandbox(slurm_mode);
                // Observe opening the form without starting scheduler discovery.
                app.sessions.slurm_busy = true;
                app.open_slurm(&ctx);
                assert_eq!(app.sessions.allocation_sandbox, slurm_mode);
                assert_eq!(app.workbench.terminal_sandbox(), terminal_mode);
                assert_eq!(app.workbench.slurm_sandbox(), slurm_mode);
                let plan = app.slurm_allocation_plan();
                if slurm_mode && !active {
                    assert!(plan.is_err());
                    if requested {
                        assert!(plan.unwrap_err().contains("Fixture sandbox unavailable"));
                    }
                } else {
                    assert_eq!(plan.unwrap().active, slurm_mode && active);
                }
            }
        }
    }
}

#[test]
fn allocation_modes_capture_the_selected_policy_and_preserve_the_project_plan() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, true, true);
    let project = app.session_plan();
    for sandboxed in [false, true] {
        app.sessions.allocation_sandbox = sandboxed;
        let plan = app.slurm_allocation_plan().unwrap();
        assert_eq!(plan.root, project.root);
        assert_eq!(plan.active, sandboxed);
        assert_eq!(plan.prefix.is_empty(), !sandboxed);
        assert_eq!(
            service::fingerprint(&plan),
            service::fingerprint(&app.terminal_plan(sandboxed))
        );
        assert_eq!(
            service::fingerprint(&app.session_plan()),
            service::fingerprint(&project)
        );
    }
}

#[test]
fn allocation_controls_show_both_modes_and_block_unavailable_or_busy_launches() {
    for width in [430., 900.] {
        for active in [false, true] {
            for busy in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("allocation.json");
                let ctx = egui::Context::default();
                let mut app = app(&ctx, true, active);
                app.sessions.allocation_sandbox = true;
                app.sessions.starting = busy;
                let preferred = app.workbench.terminal_sandbox();
                let slurm_preferred = app.workbench.slurm_sandbox();
                super::super::super::ui_probe::install_at(&ctx, path.clone());
                let render = |app: &mut EditorApp, events| {
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 1000.),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| app.slurm_dialog(ui, ctx));
                        },
                    );
                    serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap())
                        .unwrap()
                };
                render(&mut app, vec![]);
                let snapshot = render(&mut app, vec![]);
                let controls = snapshot["buttons"].as_array().unwrap();
                assert!(controls.iter().any(|b| b["name"]
                    == format!(
                        "slurm-execution-mode:{}",
                        crate::files::digest(b"sandboxed")
                    )));
                let control = |name| controls.iter().find(|b| b["name"] == name).unwrap();
                assert_eq!(control("slurm-mode-sandbox")["enabled"], active && !busy);
                assert_eq!(control("slurm-mode-host")["enabled"], !busy);
                for name in ["slurm-allocate", "slurm-reserve"] {
                    assert_eq!(control(name)["enabled"], active && !busy);
                }
                let rect = &control("slurm-mode-host")["rect"];
                let point = egui::pos2(
                    (rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap()) as f32 / 2.,
                    (rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap()) as f32 / 2.,
                );
                for pressed in [true, false] {
                    render(
                        &mut app,
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
                let snapshot = render(&mut app, vec![]);
                assert_eq!(app.sessions.allocation_sandbox, busy);
                assert!(
                    snapshot["buttons"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|b| b["name"]
                            == format!(
                                "slurm-execution-mode:{}",
                                crate::files::digest(if busy {
                                    b"sandboxed"
                                } else {
                                    b"outside_sandbox"
                                })
                            ))
                );
                for name in ["slurm-allocate", "slurm-reserve"] {
                    let button = snapshot["buttons"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|b| b["name"] == name)
                        .unwrap();
                    assert_eq!(button["enabled"], !busy);
                }
                assert_eq!(app.workbench.terminal_sandbox(), preferred);
                assert_eq!(app.workbench.slurm_sandbox(), slurm_preferred);
                assert_eq!(app.session_plan().active, active);
                assert_eq!(app.sessions.view_count(), 0);
            }
        }
    }
}

#[test]
fn allocation_footer_stays_visible_with_expanded_options_errors_and_scaled_windows() {
    fn control(snapshot: &serde_json::Value, name: &str) -> egui::Rect {
        let button = snapshot["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|button| button["name"] == name)
            .unwrap_or_else(|| panic!("Missing {name}: {snapshot}"));
        let [left, top, right, bottom]: [f32; 4] =
            serde_json::from_value(button["rect"].clone()).unwrap();
        egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
    }

    for theme in ["carbon", "sumie-light"] {
        for scale in [0.75, 1., 1.5] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("allocation.json");
            let ctx = egui::Context::default();
            let mut app = app(&ctx, true, true);
            app.themes.select(theme, &ctx).unwrap();
            ctx.set_zoom_factor(scale);
            ctx.style_mut(|style| style.animation_time = 0.);
            app.set_dialog(Dialog::Slurm);
            super::super::super::ui_probe::install_at(&ctx, path.clone());
            let render = |app: &mut EditorApp, size, events| {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        events,
                        ..Default::default()
                    },
                    |ctx| app.dialogs(ctx),
                );
                let snapshot =
                    serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap())
                        .unwrap();
                (snapshot, output)
            };
            let large = egui::vec2(1380., 1000.);
            for _ in 0..5 {
                render(&mut app, large, vec![]);
            }
            // A short form must keep its actions beside the content instead of
            // centring them in unused viewport space. Text fields share their
            // row's control height even when the interface is zoomed out.
            let (snapshot, output) = render(&mut app, large, vec![]);
            let actions = control(&snapshot, "slurm-allocate");
            let lifetime = control(&snapshot, "slurm-lifetime");
            assert!(
                actions.top() - lifetime.bottom() <= 2. * actions.height(),
                "Detached footer at {theme}/{scale}: {lifetime:?}, {actions:?}"
            );
            for value in ["2", "80"] {
                let (field, clip) = crate::app::layout::tests::button_frame(&output, value);
                assert!(clip.contains_rect(field));
                assert!(
                    (field.height() - actions.height()).abs() <= 1.,
                    "Uneven controls at {theme}/{scale}: field {field:?}, action {actions:?}"
                );
            }
            // Open real disclosure controls before resizing, so retained grid
            // widths and long content exercise the same modal as native input.
            for name in ["slurm-lifetime", "slurm-advanced"] {
                let (snapshot, _) = render(&mut app, large, vec![]);
                let point = control(&snapshot, name).center();
                for pressed in [true, false] {
                    render(
                        &mut app,
                        large,
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
            }
            app.sessions.slurm_error = "Scheduler temporarily unavailable. ".repeat(20);
            app.sessions.allocation_error = "Invalid allocation setting. ".repeat(20);
            for size in [
                large,
                egui::vec2(850., 560.),
                egui::vec2(650., 480.),
                egui::vec2(480., 640.),
            ] {
                for _ in 0..5 {
                    render(&mut app, size, vec![]);
                }
                let (snapshot, output) = render(&mut app, size, vec![]);
                let viewport = ctx.content_rect().shrink(8.);
                let buttons = ["slurm-allocate", "slurm-reserve", "slurm-close"]
                    .map(|name| control(&snapshot, name));
                for (index, rect) in buttons.iter().enumerate() {
                    assert!(
                        viewport.contains_rect(*rect),
                        "{theme}/{scale}/{size:?}: {rect:?} outside {viewport:?}"
                    );
                    assert!(
                        rect.height()
                            >= app
                                .themes
                                .active()
                                .palette
                                .ui
                                .unwrap_or_default()
                                .control_height
                                - 1.
                    );
                    for other in &buttons[index + 1..] {
                        assert!(
                            !rect.intersects(*other),
                            "Overlapping actions: {rect:?}, {other:?}"
                        );
                    }
                }
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape
                        && ["Allocate in new terminal", "Reserve for notebooks", "Close"]
                            .contains(&text.galley.text())
                    {
                        let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                        assert!(
                            shape.clip_rect.contains_rect(bounds),
                            "Clipped action: {} at {theme}/{scale}/{size:?}",
                            text.galley.text()
                        );
                    }
                }
            }
            // Submitting invalid input must reveal feedback even when the
            // expanded form is scrolled away from the validation message.
            app.sessions.slurm_error.clear();
            app.sessions.allocation_error.clear();
            app.sessions.allocation_draft.time = "2:99".into();
            let size = egui::vec2(850., 560.);
            for _ in 0..5 {
                render(&mut app, size, vec![]);
            }
            let (snapshot, _) = render(&mut app, size, vec![]);
            let point = control(&snapshot, "slurm-allocate").center();
            for pressed in [true, false] {
                render(
                    &mut app,
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
            for _ in 0..5 {
                render(&mut app, size, vec![]);
            }
            let (_, output) = render(&mut app, size, vec![]);
            assert!(!app.sessions.allocation_error.is_empty());
            assert!(
                output.shapes.iter().any(|shape| {
                    if let egui::Shape::Text(text) = &shape.shape {
                        text.galley.text() == app.sessions.allocation_error
                            && shape.clip_rect.contains_rect(egui::Rect::from_min_size(
                                text.pos,
                                text.galley.size(),
                            ))
                    } else {
                        false
                    }
                }),
                "Validation feedback is hidden at {theme}/{scale}: {}",
                app.sessions.allocation_error
            );
        }
    }
}

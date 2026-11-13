//! Headless pointer gestures produce list edits, never accidental SSH starts.
use super::*;

fn profiles() -> Vec<Profile> {
    ["Alpha", "Beta", "Gamma"]
        .into_iter()
        .map(|name| Profile {
            name: name.into(),
            host: format!("{name}.invalid"),
            ..Default::default()
        })
        .collect()
}

fn frame(ctx: &egui::Context, events: Vec<egui::Event>) -> (egui::FullOutput, Option<Intent>) {
    let mut intent = None;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400., 600.),
            )),
            time: Some(ctx.input(|i| i.time) + 0.1),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                intent = connection_rows(ui, &profiles(), theme::Palette::default());
            });
        },
    );
    (output, intent)
}

fn text_rect(output: &egui::FullOutput, label: &str) -> egui::Rect {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing connection control {label}"))
}

fn pointer(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Default::default(),
    }
}

fn click(
    ctx: &egui::Context,
    pos: egui::Pos2,
    button: egui::PointerButton,
) -> (egui::FullOutput, Option<Intent>) {
    let (_, intent) = frame(
        ctx,
        vec![egui::Event::PointerMoved(pos), pointer(pos, button, true)],
    );
    assert!(
        intent.is_none(),
        "Pressing a row must not connect before release"
    );
    frame(ctx, vec![pointer(pos, button, false)])
}

#[test]
fn saved_connection_context_menu_removes_edits_and_moves_without_connecting() {
    for menu in [
        "Remove saved connection",
        "Edit connection…",
        "Rename…",
        "Move up",
        "Move down",
    ] {
        let ctx = egui::Context::default();
        frame(&ctx, vec![]);
        let (output, _) = frame(&ctx, vec![]);
        let row = text_rect(&output, "Beta");
        let (_, intent) = click(&ctx, row.center(), egui::PointerButton::Secondary);
        assert!(intent.is_none(), "Right-click must not connect");
        frame(&ctx, vec![]);
        let (output, _) = frame(&ctx, vec![]);
        let target = text_rect(&output, menu);
        let (_, intent) = click(&ctx, target.center(), egui::PointerButton::Primary);
        match (menu, intent) {
            ("Remove saved connection", Some(Intent::Change(ProfileListEdit::Remove(profile)))) => {
                assert_eq!(profile.name, "Beta")
            }
            (
                "Edit connection…",
                Some(Intent::Edit {
                    profile,
                    focus_name,
                }),
            ) => {
                assert_eq!(profile.name, "Beta");
                assert!(!focus_name);
            }
            (
                "Rename…",
                Some(Intent::Edit {
                    profile,
                    focus_name,
                }),
            ) => {
                assert_eq!(profile.name, "Beta");
                assert!(focus_name);
            }
            (
                "Move up",
                Some(Intent::Change(ProfileListEdit::Move {
                    label,
                    relative_to,
                    after,
                })),
            ) => {
                assert_eq!(
                    (label.as_str(), relative_to.as_str(), after),
                    ("Beta", "Alpha", false)
                );
            }
            (
                "Move down",
                Some(Intent::Change(ProfileListEdit::Move {
                    label,
                    relative_to,
                    after,
                })),
            ) => {
                assert_eq!(
                    (label.as_str(), relative_to.as_str(), after),
                    ("Beta", "Gamma", true)
                );
            }
            _ => panic!("Wrong action from {menu}"),
        }
    }
}

#[test]
fn saved_connection_drag_reorders_in_both_directions_and_escape_cancels() {
    for (source, target, after, cancel) in [
        ("Alpha", "Gamma", true, false),
        ("Gamma", "Alpha", false, false),
        ("Alpha", "Gamma", true, true),
    ] {
        let ctx = egui::Context::default();
        frame(&ctx, vec![]);
        let (output, _) = frame(&ctx, vec![]);
        let from = text_rect(&output, source).center();
        let to = text_rect(&output, target).center() + egui::vec2(0., if after { 6. } else { -6. });
        let (_, intent) = frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(from),
                pointer(from, egui::PointerButton::Primary, true),
            ],
        );
        assert!(intent.is_none());
        let (_, intent) = frame(
            &ctx,
            vec![egui::Event::PointerMoved(from + egui::vec2(12., 0.))],
        );
        assert!(intent.is_none());
        let (_, intent) = frame(&ctx, vec![egui::Event::PointerMoved(to)]);
        assert!(intent.is_none());
        if cancel {
            frame(
                &ctx,
                vec![egui::Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
            );
        }
        let (_, intent) = frame(&ctx, vec![pointer(to, egui::PointerButton::Primary, false)]);
        if cancel {
            assert!(intent.is_none());
        } else if let Some(Intent::Change(ProfileListEdit::Move {
            label,
            relative_to,
            after: actual_after,
        })) = intent
        {
            assert_eq!(
                (label.as_str(), relative_to.as_str(), actual_after),
                (source, target, after)
            );
        } else {
            panic!("Drag did not produce a reorder");
        }
    }
}

#[test]
fn saved_connection_plain_click_keeps_the_connection_action() {
    let ctx = egui::Context::default();
    frame(&ctx, vec![]);
    let (output, _) = frame(&ctx, vec![]);
    let (_, intent) = click(
        &ctx,
        text_rect(&output, "Alpha").center(),
        egui::PointerButton::Primary,
    );
    assert!(matches!(intent, Some(Intent::Connect(profile)) if profile.name == "Alpha"));
}

#[test]
fn saved_connection_rename_selects_the_name_without_connecting_or_changing_the_host() {
    for size in [egui::vec2(1000., 900.), egui::vec2(620., 600.)] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.connection_profiles.profiles = profiles();
        app.connection_profiles.profiles[1].name = "β Work cluster".into();
        let original = app.connection_profiles.profiles[1].clone();
        app.new_file();
        app.documents[0].text = "unsaved source".into();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        ui_probe::install_at(&ctx, path.clone());
        let frame = |app: &mut EditorApp, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::SidePanel::left("connections").show(ctx, |ui| {
                        app.saved_connections_ui(ui, ctx);
                    });
                    egui::CentralPanel::default().show(ctx, |_| {});
                    app.dialogs(ctx);
                },
            )
        };
        let click = |app: &mut EditorApp, point, button| {
            frame(
                app,
                vec![
                    egui::Event::PointerMoved(point),
                    pointer(point, button, true),
                ],
            );
            frame(app, vec![pointer(point, button, false)]);
        };
        frame(&mut app, vec![]);
        let output = frame(&mut app, vec![]);
        click(
            &mut app,
            text_rect(&output, &original.name).center(),
            egui::PointerButton::Secondary,
        );
        frame(&mut app, vec![]);
        let output = frame(&mut app, vec![]);
        click(
            &mut app,
            text_rect(&output, "Rename…").center(),
            egui::PointerButton::Primary,
        );
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        assert!(app.dialog == Dialog::Connect);
        let id = egui::Id::new(("connection-field", "Connection name"));
        assert!(ctx.memory(|memory| memory.has_focus(id)));
        let selection = egui::TextEdit::load_state(&ctx, id)
            .unwrap()
            .cursor
            .char_range()
            .unwrap();
        assert_eq!(selection.sorted_cursors()[0].index, 0);
        assert_eq!(
            selection.sorted_cursors()[1].index,
            original.name.chars().count()
        );
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let top = |name| {
            state["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .find(|button| button["name"] == name)
                .unwrap()["rect"][1]
                .as_f64()
                .unwrap()
        };
        assert!(top("ssh-field-Connection name") < top("ssh-field-Host / alias"));
        frame(&mut app, vec![egui::Event::Text("Research cluster".into())]);
        assert_eq!(app.connection_profiles.draft.name, "Research cluster");
        assert_eq!(app.connection_profiles.draft.host, original.host);
        assert_eq!(
            app.connection_profiles.baseline.as_ref().unwrap().name,
            original.name
        );
        assert_eq!(
            app.connection_profiles.profiles[1].name, original.name,
            "Renaming waits for Save profile"
        );
        assert_eq!(app.documents[0].text, "unsaved source");
        assert!(app.pending.is_none() && app.ssh_master.is_none() && app.workspace.is_none());
        assert!(!app.connection_profiles.saving && app.connection_profiles.writer.is_none());
        frame(
            &mut app,
            vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert!(app.dialog == Dialog::None);
        // Ordinary Connect still starts in the host field after the rename form closes.
        app.set_dialog(Dialog::Connect);
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        assert!(ctx.memory(|memory| {
            memory.has_focus(egui::Id::new(("connection-field", "Host / alias")))
        }));
    }
}

#[test]
fn saved_connection_results_preserve_drafts_and_prevent_stale_reload() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.connection_profiles.profiles = profiles();
    app.connection_profiles.baseline = Some(app.connection_profiles.profiles[0].clone());
    app.connection_profiles.draft = app.connection_profiles.profiles[0].clone();
    app.connection_profiles.draft.user = "unsaved-login".into();
    app.connection_profiles.saving = true;
    app.set_dialog(Dialog::Connect);
    assert!(
        app.connection_profiles.reload.is_none(),
        "A reload must not race an in-flight edit"
    );
    app.new_file();
    app.documents[0].text = "unsaved source".into();
    app.workers
        .profiles(&ctx)
        .send(ProfileEvent::Changed {
            result: Err("Read-only configuration".into()),
            removed: Some("Alpha".into()),
        })
        .unwrap();
    app.poll(&ctx);
    assert_eq!(app.connection_profiles.profiles.len(), 3);
    assert!(!app.connection_profiles.saving);
    assert!(app.notice.as_ref().unwrap().1);
    app.workers
        .profiles(&ctx)
        .send(ProfileEvent::Changed {
            result: Ok(profiles().into_iter().skip(1).collect()),
            removed: Some("Alpha".into()),
        })
        .unwrap();
    app.poll(&ctx);
    assert_eq!(
        app.connection_profiles
            .profiles
            .iter()
            .map(Profile::label)
            .collect::<Vec<_>>(),
        ["Beta", "Gamma"]
    );
    assert!(app.connection_profiles.baseline.is_none());
    assert_eq!(app.connection_profiles.draft.user, "unsaved-login");
    assert_eq!(app.documents[0].text, "unsaved source");
    assert!(app.workspace.is_none() && app.ssh_master.is_none());
}

#[test]
fn profile_save_survives_workspace_reset_without_claiming_a_newer_forms_identity() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let saved = profiles();
    app.connection_profiles.select(saved[0].clone(), true);
    let submitted_form = app.connection_profiles.form_id;
    app.connection_profiles.saving = true;
    app.connection_profiles.select(saved[1].clone(), true);
    app.connection_profiles.draft.user = "unsaved-login".into();
    app.reset_workspace();
    app.workers
        .profiles(&ctx)
        .send(ProfileEvent::Saved {
            result: Ok(saved.clone()),
            submitted: Box::new(saved[0].clone()),
            form: submitted_form,
        })
        .unwrap();
    app.poll(&ctx);
    assert_eq!(app.connection_profiles.profiles.len(), saved.len());
    assert_eq!(
        app.connection_profiles.baseline.as_ref().unwrap().label(),
        saved[1].label()
    );
    assert_eq!(app.connection_profiles.draft.user, "unsaved-login");
    assert!(!app.connection_profiles.saving());
    assert!(app.workspace.is_none());
    assert!(!app.notice.as_ref().unwrap().1);
}

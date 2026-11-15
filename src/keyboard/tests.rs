use super::*;

fn key(key: Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}
fn frame(ctx: &egui::Context, events: Vec<egui::Event>, enabled: bool, text: &mut String) -> usize {
    let mut clicked = 0;
    let _ = ctx.run(
        egui::RawInput {
            events,
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800., 600.),
            )),
            ..Default::default()
        },
        |ctx| {
            prepare(ctx, Id::new("scope"));
            capture(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                if ui.add_enabled(enabled, Button::new("Apply")).clicked() {
                    clicked += 1;
                }
                ui.add(named(
                    "Name",
                    egui::TextEdit::singleline(text).id(Id::new("name")),
                ));
                button(ui, "After");
            });
            finish(ctx, egui::Color32::BLACK, egui::Color32::WHITE);
        },
    );
    clicked
}

#[test]
fn hints_focus_then_deliver_batched_enter_once() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut text = String::new();
    frame(&ctx, vec![], true, &mut text);
    start(&ctx, false);
    let code = snapshot(&ctx)["hints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "Apply")
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        frame(
            &ctx,
            vec![egui::Event::Text(code), key(Key::Enter)],
            true,
            &mut text
        ),
        0
    );
    assert_eq!(frame(&ctx, vec![], true, &mut text), 1);
    assert_eq!(frame(&ctx, vec![], true, &mut text), 0);
    assert!(text.is_empty());
}

#[test]
fn disappearing_target_discards_pending_activation() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut text = String::new();
    frame(&ctx, vec![], true, &mut text);
    start(&ctx, false);
    let code = snapshot(&ctx)["hints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "Apply")
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        frame(
            &ctx,
            vec![egui::Event::Text(code), key(Key::Enter)],
            false,
            &mut text
        ),
        0
    );
    assert_eq!(frame(&ctx, vec![], true, &mut text), 0);
    assert_eq!(snapshot(&ctx)["mode"], "idle");
}

#[test]
fn hint_codes_follow_layout_without_reassignment() {
    let ctx = egui::Context::default();
    install(&ctx);
    let render = |padding, enabled| {
        let _ = ctx.run(Default::default(), |ctx| {
            prepare(ctx, Id::new("scope"));
            capture(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_space(padding);
                ui.add_enabled(enabled, Button::new("Apply"));
                button(ui, "After");
            });
            finish(ctx, egui::Color32::BLACK, egui::Color32::WHITE);
        });
    };
    render(0., true);
    start(&ctx, false);
    let before = snapshot(&ctx)["hints"][1].clone();
    let catalog = catalog(&ctx).unwrap();
    let old_rect = {
        let windows = catalog.0.lock().unwrap();
        let Mode::Hints { choices, .. } = &windows[&egui::ViewportId::ROOT].mode else {
            panic!("hints not active");
        };
        choices[1].target.rect
    };
    render(70., false);
    assert_eq!(snapshot(&ctx)["hints"], serde_json::json!([before]));
    let windows = catalog.0.lock().unwrap();
    let Mode::Hints { choices, .. } = &windows[&egui::ViewportId::ROOT].mode else {
        panic!("hints not active");
    };
    assert_eq!(choices[0].target.rect.top() - old_rect.top(), 70.);
}

#[test]
fn field_handoff_preserves_case_and_batched_text() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut text = String::new();
    frame(&ctx, vec![], true, &mut text);
    start(&ctx, false);
    let code = snapshot(&ctx)["hints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "Name")
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    frame(
        &ctx,
        vec![egui::Event::Text(format!("{code}ABC"))],
        true,
        &mut text,
    );
    frame(&ctx, vec![], true, &mut text);
    assert_eq!(text, "ABC");
}

#[test]
fn changing_scope_never_replays_hint_input_into_a_new_modal() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut text = String::new();
    frame(&ctx, vec![], true, &mut text);
    start(&ctx, false);
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Text("a".into()), key(Key::Enter)],
            ..Default::default()
        },
        |ctx| {
            prepare(ctx, Id::new("new modal"));
            assert!(ctx.input(|i| i.events.is_empty()));
        },
    );
    assert_eq!(snapshot(&ctx)["mode"], "idle");
}

#[test]
fn fuzzy_ranking_prefers_exact_and_supports_abbreviations() {
    assert!(rank("mdl", "Agent model").is_some());
    assert!(rank("model", "Model").unwrap() < rank("model", "Agent model").unwrap());
    assert_eq!(rank("zzz", "Model"), None);
}

#[test]
fn a_button_group_has_one_tab_stop_and_arrows_do_not_activate_it() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut ids = vec![];
    let mut after = None;
    let mut clicks = 0;
    let mut render = |events| {
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                prepare(ctx, Id::new("scope"));
                ids.clear();
                egui::CentralPanel::default().show(ctx, |ui| {
                    group(ui, "tools", |ui| {
                        ui.horizontal(|ui| {
                            for name in ["First", "Second", "Third"] {
                                let response = button(ui, name);
                                ids.push(response.id);
                                if response.clicked() {
                                    clicks += 1;
                                }
                            }
                        });
                    });
                    after = Some(button(ui, "After").id);
                });
            },
        );
        (ids.clone(), after, clicks)
    };
    let (ids, _, _) = render(vec![]);
    ctx.memory_mut(|m| m.request_focus(ids[0]));
    render(vec![]);
    let (_, _, clicks) = render(vec![key(Key::ArrowRight)]);
    assert_eq!(ctx.memory(|m| m.focused()), Some(ids[1]));
    assert_eq!(clicks, 0);
    let (_, after, _) = render(vec![key(Key::Tab)]);
    assert_eq!(ctx.memory(|m| m.focused()), after);
}

#[test]
fn a_hint_can_activate_a_non_default_group_member_once() {
    let ctx = egui::Context::default();
    install(&ctx);
    let render = |events| {
        let mut activated = Vec::new();
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                prepare(ctx, Id::new("scope"));
                capture(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    group(ui, "tabs", |ui| {
                        ui.horizontal(|ui| {
                            for name in ["First", "Second", "Third"] {
                                if button(ui, name).clicked() {
                                    activated.push(name);
                                }
                            }
                        });
                    });
                });
                finish(ctx, egui::Color32::BLACK, egui::Color32::WHITE);
            },
        );
        activated
    };
    render(vec![]);
    start(&ctx, false);
    let snapshot = snapshot(&ctx);
    let code = snapshot["hints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hint| hint["label"] == "Second")
        .unwrap()["code"]
        .as_str()
        .unwrap();
    assert!(render(vec![egui::Event::Text(code.into()), key(Key::Enter)]).is_empty());
    assert_eq!(render(vec![]), ["Second"]);
    assert!(render(vec![]).is_empty());
}

#[test]
fn keyboard_context_menu_retains_its_anchor_and_owns_its_own_focus() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut trigger = Id::NULL;
    let mut render = |events| {
        let mut visible = false;
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                prepare(ctx, Id::new("scope"));
                egui::CentralPanel::default().show(ctx, |ui| {
                    group(ui, "tabs", |ui| {
                        let response = button(ui, "Tab");
                        trigger = response.id;
                        visible = context_menu(&response, |ui| {
                            let action = button(ui, "Context action");
                            assert!(action.sense.is_focusable());
                        })
                        .is_some();
                    });
                });
            },
        );
        (trigger, visible)
    };
    let (trigger, _) = render(vec![]);
    ctx.memory_mut(|memory| memory.request_focus(trigger));
    render(vec![]);
    let mut open = key(Key::F10);
    if let egui::Event::Key { modifiers, .. } = &mut open {
        *modifiers = egui::Modifiers::SHIFT;
    }
    assert!(render(vec![open]).1);
    assert!(render(vec![]).1);
    let catalog = catalog(&ctx).unwrap();
    let windows = catalog.0.lock().unwrap();
    let action = windows[&egui::ViewportId::ROOT]
        .published
        .as_ref()
        .unwrap()
        .targets
        .iter()
        .find(|target| target.label == "Context action")
        .unwrap();
    assert!(action.group.is_none());
}

#[test]
fn native_viewports_keep_independent_discovery_modes() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut text = String::new();
    frame(&ctx, vec![], true, &mut text);
    start(&ctx, false);
    let child = egui::ViewportId::from_hash_of("child");
    let mut input = egui::RawInput {
        viewport_id: child,
        ..Default::default()
    };
    input.viewports.insert(child, Default::default());
    let _ = ctx.run(input, |ctx| {
        prepare(ctx, Id::new("child scope"));
        assert_eq!(snapshot(ctx)["mode"], "idle");
    });
    frame(&ctx, vec![], true, &mut text);
    assert_eq!(snapshot(&ctx)["mode"], "hints");
    forget(&ctx, child);
    assert!(
        !catalog(&ctx)
            .unwrap()
            .0
            .lock()
            .unwrap()
            .contains_key(&child)
    );
}

#[test]
fn a_batched_tab_after_a_field_hint_uses_native_focus_order() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut text = String::new();
    frame(&ctx, vec![], true, &mut text);
    start(&ctx, false);
    let code = snapshot(&ctx)["hints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "Name")
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    frame(
        &ctx,
        vec![egui::Event::Text(code), key(Key::Tab)],
        true,
        &mut text,
    );
    frame(&ctx, vec![], true, &mut text);
    let catalog = catalog(&ctx).unwrap();
    let windows = catalog.0.lock().unwrap();
    let after = windows[&egui::ViewportId::ROOT]
        .published
        .as_ref()
        .unwrap()
        .targets
        .iter()
        .find(|t| t.label == "After")
        .unwrap();
    assert!(ctx.memory(|m| m.has_focus(after.widget)));
    assert!(text.is_empty());
}

#[test]
fn oversized_handoff_is_discarded_with_a_notice() {
    let mut window = Window::default();
    window.hold(egui::Event::Paste("x".repeat(MAX_EVENT_BYTES + 1)));
    assert!(window.held.is_empty());
    assert!(window.notice.is_some());
    assert!(matches!(window.mode, Mode::Idle));
}

#[test]
fn empty_hosts_cancel_discovery_without_replaying_input_into_later_controls() {
    for search in [false, true] {
        let ctx = egui::Context::default();
        install(&ctx);
        let blank = |events| {
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    prepare(ctx, Id::new("scope"));
                    capture(ctx);
                    finish(ctx, egui::Color32::BLACK, egui::Color32::WHITE);
                },
            );
        };
        blank(vec![]);
        start(&ctx, search);
        blank(vec![egui::Event::Text("stale".into()), key(Key::Enter)]);
        for _ in 0..8 {
            blank(vec![]);
        }
        assert_eq!(snapshot(&ctx)["mode"], "idle");
        assert!(take_notice(&ctx).is_some());
        let mut text = String::new();
        ctx.memory_mut(|memory| memory.request_focus(Id::new("name")));
        assert_eq!(
            frame(
                &ctx,
                vec![egui::Event::Text("later".into())],
                true,
                &mut text
            ),
            0
        );
        assert_eq!(text, "later");
    }
}

#[test]
fn cancellation_keeps_later_typing_behind_the_restored_focus() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut text = String::new();
    frame(&ctx, vec![], true, &mut text);
    ctx.memory_mut(|m| m.request_focus(Id::new("name")));
    frame(&ctx, vec![], true, &mut text);
    start(&ctx, false);
    frame(
        &ctx,
        vec![key(Key::Escape), egui::Event::Text("ABC".into())],
        true,
        &mut text,
    );
    frame(&ctx, vec![], true, &mut text);
    assert_eq!(text, "ABC");
}

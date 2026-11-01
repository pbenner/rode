//! Agent chat layout, input, settings and reply rendering regressions.

use super::*;
use std::collections::BTreeMap;

#[test]
fn long_responses_wrap_to_the_visible_chat_width_after_resizing() {
    for (body, code) in [
        (
            "A long response should remain readable in the chat panel. ".repeat(80),
            false,
        ),
        (
            format!("https://example.invalid/{}", "longpath".repeat(200)),
            false,
        ),
        (format!("    result = '{}'", "value".repeat(200)), true),
    ] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.agents.config.provider = Provider::Acp;
        app.agents
            .chat
            .messages
            .push_back(agent::protocol::Message {
                id: "wrapped-response".into(),
                role: Role::Assistant,
                title: "Agent".into(),
                text: if code {
                    format!("```text\n{body}\n```")
                } else {
                    body.clone()
                },
            });
        for (width, oversized) in [
            (1100., false),
            (500., false),
            (320., false),
            (1100., true),
            (500., true),
        ] {
            let mut output = None;
            for _ in 0..2 {
                output = Some(ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 800.),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            if oversized {
                                ui.set_min_width(width * 2.);
                            }
                            app.agent_ui(ui, ctx);
                        });
                    },
                ));
            }
            let output = output.unwrap();
            let (shape, text) = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == body.trim_end() => {
                        Some((shape, text))
                    }
                    _ => None,
                })
                .expect("rendered response text");
            assert!(text.galley.rows.len() > 1);
            assert!(
                text.pos.x + text.galley.size().x <= shape.clip_rect.right() + 1.,
                "response overruns visible chat: width={width}, right={}, clip={}",
                text.pos.x + text.galley.size().x,
                shape.clip_rect.right()
            );
            assert!(!text.galley.elided);
        }
    }
}

#[test]
fn copy_response_copies_displayed_content_without_code_fences() {
    for (source, expected) in [
        (
            "```text\nCommit title\n\n- change\n```",
            "Commit title\n\n- change",
        ),
        (
            "Intro\n\n```rust\n  let value = `literal`;\n```\n\nOutro",
            "Intro\n\n  let value = `literal`;\n\nOutro",
        ),
    ] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.agents.config.provider = Provider::Acp;
        app.agents
            .chat
            .messages
            .push_back(agent::protocol::Message {
                id: "copy-response-test".into(),
                role: Role::Assistant,
                title: "Agent".into(),
                text: source.into(),
            });
        let mut frame = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 760.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.agent_ui(ui, ctx));
                },
            )
        };
        let output = frame(vec![]);
        let point = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Agent" => Some(
                    text.pos
                        + egui::vec2(text.galley.size().x + 8. + 13., text.galley.size().y / 2.),
                ),
                _ => None,
            })
            .expect("assistant message header");
        let pointer = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(point), pointer(true)]);
        let output = frame(vec![pointer(false)]);
        assert!(
            output.platform_output.commands.iter().any(|command| {
                matches!(command, egui::OutputCommand::CopyText(text) if text == expected)
            }),
            "Copy response did not copy the displayed text"
        );
    }
}

#[test]
fn busy_submission_queues_without_starting_another_turn_and_replacement_clears_it() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.agents.chat.busy = true;
    app.agents.draft = "Follow up after this task".into();
    app.send_agent(&ctx);
    assert_eq!(app.agents.queue.next(), Some("Follow up after this task"));
    assert!(app.agents.draft.is_empty());
    assert!(!app.agents.chat.connected());
    assert!(app.agents.chat.messages.is_empty());
    app.agents.chat.busy = false;
    app.dialog = Dialog::Shortcuts;
    app.dispatch_agent_queue(&ctx);
    assert_eq!(app.agents.queue.next(), Some("Follow up after this task"));
    app.agents.replace_chat();
    assert!(app.agents.queue.is_empty());
}

#[test]
fn failed_queue_submission_keeps_the_prompt_and_draft_and_pauses_dispatch() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.agents.config.codex = "invalid\0executable".into();
    app.agents.queue.push("retained follow-up".into()).unwrap();
    app.agents.draft = "unfinished draft".into();
    app.dispatch_agent_queue(&ctx);
    assert!(!app.agents.queue.is_empty());
    assert!(app.agents.queue.next().is_none());
    assert_eq!(app.agents.draft, "unfinished draft");
    assert!(!app.agents.chat.busy);
    assert!(
        app.agents
            .chat
            .messages
            .iter()
            .any(|message| message.text.contains("Queued prompt was not sent"))
    );
}

#[test]
fn response_selection_copies_across_prose_and_code_without_editing() {
    let ctx = egui::Context::default();
    let source = "**Alpha** *βeta*\n\n```rust\nlet value = 1;\n```\n\nOmega end";
    let frame = |events| {
        let mut output = None;
        let full = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    output = Some(render_message(ui, source, theme::Palette::default()));
                });
            },
        );
        (full, output.unwrap())
    };
    let (_, output) = frame(vec![]);
    let display = output.galley.text().to_owned();
    assert_eq!(display, "Alpha βeta\n\nlet value = 1;\n\nOmega end");
    let start = 6;
    let end = display.chars().count() - 4;
    let position = |index| {
        output.galley_pos
            + output
                .galley
                .pos_from_cursor(egui::text::CCursor::new(index))
                .center()
                .to_vec2()
    };
    let from = position(start);
    let to = position(end);
    let pointer = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![egui::Event::PointerMoved(from), pointer(from, true)]);
    frame(vec![egui::Event::PointerMoved(to)]);
    let (_, output) = frame(vec![pointer(to, false)]);
    assert!(output.response.has_focus());
    assert_eq!(
        output.cursor_range.unwrap().as_sorted_char_range(),
        start..end
    );
    let (full, _) = frame(vec![egui::Event::Copy]);
    let expected: String = display.chars().skip(start).take(end - start).collect();
    assert!(full.platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &expected)
    ));
    let secondary = |pressed| egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![secondary(true)]);
    frame(vec![secondary(false)]);
    let (full, output) = frame(vec![]);
    assert_eq!(
        output
            .state
            .cursor
            .range(&output.galley)
            .unwrap()
            .as_sorted_char_range(),
        start..end
    );
    let copy_position = full
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Copy selection" => {
                Some(text.pos + text.galley.rect.center().to_vec2())
            }
            _ => None,
        })
        .expect("copy selection menu item");
    frame(vec![
        egui::Event::PointerMoved(copy_position),
        pointer(copy_position, true),
    ]);
    let (full, _) = frame(vec![pointer(copy_position, false)]);
    assert!(full.platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &expected)
    ));
    output.response.request_focus();
    let (_, output) = frame(vec![
        egui::Event::Text("replacement".into()),
        egui::Event::Paste("injected".into()),
        egui::Event::Cut,
    ]);
    assert!(output.response.has_focus());
    assert_eq!(output.galley.text(), display);
}

#[test]
fn response_selection_survives_streaming_updates() {
    let ctx = egui::Context::default();
    let frame = |text: &str, events| {
        let mut output = None;
        let full = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    output = Some(render_message(ui, text, theme::Palette::default()));
                });
            },
        );
        (full, output.unwrap())
    };
    let (_, mut output) = frame("Alpha **βeta**", vec![]);
    output
        .state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(6),
            egui::text::CCursor::new(10),
        )));
    output.state.store(&ctx, output.response.id);
    output.response.request_focus();
    let (full, output) = frame(
        "Alpha **βeta**\n\n```rust\nlet value = 1;\n```",
        vec![egui::Event::Copy],
    );
    assert_eq!(output.cursor_range.unwrap().as_sorted_char_range(), 6..10);
    assert!(
        full.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "βeta")
        )
    );
}

#[test]
fn composer_sends_enter_preserves_multiline_text_and_confirms_ime_without_sending() {
    let ctx = egui::Context::default();
    let mut draft = String::new();
    let mut composing = false;
    let id = egui::Id::new("agent-composer");
    let mut frame = |mut events: Vec<egui::Event>, focused: bool, modal: bool| {
        events.push(egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut send = false;
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.memory_mut(|m| m.request_focus(if focused { id } else { id.with("other") }));
                    send = composer_submit(ui, id, modal, &mut composing);
                    ui.add(egui::TextEdit::multiline(&mut draft).id(id).return_key(
                        egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, Key::Enter),
                    ));
                });
            },
        );
        (send, draft.clone())
    };
    let enter = |modifiers, repeat| egui::Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed: true,
        repeat,
        modifiers,
    };
    assert!(!frame(vec![egui::Event::Text("one".into())], true, false).0);
    assert_eq!(
        frame(vec![enter(egui::Modifiers::SHIFT, false)], true, false),
        (false, "one\n".into())
    );
    assert_eq!(
        frame(vec![egui::Event::Paste("two\nthree".into())], true, false),
        (false, "one\ntwo\nthree".into())
    );
    assert_eq!(
        frame(vec![enter(egui::Modifiers::NONE, false)], true, false),
        (true, "one\ntwo\nthree".into())
    );
    // egui derives repeat from its held-key state, not the raw event flag.
    ctx.input_mut(|input| {
        input.keys_down.insert(Key::Enter);
    });
    assert!(!frame(vec![enter(egui::Modifiers::NONE, true)], true, false).0);
    assert!(!frame(vec![enter(egui::Modifiers::NONE, false)], true, true).0);
    assert!(!frame(vec![enter(egui::Modifiers::NONE, false)], false, false).0);
    assert!(!frame(vec![egui::Event::Ime(egui::ImeEvent::Enabled)], true, false).0);
    assert!(
        !frame(
            vec![
                egui::Event::Ime(egui::ImeEvent::Preedit("候".into())),
                enter(egui::Modifiers::NONE, false)
            ],
            true,
            false
        )
        .0
    );
    assert!(
        !frame(
            vec![
                egui::Event::Ime(egui::ImeEvent::Commit("候".into())),
                enter(egui::Modifiers::NONE, false)
            ],
            true,
            false
        )
        .0
    );
    assert!(frame(vec![enter(egui::Modifiers::NONE, false)], true, false).0);
}

#[derive(Default)]
struct Storage(BTreeMap<String, String>);
impl eframe::Storage for Storage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn flush(&mut self) {}
}
#[test]
fn project_preferences_restore_without_persisting_conversation_content() {
    let mut agents = Agents::load(None);
    agents.key = "host /project".into();
    agents.config.provider = Provider::Local;
    agents.config.local_model = "installed-model".into();
    agents.chat.notice("private conversation content");
    agents.save_config();
    let mut storage = Storage::default();
    agents.store(&mut storage);
    let restored = Agents::load(Some(&storage));
    assert_eq!(
        restored.preferences["host /project"].local_model,
        "installed-model"
    );
    assert!(!storage.0["agent_preferences"].contains("private conversation content"));
}

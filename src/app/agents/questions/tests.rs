//! Question layout and explicit answer dispatch, including compact themed panels.
use super::*;

fn prompt() -> Prompt {
    serde_json::from_value(json!({
        "id":401, "title":"Agent needs your input", "summary":"", "detail":"",
        "choices":[], "cancel":{"answers":{}}, "questions":[{
            "id":"tests", "header":"Tests", "text":"Which tests should I run?",
            "options":["All tests"], "descriptions":["Run the full suite before continuing."],
            "answer":"", "secret":false
        }]
    }))
    .unwrap()
}

fn frame(
    ctx: &egui::Context,
    prompt: &mut Prompt,
    events: Vec<egui::Event>,
    enabled: bool,
) -> (egui::FullOutput, Action) {
    let mut action = Action::default();
    let output = ctx.run(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                action = show(ui, prompt, enabled, true, crate::theme::Palette::default());
            });
        },
    );
    (output, action)
}

fn enter(modifiers: egui::Modifiers) -> Vec<egui::Event> {
    [true, false]
        .into_iter()
        .map(|pressed| egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        })
        .collect()
}

#[test]
fn question_enter_sends_shift_enter_keeps_newlines_and_ime_does_not_submit() {
    let ctx = egui::Context::default();
    let mut prompt = prompt();
    frame(&ctx, &mut prompt, vec![], true);
    let id = answer_id(&prompt.id, &prompt.questions[0].id);
    ctx.memory_mut(|m| m.request_focus(id));
    frame(
        &ctx,
        &mut prompt,
        vec![egui::Event::Text("first".into())],
        true,
    );
    assert!(
        frame(&ctx, &mut prompt, enter(egui::Modifiers::SHIFT), true)
            .1
            .answer
            .is_none()
    );
    frame(
        &ctx,
        &mut prompt,
        vec![egui::Event::Text("second".into())],
        true,
    );
    assert_eq!(prompt.questions[0].answer, "first\nsecond");
    let mut ime = vec![egui::Event::Ime(egui::ImeEvent::Commit("字".into()))];
    ime.extend(enter(egui::Modifiers::NONE));
    assert!(frame(&ctx, &mut prompt, ime, true).1.answer.is_none());
    assert!(
        frame(&ctx, &mut prompt, enter(egui::Modifiers::NONE), false)
            .1
            .answer
            .is_none()
    );
    ctx.memory_mut(|m| m.request_focus(id));
    let (_, action) = frame(&ctx, &mut prompt, enter(egui::Modifiers::NONE), true);
    assert_eq!(action.answer, prompt.answers());
    assert!(action.answer.is_some());
}

#[test]
fn one_selected_async_option_enables_send_and_enter_without_answering_another_question() {
    let ctx = egui::Context::default();
    let mut prompt = prompt();
    prompt.partial_answers = true;
    let mut second = prompt.questions[0].clone();
    second.id = "details".into();
    second.text = "Additional details?".into();
    second.options.clear();
    prompt.questions.push(second);
    frame(&ctx, &mut prompt, vec![], true);
    let (output, _) = frame(&ctx, &mut prompt, vec![], true);
    let point = layout::tests::button_frame(&output, "All tests").0.center();
    for pressed in [true, false] {
        let (_, action) = frame(
            &ctx,
            &mut prompt,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            true,
        );
        assert!(
            action.answer.is_none(),
            "Selecting an option submitted it automatically"
        );
    }
    assert!(ctx.memory(|m| m.has_focus(answer_id(&prompt.id, "tests"))));
    let (_, action) = frame(&ctx, &mut prompt, enter(egui::Modifiers::NONE), true);
    assert_eq!(
        action.answer,
        Some(json!({"answers":{"tests":{"answers":["All tests"]}}}))
    );
    prompt.partial_answers = false;
    assert!(
        frame(&ctx, &mut prompt, enter(egui::Modifiers::NONE), true)
            .1
            .answer
            .is_none()
    );
}

#[test]
fn questions_replace_the_composer_without_losing_drafts_and_actions_fit_compact_panels() {
    for theme in ["phosphor", "studio", "studio-neutral", "studio-hacker"] {
        for (width, height, scale) in [(320., 650., 1.), (650., 900., 1.5), (320., 650., 1.5)] {
            for working in [true, false] {
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
                app.agents.config.provider = Provider::Acp;
                app.agents.chat.busy = working;
                let mut question = prompt();
                question.blocking = working;
                app.agents.chat.prompts.push_back(question);
                app.agents.draft = "Keep my unsent draft".into();
                app.agents.queue.push("Queued follow-up".into()).unwrap();
                app.agents.history_open = true;
                let mut render = || {
                    ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, height),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| app.agent_ui(ui, ctx));
                        },
                    )
                };
                render();
                let output = render();
                for label in [
                    "Send answers",
                    if working { "Stop" } else { "Dismiss question" },
                ] {
                    let (rect, clip) = layout::tests::button_frame(&output, label);
                    assert!(
                        clip.contains_rect(rect),
                        "{label}: {theme}, {width}, {scale}: {rect:?} vs {clip:?}"
                    );
                }
                assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Queue")));
                assert_eq!(app.agents.draft, "Keep my unsent draft");
                assert_eq!(app.agents.queue.next(), Some("Queued follow-up"));
                assert!(!app.agents.history_open);
                assert_eq!(app.agents.chat.busy, working);
            }
        }
    }
}

#[test]
fn an_idle_unconfirmed_answer_stays_dismissible_without_resubmission() {
    let ctx = egui::Context::default();
    let mut prompt = prompt();
    prompt.blocking = false;
    prompt.sending = true;
    prompt.error = "The answer acknowledgement timed out".into();
    let render = |prompt: &mut Prompt, events| {
        let mut action = Action::default();
        let output = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    action = show(ui, prompt, true, false, crate::theme::Palette::default());
                });
            },
        );
        (output, action)
    };
    render(&mut prompt, vec![]);
    let (output, _) = render(&mut prompt, vec![]);
    let dismiss = layout::tests::button_frame(&output, "Dismiss question")
        .0
        .center();
    for pressed in [true, false] {
        let (_, action) = render(
            &mut prompt,
            vec![
                egui::Event::PointerMoved(dismiss),
                egui::Event::PointerButton {
                    pos: dismiss,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert!(action.answer.is_none());
        if !pressed {
            assert!(action.stop, "An unconfirmed idle answer trapped the form");
        }
    }
}

#[test]
fn choosing_an_option_requires_an_explicit_send_and_free_text_is_returned_verbatim() {
    let ctx = egui::Context::default();
    let mut prompt = prompt();
    let render = |prompt: &mut Prompt, events| {
        let mut action = Action::default();
        let output = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    action = show(ui, prompt, true, true, crate::theme::Palette::default());
                });
            },
        );
        (output, action)
    };
    render(&mut prompt, vec![]);
    let (output, action) = render(&mut prompt, vec![]);
    assert!(action.answer.is_none());
    let option = layout::tests::button_frame(&output, "All tests").0.center();
    let click = |pos, pressed| {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    render(&mut prompt, click(option, true));
    let (output, action) = render(&mut prompt, click(option, false));
    assert!(
        action.answer.is_none(),
        "Choosing an option must not submit it"
    );
    assert_eq!(prompt.questions[0].answer, "All tests");
    prompt.questions[0].answer = "Only tests for the changed files".into();
    let send = layout::tests::button_frame(&output, "Send answers")
        .0
        .center();
    render(&mut prompt, click(send, true));
    let (_, action) = render(&mut prompt, click(send, false));
    assert_eq!(
        action.answer,
        Some(json!({"answers":{"tests":{"answers":["Only tests for the changed files"]}}}))
    );
}

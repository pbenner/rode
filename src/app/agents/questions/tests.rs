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

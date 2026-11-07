//! Explicit answers to app-server questions. The form replaces the composer
//! temporarily; drafts, attachments and queued turns remain owned by the chat.
use super::*;
use crate::agent::protocol::Prompt;

/// Deferred actions keep rendering separate from transport and cancellation.
#[derive(Default)]
pub(super) struct Action {
    pub answer: Option<serde_json::Value>,
    pub stop: bool,
}

/// Fill the available chat space, keeping the actions outside the scroll area.
/// A non-blocking question must not claim that the agent has stopped working.
pub(super) fn show(
    ui: &mut egui::Ui,
    prompt: &mut Prompt,
    enabled: bool,
    working: bool,
    palette: crate::theme::Palette,
) -> Action {
    let mut action = Action::default();
    ui.label(RichText::new(&prompt.title).strong().color(palette.accent));
    let state = ui.add(
        egui::Label::new(if prompt.blocking {
            "The agent is waiting for your answer. Your message draft and queued work are kept."
        } else if working {
            "The agent can continue independent work while you answer. Your message draft is kept."
        } else {
            "The agent has finished this turn. Send your answer to continue the conversation. Your message draft is kept."
        })
        .wrap(),
    );
    ui_probe::button(
        ui,
        if working {
            "agent-question-working"
        } else {
            "agent-question-idle"
        },
        &state,
    );
    let footer = crate::app::layout::control_height(ui) + 2. * ui.spacing().item_spacing.y;
    egui::ScrollArea::vertical()
        .id_salt(("agent-questions", prompt.id.to_string()))
        .auto_shrink([false, false])
        .max_height((ui.available_height() - footer).max(1.))
        .show(ui, |ui| {
            if !prompt.error.is_empty() {
                ui.add(egui::Label::new(RichText::new(&prompt.error).color(palette.error)).wrap());
            }
            ui.add_enabled_ui(enabled && !prompt.sending, |ui| {
                for (index, question) in prompt.questions.iter_mut().enumerate() {
                    ui.push_id((&prompt.id.to_string(), &question.id), |ui| {
                        egui::Frame::new()
                            .fill(palette.raised)
                            .inner_margin(10)
                            .corner_radius(8)
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                if !question.header.is_empty() {
                                    let response = ui.label(
                                        RichText::new(&question.header)
                                            .small()
                                            .color(palette.muted),
                                    );
                                    ui_probe::button(
                                        ui,
                                        &format!("agent-question-{index}-header"),
                                        &response,
                                    );
                                }
                                ui.add(
                                    egui::Label::new(RichText::new(&question.text).strong()).wrap(),
                                );
                                for (option_index, option) in question.options.iter().enumerate() {
                                    let selected = question.answer == *option;
                                    let response =
                                        ui.add(egui::Button::new(option).selected(selected).wrap());
                                    ui_probe::button(
                                        ui,
                                        &format!("agent-question-{index}-option-{option_index}"),
                                        &response,
                                    );
                                    if response.clicked() {
                                        question.answer = option.clone();
                                    }
                                    if let Some(description) = question
                                        .descriptions
                                        .get(option_index)
                                        .filter(|s| !s.is_empty())
                                    {
                                        let response = ui.add(
                                            egui::Label::new(
                                                RichText::new(description)
                                                    .small()
                                                    .color(palette.muted),
                                            )
                                            .wrap(),
                                        );
                                        ui_probe::button(
                                            ui,
                                            &format!(
                                                "agent-question-{index}-description-{option_index}"
                                            ),
                                            &response,
                                        );
                                    }
                                }
                                ui.label(if question.options.is_empty() {
                                    "Your answer"
                                } else {
                                    "Choose an option or write your own answer"
                                });
                                let editor = egui::TextEdit::singleline(&mut question.answer)
                                    .password(question.secret)
                                    .desired_width(f32::INFINITY)
                                    .char_limit(8192);
                                let response = ui.add(editor);
                                ui_probe::button(
                                    ui,
                                    &format!("agent-question-{index}-answer"),
                                    &response,
                                );
                            });
                        ui.add_space(6.);
                    });
                }
            });
        });
    ui.horizontal_wrapped(|ui| {
        let result = prompt.answers();
        let send = ui
            .add_enabled(
                enabled && !prompt.sending && result.is_some(),
                egui::Button::new(if prompt.sending {
                    "Sending answers…"
                } else {
                    "Send answers"
                }),
            )
            .on_disabled_hover_text(if prompt.sending {
                "Waiting for the agent to acknowledge your answer."
            } else {
                "Answer every question before sending."
            });
        ui_probe::button(ui, "agent-question-send", &send);
        if send.clicked() {
            action.answer = result;
        }
        let stop = ui
            .add_enabled(
                enabled && (working || !prompt.sending || !prompt.error.is_empty()),
                egui::Button::new(if working { "Stop" } else { "Dismiss question" }),
            )
            .on_hover_text(if working {
                "Cancel the questions and stop the current turn."
            } else {
                "Dismiss this question without sending an answer."
            });
        ui_probe::button(ui, "agent-question-stop", &stop);
        action.stop = stop.clicked();
    });
    action
}

#[cfg(test)]
mod tests;

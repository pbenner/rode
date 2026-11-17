//! Compact tool presentation over the original ordered, bounded transcript.
//! Completed output remains inspectable; this module never removes messages or
//! changes controller state, permissions or saved backend history.
//!
//! Consecutive successful tools share a disclosure; prose, active tools and
//! failures keep their original positions. The first item's stable identity
//! owns the disclosure state, so changing its count does not reset inspection.
//! Missing lifecycle metadata is treated conservatively until the turn is idle.
use super::*;
use crate::agent::protocol::{Message, ToolState};
use std::collections::VecDeque;

/// Whether a row may join a completed-activity group. Never infer completion
/// from message ordering, a command title or output that happens to say "done".
fn completed(message: &Message, busy: bool) -> bool {
    message.role == Role::Tool
        && match message.tool_state {
            Some(ToolState::Completed) => true,
            // Older helpers cannot identify individual completed commands. Keep
            // their tool rows while working rather than guessing from title text.
            None => !busy,
            _ => false,
        }
}

/// Render compact groups, or every tool row when `keep_completed` is true.
/// Only egui's disclosure/selection state changes; the borrowed transcript and
/// its retention limits remain under the controller's ownership.
pub(super) fn show(
    ui: &mut egui::Ui,
    messages: &VecDeque<Message>,
    busy: bool,
    keep_completed: bool,
    palette: theme::Palette,
) {
    let mut index = 0;
    while let Some(message) = messages.get(index) {
        if !keep_completed && completed(message, busy) {
            let first = index;
            while messages
                .get(index)
                .is_some_and(|message| completed(message, busy))
            {
                index += 1;
            }
            let count = index - first;
            let header = crate::keyboard::CollapsingHeader::new(
                palette.tag(format!("Completed activity · {count}")),
            )
            .id_salt(("completed-activity", &message.id))
            .show(ui, |ui| {
                for message in messages.range(first..index) {
                    show_message(ui, message, palette);
                }
            });
            ui_probe::button(ui, "agent-completed-activity", &header.header_response);
            ui.add_space(5.);
        } else {
            show_message(ui, message, palette);
            index += 1;
        }
    }
}

/// Share speaker/tool styling, selectable output and copy actions between
/// grouped and standalone rows. Scope child controls by message ID rather than
/// a changing row index or tool label.
fn show_message(ui: &mut egui::Ui, message: &Message, palette: theme::Palette) {
    ui.push_id(&message.id, |ui| {
        if message.role == Role::Tool {
            crate::keyboard::CollapsingHeader::new(
                RichText::new(&message.title).monospace().size(12.).color(
                    if message.tool_state == Some(ToolState::Failed) {
                        palette.error
                    } else {
                        palette.muted
                    },
                ),
            )
            .id_salt("tool-output")
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(RichText::new(&message.text).monospace().size(11.))
                        .wrap()
                        .selectable(true),
                );
            });
        } else {
            egui::Frame::new()
                .fill(if message.role == Role::User {
                    palette.raised
                } else {
                    palette.background
                })
                .inner_margin(8)
                .corner_radius(4)
                .show(ui, |ui| {
                    let copy = ui
                        .horizontal(|ui| {
                            ui.label(RichText::new(&message.title).strong().size(11.).color(
                                if message.role == Role::Notice {
                                    palette.muted
                                } else {
                                    palette.accent
                                },
                            ));
                            if message.role == Role::Assistant {
                                let button = icons::small(ui, Icon::Copy, "Copy response");
                                if message.text.len() <= 64 * 1024 {
                                    ui_probe::value(
                                        ui,
                                        "agent-copy-response",
                                        &button,
                                        &message.text,
                                    );
                                }
                                button.clicked()
                            } else {
                                false
                            }
                        })
                        .inner;
                    let output = render_message(ui, &message.text, palette, copy);
                    if message.role == Role::Assistant && message.text.len() <= 64 * 1024 {
                        ui_probe::value(ui, "agent-response", &output.response, &message.text);
                    }
                });
        }
    });
    ui.add_space(5.);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_activity_is_recoverable_and_running_or_failed_tools_stay_visible() {
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.);
        let mut messages: VecDeque<_> = [
            ("finished-one", Role::Tool, Some(ToolState::Completed)),
            ("finished-two", Role::Tool, Some(ToolState::Completed)),
            ("still-running", Role::Tool, Some(ToolState::Running)),
            ("failed-command", Role::Tool, Some(ToolState::Failed)),
            ("conversation", Role::Assistant, None),
        ]
        .into_iter()
        .map(|(title, role, tool_state)| Message {
            id: title.into(),
            title: title.into(),
            role,
            tool_state,
            text: "Retained output".into(),
        })
        .collect();
        let snapshot = messages.clone();
        let frame = |ctx: &egui::Context, messages: &VecDeque<Message>, keep, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(320., 700.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        show(ui, messages, true, keep, theme::Theme::phosphor().palette);
                    });
                },
            )
        };
        let has = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == label)
        })
        };
        frame(&ctx, &messages, false, vec![]);
        let output = frame(&ctx, &messages, false, vec![]);
        assert!(has(&output, "Completed activity · 2"));
        assert!(!has(&output, "finished-one"));
        assert!(has(&output, "still-running"));
        assert!(has(&output, "failed-command"));
        assert!(has(&output, "conversation"));
        let target = layout::tests::text_bounds(&output, "Completed activity · 2").center();
        frame(
            &ctx,
            &messages,
            false,
            vec![egui::Event::PointerMoved(target)],
        );
        for pressed in [true, false] {
            frame(
                &ctx,
                &messages,
                false,
                vec![egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        let output = frame(&ctx, &messages, false, vec![]);
        assert!(has(&output, "finished-one"));
        assert!(has(&output, "finished-two"));
        let output = frame(&ctx, &messages, true, vec![]);
        assert!(!has(&output, "Completed activity · 2"));
        assert!(has(&output, "finished-one"));
        assert_eq!(
            messages, snapshot,
            "Display changes must not edit the transcript"
        );

        // Older helpers cannot distinguish an earlier turn from a live tool
        // followed by an asynchronous user answer. Keep unknown tools visible
        // while busy rather than inferring completion from message order.
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.);
        messages[0].tool_state = None;
        messages[1].role = Role::User;
        messages[2].tool_state = None;
        let output = frame(&ctx, &messages, false, vec![]);
        assert!(has(&output, "finished-one"));
        assert!(has(&output, "still-running"));
    }
}

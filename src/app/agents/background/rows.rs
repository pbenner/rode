//! Uniform live-agent cards. Long metadata stays on one line with full tooltips;
//! action columns are fixed, with a separate action row in narrow windows.
use crate::theme::Palette;
use eframe::egui::{self, Align, Layout, RichText};
use rode_agent_core::service::Info;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Selected live-session intent; the owner performs attachment or confirmed termination.
pub(super) enum Action {
    Show,
    Attach,
    End,
}

/// Current chat and host-policy identity used to explain whether switching attachments is allowed.
pub(super) struct Context<'a> {
    pub current: Option<&'a str>,
    pub root: &'a str,
    pub policy: &'a str,
    pub mode: &'a str,
    pub can_leave: bool,
}

/// Derived action eligibility and its complete user-visible explanation for one live session.
struct Attachment {
    current: bool,
    enabled: bool,
    summary: &'static str,
    explanation: String,
}

impl Context<'_> {
    /// Check project, policy, controlling-lease and current-turn constraints before enabling Attach.
    fn attachment(&self, info: &Info) -> Attachment {
        let current = self.current == Some(info.id.as_str());
        let compatible = info.root == self.root && info.policy == self.policy;
        let enabled = current || (compatible && !info.attached && self.can_leave);
        let summary = if current {
            "Open in this window"
        } else if enabled {
            "Ready to attach"
        } else if info.root != self.root {
            "Different project"
        } else if info.policy != self.policy {
            "Different sandbox settings"
        } else if info.attached {
            "Controlled by another window"
        } else {
            "Finish or persist your current chat to switch"
        };
        let mut reasons = Vec::new();
        if !enabled {
            if info.root != self.root {
                reasons.push("Different project: open the agent's project shown above with its original sandbox settings.".to_owned());
            } else if info.policy != self.policy {
                if info.mode != self.mode {
                    let action = if info.mode.starts_with("NORMAL MODE") {
                        "Reopen this project with sandboxing off."
                    } else {
                        "Reopen this project with sandboxing on and its original access settings."
                    };
                    reasons.push(format!("Different sandbox mode: this agent uses {}, but this window uses {}. {action} A running agent cannot move into or out of a sandbox.", info.mode, self.mode));
                } else {
                    reasons.push("Different sandbox settings: the mode matches, but the original access policy does not. Restore the original folder access, read-only and network settings in View → Project settings / sandbox access.".to_owned());
                }
            }
            if info.attached {
                reasons.push("Controlled by another RODE window: detach it there before attaching here. Hiding the chat panel does not detach it.".to_owned());
            }
            if !self.can_leave {
                reasons.push("Your current chat is working with Persist off. Enable Persist or stop its current turn before switching to this agent.".to_owned());
            }
        }
        Attachment {
            current,
            enabled,
            summary,
            explanation: reasons.join("\n\n"),
        }
    }
}

/// Render equal-height session rows and return an action paired with its stable service ID.
pub(super) fn show(
    ui: &mut egui::Ui,
    items: &[Info],
    context: &Context<'_>,
    palette: Palette,
) -> Option<(String, Action)> {
    let mut selected = None;
    for info in items {
        let (_, action) = ui
            .push_id(&info.id, |ui| row(ui, info, context, palette))
            .inner;
        if let Some(action) = action {
            selected = Some((info.id.clone(), action));
        }
        ui.add_space(8.);
    }
    selected
}

/// Render one bounded metadata card with aligned actions, moving actions below text at narrow widths.
fn row(
    ui: &mut egui::Ui,
    info: &Info,
    context: &Context<'_>,
    palette: Palette,
) -> (egui::Response, Option<Action>) {
    let attachment = context.attachment(info);
    let narrow = ui.available_width() < 540.;
    let mut action = None;
    let response = egui::Frame::new()
        .fill(if attachment.current {
            palette.active
        } else {
            palette.panel
        })
        .stroke(egui::Stroke::new(
            1_f32,
            if attachment.current {
                palette.accent
            } else {
                palette.border
            },
        ))
        .corner_radius(6)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(if narrow { 166. } else { 132. });
            ui.spacing_mut().item_spacing = egui::vec2(8., 4.);
            let top = ui.cursor().top();
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 28.),
                Layout::right_to_left(Align::Center),
                |ui| {
                    if !narrow {
                        action = buttons(ui, &attachment, palette);
                    }
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 28.),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(single_line(&info.name)).size(14.).strong(),
                                )
                                .truncate()
                                .show_tooltip_when_elided(false),
                            )
                            .on_hover_text(&info.name);
                        },
                    );
                },
            );
            let metadata = format!(
                "{} · {} · {}",
                info.provider.label(),
                if info.persistent {
                    "Persistent"
                } else {
                    "Until view closes"
                },
                if !info.alive {
                    "Stopped"
                } else if info.busy {
                    "Working"
                } else {
                    "Idle"
                }
            );
            ui.add(
                egui::Label::new(RichText::new(&metadata).size(12.).color(palette.muted))
                    .truncate()
                    .show_tooltip_when_elided(false),
            )
            .on_hover_text(&metadata);
            field(ui, "PROJECT", &info.root, palette);
            field(ui, "MODE", &info.mode, palette);
            field(ui, "STATUS", &info.status, palette);
            ui.add(
                egui::Label::new(RichText::new(attachment.summary).size(12.).color(
                    if attachment.enabled {
                        palette.accent
                    } else {
                        palette.muted
                    },
                ))
                .truncate()
                .show_tooltip_when_elided(false),
            )
            .on_hover_text(if attachment.explanation.is_empty() {
                attachment.summary
            } else {
                &attachment.explanation
            });
            if narrow {
                ui.add_space((166. - (ui.cursor().top() - top) - 28.).max(0.));
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 28.),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        action = buttons(ui, &attachment, palette).or(action);
                    },
                );
            }
        })
        .response;
    (response, action)
}

/// Right-to-left ordering keeps End at the outer edge and the primary action aligned.
fn buttons(ui: &mut egui::Ui, attachment: &Attachment, palette: Palette) -> Option<Action> {
    let mut action = None;
    if ui
        .add_sized(
            [60., 28.],
            egui::Button::new(RichText::new("End").color(palette.error)),
        )
        .on_hover_text("Stop this agent and its tools after confirmation")
        .clicked()
    {
        action = Some(Action::End);
    }
    if ui
        .add_enabled_ui(attachment.enabled, |ui| {
            ui.add_sized(
                [104., 28.],
                egui::Button::new(if attachment.current {
                    "Show chat"
                } else {
                    "Attach"
                }),
            )
        })
        .inner
        .on_disabled_hover_text(&attachment.explanation)
        .clicked()
    {
        action = Some(if attachment.current {
            Action::Show
        } else {
            Action::Attach
        });
    }
    action
}

fn field(ui: &mut egui::Ui, label: &str, value: &str, palette: Palette) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(54., 18.),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(54.);
                ui.label(palette.tag(label));
            },
        );
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 18.),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.add(
                    egui::Label::new(RichText::new(single_line(value)).size(12.))
                        .truncate()
                        .show_tooltip_when_elided(false),
                )
                .on_hover_text(value);
            },
        );
    });
}

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

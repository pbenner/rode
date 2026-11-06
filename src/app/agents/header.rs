//! Responsive chat header. Reserve the actual themed action widths before sizing the provider.
use super::*;

#[derive(Default)]
pub(super) struct Actions {
    pub provider_changed: bool,
    pub settings: bool,
    pub new_chat: bool,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    config: &mut Config,
    busy: bool,
    queued: bool,
    offline: bool,
    new_chat_enabled: bool,
) -> Actions {
    let mut result = Actions::default();
    let button_width = |label: &str| {
        ui.painter()
            .layout_no_wrap(
                label.into(),
                egui::TextStyle::Button.resolve(ui.style()),
                ui.visuals().text_color(),
            )
            .size()
            .x
            .ceil()
            + 2. * ui.spacing().button_padding.x
    };
    // Settings is a sized icon button. Leave paint room and round measured text
    // widths up so fractional display scales cannot shave off the final corner.
    let reserved = 26.
        + button_width("New chat")
        + button_width(config.personality.button())
        + 3. * ui.spacing().item_spacing.x
        + 2. * ui.visuals().clip_rect_margin;
    let width = ui.available_width();
    if width >= reserved + 80. {
        ui.horizontal(|ui| {
            result.provider_changed = provider(ui, config, width - reserved, busy, queued);
            actions(ui, config, busy, offline, new_chat_enabled, &mut result);
        });
    } else {
        ui.horizontal(|ui| {
            result.provider_changed = provider(ui, config, width, busy, queued);
        });
        ui.horizontal_wrapped(|ui| {
            actions(ui, config, busy, offline, new_chat_enabled, &mut result)
        });
    }
    result
}

fn provider(ui: &mut egui::Ui, config: &mut Config, width: f32, busy: bool, queued: bool) -> bool {
    let old = config.provider;
    // ComboBox::width is a minimum. Constrain the child as well so a long
    // selected provider label truncates instead of growing the entire header.
    ui.allocate_ui_with_layout(
        egui::vec2(width, layout::control_height(ui)),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(width);
            egui::ComboBox::from_id_salt("agent-provider")
                .width(width)
                .truncate()
                .selected_text(config.provider.label())
                .show_ui(ui, |ui| {
                    for provider in Provider::ALL {
                        let enabled = !busy
                            && !queued
                            && !(provider.is_acp() && config.native_tools());
                        ui.add_enabled_ui(enabled, |ui| {
                            ui.selectable_value(&mut config.provider, provider, provider.label())
                        })
                        .response
                        .on_disabled_hover_text("File-only and model-connection-only tools require Codex. Change those access/network settings before switching to Claude Code or ACP.");
                    }
                })
                .response
                .on_hover_text(config.provider.label());
        },
    );
    old != config.provider
}

fn actions(
    ui: &mut egui::Ui,
    config: &mut Config,
    busy: bool,
    offline: bool,
    new_chat_enabled: bool,
    result: &mut Actions,
) {
    let settings = icons::small(ui, Icon::Settings, "Agent settings");
    ui_probe::button(ui, "agent-settings", &settings);
    result.settings = settings.clicked();
    let new_chat = ui
        .add_enabled(new_chat_enabled, egui::Button::new("New chat"))
        .on_hover_text("Start a fresh conversation")
        .on_disabled_hover_text(
            "Wait for the agent to become idle. Use Stop to interrupt active work.",
        );
    ui_probe::button(ui, "agent-new-chat", &new_chat);
    result.new_chat = new_chat.clicked();
    controls::personality(ui, config, !busy && !offline);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_chat_stays_visible_and_only_accepts_clicks_when_idle() {
        for width in [1100., 320.] {
            for (busy, enabled) in [(true, false), (false, false), (false, true)] {
                let ctx = egui::Context::default();
                let mut config = Config::default();
                let mut frame = |events| {
                    let mut clicked = false;
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 400.),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                clicked =
                                    show(ui, &mut config, busy, false, false, enabled).new_chat;
                            });
                        },
                    );
                    (output, clicked)
                };
                frame(vec![]);
                let (output, _) = frame(vec![]);
                let (rect, clip) = layout::tests::button_frame(&output, "New chat");
                assert!(clip.contains_rect(rect));
                let mut clicked = false;
                for pressed in [true, false] {
                    clicked |= frame(vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ])
                    .1;
                }
                assert_eq!(clicked, enabled, "busy={busy}, width={width}");
            }
        }
    }
}

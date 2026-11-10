//! Visible recovery keeps local drafts and host-owned queued prompts intact.
use super::*;

/// Render recovery choices without starting a backend or sending a prompt.
/// Return `Some(true)` for a fresh context, `Some(false)` for resume, or `None` for no action.
pub(super) fn ui(ui: &mut egui::Ui, chat: &Chat, enabled: bool, config: &Config) -> Option<bool> {
    if !chat.recoverable() {
        return None;
    }
    let mut action = None;
    ui.group(|ui| {
        ui.strong(if chat.busy { "Recovering agent…" } else { "Agent connection stopped" });
        ui.add(egui::Label::new("Your draft and queued prompts are kept. Recovery does not repeat the interrupted prompt; resume the queue when ready.").wrap());
        let fresh_required = config.native_tools() && chat.conversation_id().is_some();
        if fresh_required {
            ui.label("Choose Start new context in Recovery options to recreate confined native tools.");
        }
        ui.horizontal_wrapped(|ui| {
            let ready = enabled && !chat.busy && !chat.stopping() && !chat.queue_pending();
            if ui.add_enabled(ready && !fresh_required, egui::Button::new("Recover agent")).on_hover_text(
                "Reopen the backend and reconnect its saved conversation. No prompt is sent.",
            ).clicked() { action = Some(false); }
            egui::containers::menu::MenuButton::from_button(egui::Button::new("Recovery options"))
                .ui(ui, |ui| {
                    if ui.add_enabled(ready, egui::Button::new("Start new context · keep queue")).on_hover_text(
                        "Use when the saved conversation cannot be resumed. Earlier messages stay visible, but are not sent to the new agent. Queued prompts and attachments are kept.",
                    ).clicked() { action = Some(true); ui.close(); }
                    if config.native_tools() {
                        ui.label("Native-tool recovery uses a new context so confined tools are recreated.");
                    }
                });
        });
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2, Rect};

    #[test]
    fn recovery_actions_are_clickable_and_offer_a_fresh_context_for_file_only() {
        for files_only in [false, true] {
            let ctx = egui::Context::default();
            let mut chat = Chat::default();
            let config = Config {
                access: if files_only {
                    agent::Access::FilesOnly
                } else {
                    agent::Access::Project
                },
                ..Default::default()
            };
            chat.apply_view(rode_agent_core::service::ChatView {
                conversation: "saved-thread".into(),
                recoverable: true,
                config: config.clone(),
                ..Default::default()
            });
            let frame = |events| {
                let mut action = None;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(420., 480.))),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            action = super::ui(ui, &chat, true, &config);
                        });
                    },
                );
                (output, action)
            };
            let click = |pos| {
                let mut action = None;
                for pressed in [true, false] {
                    let result = frame(vec![
                        Event::PointerMoved(pos),
                        Event::PointerButton {
                            pos,
                            button: PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]);
                    action = result.1.or(action);
                }
                action
            };
            frame(vec![]);
            let output = frame(vec![]).0;
            let recover = layout::tests::button_frame(&output, "Recover agent").0;
            assert_eq!(
                click(recover.center()),
                if files_only { None } else { Some(false) }
            );
            let options = layout::tests::button_frame(&frame(vec![]).0, "Recovery options").0;
            assert!(click(options.center()).is_none());
            let output = frame(vec![]).0;
            let fresh = layout::tests::button_frame(&output, "Start new context · keep queue").0;
            assert_eq!(click(fresh.center()), Some(true));
            assert!(
                chat.recoverable(),
                "Rendering actions must not discard failed state"
            );
        }
    }
}

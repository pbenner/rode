//! Compact native model/permission popovers shared by the chat providers.
//! Provider metadata is read from the already-started chat; drawing these
//! controls never probes an executable, a remote host or a model server.

use crate::agent::{Access, Approval, Config, Provider, controls::Control, protocol::Chat};
use eframe::egui;
use serde_json::Value;

pub(super) fn buttons(
    ui: &mut egui::Ui,
    config: &mut Config,
    chat: &Chat,
    read_only: bool,
    isolated: bool,
) {
    let width = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.).max(60.);
    let model = if config.provider == Provider::Acp {
        chat.controls
            .iter()
            .find(|c| c.category == "model" && chat.controls_match(config))
            .and_then(|c| config.acp_options.get(&c.id).unwrap_or(&c.current).as_str())
            .unwrap_or("agent")
    } else if config.model().is_empty() {
        "default"
    } else {
        config.model()
    };
    let label = format!("Model: {model}");
    menu(ui, &label, width, |ui| {
        ui.set_min_width(260.);
        ui.set_max_width(360.);
        ui.label(egui::RichText::new("Model and reasoning").strong());
        if config.provider == Provider::Acp {
            acp(ui, config, chat, false);
        } else {
            let old = config.model().to_owned();
            let value = if config.provider == Provider::Local {
                &mut config.local_model
            } else {
                &mut config.model
            };
            ui.label(if config.provider == Provider::Local {
                "Installed model ID"
            } else {
                "Model ID (blank uses the agent default)"
            });
            ui.add(
                egui::TextEdit::singleline(value)
                    .desired_width(f32::INFINITY)
                    .char_limit(256),
            );
            if config.provider == Provider::Codex {
                if chat.controls_match(config) && !chat.models.is_empty() {
                    egui::ScrollArea::vertical()
                        .id_salt("codex-model-picker")
                        .max_height(180.)
                        .show(ui, |ui| {
                            for model in &chat.models {
                                if ui
                                    .selectable_label(
                                        config.model == model.value.id,
                                        &model.value.name,
                                    )
                                    .on_hover_text(&model.value.description)
                                    .clicked()
                                {
                                    config.model.clone_from(&model.value.id);
                                }
                            }
                        });
                } else {
                    ui.small("Model choices load after the first message. A custom ID can be entered now.");
                }
            } else {
                ui.small("Enter a model installed on the configured local server.");
            }
            if old != config.model() {
                config.reasoning_effort.clear();
            }
            ui.separator();
            ui.label("Reasoning effort");
            let advertised = chat.controls_match(config)
                && config.provider == Provider::Codex
                && chat.models.iter().any(|m| {
                    (m.value.id == config.model() || (config.model().is_empty() && m.default))
                        && !m.efforts.is_empty()
                });
            if advertised {
                egui::ComboBox::from_id_salt("agent-reasoning")
                    .selected_text(if config.reasoning_effort.is_empty() {
                        "Default"
                    } else {
                        &config.reasoning_effort
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut config.reasoning_effort, String::new(), "Default");
                        if chat.controls_match(config) {
                            let model = chat
                                .models
                                .iter()
                                .find(|m| m.value.id == config.model())
                                .or_else(|| {
                                    config
                                        .model()
                                        .is_empty()
                                        .then(|| chat.models.iter().find(|m| m.default))
                                        .flatten()
                                });
                            if let Some(model) = model {
                                for effort in &model.efforts {
                                    ui.selectable_value(
                                        &mut config.reasoning_effort,
                                        effort.id.clone(),
                                        &effort.name,
                                    )
                                    .on_hover_text(&effort.description);
                                }
                            }
                        }
                    });
            } else {
                ui.add(
                    egui::TextEdit::singleline(&mut config.reasoning_effort)
                        .desired_width(f32::INFINITY)
                        .hint_text("Blank: default; or an effort ID supported by your model")
                        .char_limit(256),
                );
                ui.small(
                    "Effort IDs depend on the model. Advertised choices appear when available.",
                );
            }
        }
        ui.separator();
        ui.small("Changes apply before the next message.");
    });

    let access = if read_only {
        Access::ReadOnly
    } else {
        config.access()
    };
    let label = if config.provider == Provider::Acp {
        "Permissions".into()
    } else {
        format!("Access: {}", access.label())
    };
    menu(ui, &label, width, |ui| {
        ui.set_min_width(260.);
        ui.set_max_width(360.);
        ui.label(egui::RichText::new("Agent permissions").strong());
        if config.provider == Provider::Acp {
            acp(ui, config, chat, true);
        } else {
            ui.add_enabled_ui(!read_only, |ui| {
                for choice in [Access::ReadOnly, Access::Project, Access::Full] {
                    if ui
                        .selectable_label(config.access() == choice, choice.label())
                        .clicked()
                    {
                        config.access = choice;
                        config.read_only = false;
                    }
                }
            });
            if access == Access::Full {
                ui.label("Full access uses the permissions available to the agent on this host.");
            }
            if access == Access::ReadOnly {
                ui.small("Read-only mode rejects requests for additional tool permissions.");
            }
        }
        ui.separator();
        ui.label("Permission requests");
        for choice in [Approval::Ask, Approval::Never] {
            ui.selectable_value(&mut config.approval, choice, choice.label());
        }
        ui.small("Ask when needed shows the agent's approval requests. Never ask declines requests; tools already permitted by the agent's mode can run automatically.");
        ui.separator();
        ui.label(if isolated {
            "The project sandbox still limits files and networking."
        } else {
            "Normal mode: the project has no outer sandbox."
        });
        if read_only {
            ui.label("The project's read-only restriction remains active.");
        }
        ui.small("Changes apply before the next message.");
    });
}

fn menu(ui: &mut egui::Ui, label: &str, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 24.),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            egui::containers::menu::MenuButton::from_button(
                egui::Button::new(label)
                    .min_size(egui::vec2(width, 24.))
                    .truncate(),
            )
            .ui(ui, add)
            .0
            .on_hover_text(label);
        },
    );
}

fn acp(ui: &mut egui::Ui, config: &mut Config, chat: &Chat, permissions: bool) {
    if !chat.controls_match(config) || chat.controls.is_empty() {
        ui.label("This agent has not advertised model or mode selectors. Available controls appear after the first message.");
        return;
    }
    let mut shown = false;
    egui::ScrollArea::vertical()
        .id_salt(("acp-controls", permissions))
        .max_height(230.)
        .show(ui, |ui| {
            for control in &chat.controls {
                let model_control = matches!(
                    control.category.as_str(),
                    "model" | "model_config" | "thought_level"
                );
                if permissions == model_control {
                    continue;
                }
                shown = true;
                ui.push_id(&control.id, |ui| option(ui, config, control));
            }
        });
    if !shown {
        ui.small("No supported selector is advertised for this category.");
    }
}

fn option(ui: &mut egui::Ui, config: &mut Config, control: &Control) {
    let value = config
        .acp_options
        .get(&control.id)
        .unwrap_or(&control.current);
    if control.boolean {
        let mut selected = value.as_bool().unwrap_or(false);
        if ui
            .checkbox(&mut selected, &control.name)
            .on_hover_text(&control.description)
            .changed()
        {
            config
                .acp_options
                .insert(control.id.clone(), Value::Bool(selected));
        }
    } else {
        ui.label(&control.name).on_hover_text(&control.description);
        let mut selected = value.as_str().unwrap_or_default().to_owned();
        let name = control
            .options
            .iter()
            .find(|o| o.id == selected)
            .map(|o| o.name.as_str())
            .unwrap_or(&selected);
        egui::ComboBox::from_id_salt("option")
            .width(240.)
            .selected_text(name)
            .show_ui(ui, |ui| {
                for item in &control.options {
                    ui.selectable_value(&mut selected, item.id.clone(), &item.name)
                        .on_hover_text(&item.description);
                }
            });
        if value.as_str() != Some(selected.as_str()) {
            config
                .acp_options
                .insert(control.id.clone(), selected.into());
        }
    }
}

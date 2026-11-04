//! Compact native model/permission popovers shared by the chat providers.
//! Opening Model can request metadata independently of the first chat message.
//! Child choices use native submenus: a ComboBox would replace the parent in
//! egui's single remembered-popup slot and disappear before selection.

use crate::agent::{
    Access, Approval, Config, Personality, Provider,
    controls::{Catalog, Control},
    protocol::Chat,
};
use eframe::egui;
use serde_json::Value;

/// Presentation control only: opening/selecting this menu never starts a backend
/// or changes model, reasoning, approval or tool settings.
pub(super) fn personality(ui: &mut egui::Ui, config: &mut Config, enabled: bool) {
    let selected = config.personality;
    ui.add_enabled_ui(enabled, |ui| {
        egui::containers::menu::MenuButton::from_button(
            egui::Button::new(selected.button()).selected(selected != Personality::Default),
        ).ui(ui, |ui| {
            ui.set_max_width(270.);
            ui.strong("Response personality");
            for choice in Personality::ALL {
                ui.selectable_value(&mut config.personality, choice, choice.label())
                    .on_hover_text(choice.description());
            }
            ui.separator();
            ui.add(egui::Label::new("Reply-style hint. Model, tools, permissions and project settings stay unchanged. Applies to the next message.").wrap());
        }).0.on_hover_text(format!("Response personality: {}\n{}", selected.label(), selected.description()));
    });
}

/// Edit model/reasoning and permission selectors using only metadata matching the
/// current provider/configuration. Return whether Model requested separate discovery.
pub(super) fn buttons(
    ui: &mut egui::Ui,
    config: &mut Config,
    chat: &Chat,
    catalog: Option<&Catalog>,
    read_only: bool,
    isolated: bool,
) -> bool {
    let catalog = catalog.filter(|catalog| catalog.matches(config));
    let models = if chat.controls_match(config) && !chat.models.is_empty() {
        chat.models.as_slice()
    } else {
        catalog
            .map(|catalog| catalog.models.as_slice())
            .unwrap_or_default()
    };
    let mut discover = false;
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
                if !models.is_empty() {
                    egui::ScrollArea::vertical()
                        .id_salt("codex-model-picker")
                        .max_height(180.)
                        .show(ui, |ui| {
                            for model in models {
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
                    match catalog {
                        None => {
                            discover = true;
                            ui.small("Loading model choices…");
                        }
                        Some(catalog) if catalog.busy() => {
                            ui.small("Loading model choices…");
                        }
                        Some(catalog) if catalog.error.is_none() => {
                            ui.small(
                                "This agent did not advertise model choices. Enter a custom ID.",
                            );
                        }
                        Some(_) => {}
                    }
                }
                if let Some(catalog) = catalog
                    && !catalog.busy()
                    && (catalog.error.is_some() || catalog.models.is_empty())
                {
                    if let Some(error) = &catalog.error {
                        ui.small(error);
                    }
                    discover = ui.small_button("Retry model discovery").clicked();
                }
            } else {
                ui.small("Enter a model installed on the configured local server.");
            }
            if old != config.model() {
                config.reasoning_effort.clear();
            }
            ui.separator();
            ui.label("Reasoning effort");
            let advertised = config.provider == Provider::Codex
                && models.iter().any(|m| {
                    (m.value.id == config.model() || (config.model().is_empty() && m.default))
                        && !m.efforts.is_empty()
                });
            if advertised {
                let selected = if config.reasoning_effort.is_empty() {
                    "Default".to_owned()
                } else {
                    config.reasoning_effort.clone()
                };
                choices(ui, selected, |ui| {
                    ui.selectable_value(&mut config.reasoning_effort, String::new(), "Default");
                    let model = models
                        .iter()
                        .find(|m| m.value.id == config.model())
                        .or_else(|| {
                            config
                                .model()
                                .is_empty()
                                .then(|| models.iter().find(|m| m.default))
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

    let access = if read_only && !config.files_only() {
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
            for choice in [
                Access::ReadOnly,
                Access::FilesOnly,
                Access::Project,
                Access::Full,
            ] {
                if ui
                    .add_enabled(
                        !read_only || matches!(choice, Access::ReadOnly | Access::FilesOnly),
                        egui::Button::selectable(access == choice, choice.label()),
                    )
                    .clicked()
                {
                    config.access = choice;
                    config.read_only = false;
                }
            }
            if read_only {
                ui.small("The project's read-only policy still prevents all file edits.");
            }
            if access == Access::Full {
                ui.label("Full access uses the permissions available to the agent on this host.");
            }
            if access == Access::ReadOnly {
                ui.small("Read-only mode rejects requests for additional tool permissions.");
            }
            if access == Access::FilesOnly {
                ui.small("Read, search and edit project files without running commands. Builds, tests and Git commands are left to you. Symlinks and Git/Codex metadata are protected.");
                ui.small("Changing this mode starts a new Codex conversation. Live file-only chats can be reattached through Running.");
            }
        }
        ui.separator();
        ui.label("Permission requests");
        if config.files_only() {
            ui.small(
                "File-only mode automatically declines all requests for additional permissions.",
            );
        } else {
            for choice in [Approval::Ask, Approval::Never] {
                ui.selectable_value(&mut config.approval, choice, choice.label());
            }
            ui.small("Ask when needed shows the agent's approval requests. Never ask declines requests; tools already permitted by the agent's mode can run automatically.");
        }
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
    discover
}

fn menu(ui: &mut egui::Ui, label: &str, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let height = crate::app::layout::control_height(ui);
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            egui::containers::menu::MenuButton::from_button(
                egui::Button::new(label)
                    .min_size(egui::vec2(width, height))
                    .truncate(),
            )
            .ui(ui, add)
            .0
            .on_hover_text(label);
        },
    );
}

/// Keep large backend-defined lists scrollable without replacing their parent
/// popup. A bounded initial width leaves room for the editor on small displays.
fn choices(ui: &mut egui::Ui, label: String, add: impl FnOnce(&mut egui::Ui)) {
    egui::containers::menu::SubMenuButton::new(label).ui(ui, |ui| {
        ui.set_width(200.);
        egui::ScrollArea::vertical().max_height(230.).show(ui, add);
    });
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
        choices(ui, name.to_owned(), |ui| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::controls::{Model, OptionValue};

    #[test]
    fn file_only_access_is_selectable_without_starting_an_agent() {
        for size in [egui::vec2(1380., 900.), egui::vec2(620., 560.)] {
            let ctx = egui::Context::default();
            let mut config = Config::default();
            let before = config.clone();
            let chat = Chat::default();
            frame(&ctx, &mut config, &chat, size, vec![]);
            let output = frame(&ctx, &mut config, &chat, size, vec![]);
            let pos = position(&output, "Access: Project access");
            let output = click(&ctx, &mut config, &chat, size, pos);
            let pos = position(&output, "Edit project files only");
            click(&ctx, &mut config, &chat, size, pos);
            assert_eq!(config.access(), Access::FilesOnly);
            assert!(!config.same_process(&before));
            let saved: Config =
                serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
            assert!(saved.files_only());
            assert!(!chat.connected());
            assert!(chat.messages.is_empty());
        }
    }

    fn frame(
        ctx: &egui::Context,
        config: &mut Config,
        chat: &Chat,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        frame_policy(ctx, config, chat, size, events, false)
    }

    fn frame_policy(
        ctx: &egui::Context,
        config: &mut Config,
        chat: &Chat,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        read_only: bool,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add_space(size.y - 160.);
                    ui.horizontal(|ui| buttons(ui, config, chat, None, read_only, false));
                });
            },
        )
    }

    fn position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() / 2.)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing control: {label}"))
    }

    fn click(
        ctx: &egui::Context,
        config: &mut Config,
        chat: &Chat,
        size: egui::Vec2,
        pos: egui::Pos2,
    ) -> egui::FullOutput {
        click_policy(ctx, config, chat, size, pos, false)
    }

    fn click_policy(
        ctx: &egui::Context,
        config: &mut Config,
        chat: &Chat,
        size: egui::Vec2,
        pos: egui::Pos2,
        read_only: bool,
    ) -> egui::FullOutput {
        for pressed in [true, false] {
            frame_policy(
                ctx,
                config,
                chat,
                size,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                read_only,
            );
        }
        frame_policy(ctx, config, chat, size, vec![], read_only)
    }

    #[test]
    fn read_only_projects_allow_file_only_controls_without_enabling_writes() {
        let ctx = egui::Context::default();
        let mut config = Config::default();
        let chat = Chat::default();
        let size = egui::vec2(1100., 900.);
        frame_policy(&ctx, &mut config, &chat, size, vec![], true);
        let output = frame_policy(&ctx, &mut config, &chat, size, vec![], true);
        let pos = position(&output, "Access: Read only");
        let output = click_policy(&ctx, &mut config, &chat, size, pos, true);
        let pos = position(&output, "Edit project files only");
        click_policy(&ctx, &mut config, &chat, size, pos, true);
        assert!(config.files_only());
        let output = frame_policy(&ctx, &mut config, &chat, size, vec![], true);
        let pos = position(&output, "Access: Edit project files only");
        let output = click_policy(&ctx, &mut config, &chat, size, pos, true);
        let pos = position(&output, "Full access");
        click_policy(&ctx, &mut config, &chat, size, pos, true);
        assert!(config.files_only());
        assert!(!chat.connected());
    }

    #[test]
    fn model_and_reasoning_menus_keep_advertised_choices_selectable() {
        for size in [egui::vec2(1100., 900.), egui::vec2(390., 600.)] {
            let ctx = egui::Context::default();
            let mut config = Config::default();
            let mut chat = Chat::default();
            chat.models = (0..8)
                .map(|index| Model {
                    value: OptionValue {
                        id: format!("model-{index}"),
                        name: format!("Model {index}"),
                        description: String::new(),
                    },
                    default: index == 0,
                    efforts: ["low", "medium", "high", "xhigh", "max", "ultra"]
                        .into_iter()
                        .map(|id| OptionValue {
                            id: id.into(),
                            name: id.into(),
                            description: String::new(),
                        })
                        .collect(),
                })
                .collect();
            frame(&ctx, &mut config, &chat, size, vec![]);
            let output = frame(&ctx, &mut config, &chat, size, vec![]);
            let pos = position(&output, "Model: default");
            let output = click(&ctx, &mut config, &chat, size, pos);
            let pos = position(&output, "Model 1");
            click(&ctx, &mut config, &chat, size, pos);
            assert_eq!(config.model, "model-1");
            let output = frame(&ctx, &mut config, &chat, size, vec![]);
            let pos = position(&output, "Model: model-1");
            let output = click(&ctx, &mut config, &chat, size, pos);
            let pos = position(&output, "Default");
            let output = click(&ctx, &mut config, &chat, size, pos);
            let pos = position(&output, "ultra");
            click(&ctx, &mut config, &chat, size, pos);
            assert_eq!(config.reasoning_effort, "ultra");
            assert!(!chat.connected());
            assert!(chat.messages.is_empty());
        }
    }
}

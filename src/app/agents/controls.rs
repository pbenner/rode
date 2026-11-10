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
        let (response, _) = egui::containers::menu::MenuButton::from_button(
            egui::Button::new(selected.button()).selected(selected != Personality::Default),
        ).ui(ui, |ui| {
            ui.set_max_width(270.);
            ui.strong("Response personality");
            for choice in Personality::ALL {
                let response = ui.selectable_value(&mut config.personality, choice, choice.label())
                    .on_hover_text(choice.description());
                crate::app::ui_probe::button(ui, &format!("agent-tone-choice-{}", choice.id()), &response);
            }
            ui.separator();
            ui.add(egui::Label::new("Reply-style hint. Model, tools, permissions and project settings stay unchanged. Applies to the next message.").wrap());
        });
        let response = response.on_hover_text(format!("Response personality: {}\n{}", selected.label(), selected.description()));
        crate::app::ui_probe::button(ui, "agent-tone", &response);
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
    let model = if config.provider.is_acp() {
        chat.controls
            .iter()
            .find(|c| c.category == "model" && chat.controls_match(config))
            .and_then(|c| {
                config
                    .acp_options()
                    .get(&c.id)
                    .unwrap_or(&c.current)
                    .as_str()
            })
            .unwrap_or("agent")
    } else if config.model().is_empty() {
        "default"
    } else {
        config.model()
    };
    let label = format!("Model: {model}");
    menu(ui, "agent-model-menu", &label, width, |ui| {
        ui.set_min_width(260.);
        ui.set_max_width(360.);
        ui.label(egui::RichText::new("Model and reasoning").strong());
        if config.provider.is_acp() {
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
                                let response = ui
                                    .selectable_label(
                                        config.model == model.value.id,
                                        &model.value.name,
                                    )
                                    .on_hover_text(&model.value.description);
                                crate::app::ui_probe::button(
                                    ui,
                                    &format!("agent-model-choice-{}", model.value.id),
                                    &response,
                                );
                                if response.clicked() {
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
                choices(ui, "agent-reasoning-menu", selected, |ui| {
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
                            let response = ui
                                .selectable_value(
                                    &mut config.reasoning_effort,
                                    effort.id.clone(),
                                    &effort.name,
                                )
                                .on_hover_text(&effort.description);
                            crate::app::ui_probe::button(
                                ui,
                                &format!("agent-reasoning-{}", effort.id),
                                &response,
                            );
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
        config.access
    };
    let label = if config.provider.is_acp() {
        "Permissions".into()
    } else if isolated && matches!(access, Access::Project | Access::Full) {
        "Access: Project sandbox".into()
    } else {
        format!("Access: {}", access.label())
    };
    menu(ui, "agent-access-menu", &label, width, |ui| {
        ui.set_min_width(260.);
        ui.set_max_width(360.);
        ui.label(egui::RichText::new("Agent permissions").strong());
        if config.provider.is_acp() {
            acp(ui, config, chat, true);
        } else {
            codex_access(ui, config, access, read_only, isolated);
        }
        ui.separator();
        ui.label("Agent networking");
        for choice in [
            crate::agent::Network::Project,
            crate::agent::Network::ModelOnly,
        ] {
            if ui
                .add_enabled(
                    choice == crate::agent::Network::Project || !config.provider.is_acp(),
                    egui::Button::selectable(config.network == choice, choice.label()),
                )
                .on_disabled_hover_text("This provider cannot enforce offline native tools.")
                .clicked()
            {
                config.network = choice;
            }
        }
        if config.network == crate::agent::Network::ModelOnly {
            ui.small("Requires an active project sandbox with Allow network access turned off. The model controller can connect; native commands stay offline. Web, browser, MCP and plugin tools are disabled. Changing this mode or whether commands are allowed starts a new conversation.");
        }
        ui.separator();
        ui.label("Permission requests");
        if config.native_tools() {
            ui.small(
                "Confined native tools automatically decline all requests for additional permissions.",
            );
        } else {
            for choice in [Approval::Ask, Approval::Never] {
                let response = ui.selectable_value(&mut config.approval, choice, choice.label());
                crate::app::ui_probe::button(
                    ui,
                    &format!("agent-approval-{}", choice.label()),
                    &response,
                );
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

/// Show the actual enforced access without rewriting saved normal-mode choices.
/// Project/Full already share Codex's external sandbox policy when RODE enforces
/// the project boundary. Keep both visible but disabled, and offer a safe way
/// back from read-only/file-only access without persisting unrestricted access.
fn codex_access(
    ui: &mut egui::Ui,
    config: &mut Config,
    access: Access,
    read_only: bool,
    isolated: bool,
) {
    if isolated {
        let response = ui
            .add_enabled(
                !read_only,
                egui::Button::selectable(
                    matches!(access, Access::Project | Access::Full),
                    "Use project sandbox",
                ),
            )
            .on_disabled_hover_text("The project's read-only policy prevents file edits.");
        crate::app::ui_probe::button(ui, "agent-access-project-sandbox", &response);
        if response.clicked() {
            config.access = Access::Project;
        }
    }
    for choice in [
        Access::ReadOnly,
        Access::FilesOnly,
        Access::Project,
        Access::Full,
    ] {
        let redundant = isolated && matches!(choice, Access::Project | Access::Full);
        let response = ui
            .add_enabled(
                !redundant
                    && (!read_only || matches!(choice, Access::ReadOnly | Access::FilesOnly)),
                egui::Button::selectable(!redundant && access == choice, choice.label()),
            )
            .on_disabled_hover_text(if redundant {
                "RODE already enforces the project sandbox. Choose Use project sandbox to avoid duplicate agent sandboxing."
            } else {
                "The project's read-only policy prevents file edits."
            });
        crate::app::ui_probe::button(ui, &format!("agent-access-{}", choice.label()), &response);
        if response.clicked() {
            config.access = choice;
        }
    }
    if isolated {
        ui.small("RODE provides the sandbox. Duplicate agent sandbox settings are disabled.");
    }
    if read_only {
        ui.small("The project's read-only policy still prevents all file edits.");
    }
    if access == Access::Full && !isolated {
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

fn menu(ui: &mut egui::Ui, name: &str, label: &str, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let height = crate::app::layout::control_height(ui);
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let response = egui::containers::menu::MenuButton::from_button(
                egui::Button::new(label)
                    .min_size(egui::vec2(width, height))
                    .truncate(),
            )
            .ui(ui, add)
            .0
            .on_hover_text(label);
            crate::app::ui_probe::button(ui, name, &response);
            crate::app::ui_probe::button(ui, label, &response);
        },
    );
}

/// Keep large backend-defined lists scrollable without replacing their parent
/// popup. A bounded initial width leaves room for the editor on small displays.
fn choices(ui: &mut egui::Ui, name: &str, label: String, add: impl FnOnce(&mut egui::Ui)) {
    let response = egui::containers::menu::SubMenuButton::new(label)
        .ui(ui, |ui| {
            ui.set_width(200.);
            egui::ScrollArea::vertical().max_height(230.).show(ui, add);
        })
        .0;
    crate::app::ui_probe::button(ui, name, &response);
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
        .acp_options()
        .get(&control.id)
        .unwrap_or(&control.current);
    if control.boolean {
        let mut selected = value.as_bool().unwrap_or(false);
        let response = ui
            .checkbox(&mut selected, &control.name)
            .on_hover_text(&control.description);
        crate::app::ui_probe::button(ui, &format!("agent-acp-option-{}", control.id), &response);
        if response.changed() {
            config
                .acp_options_mut()
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
        choices(
            ui,
            &format!("agent-acp-option-{}", control.id),
            name.to_owned(),
            |ui| {
                for item in &control.options {
                    let response = ui
                        .selectable_value(&mut selected, item.id.clone(), &item.name)
                        .on_hover_text(&item.description);
                    crate::app::ui_probe::button(
                        ui,
                        &format!("agent-acp-choice-{}-{}", control.id, item.id),
                        &response,
                    );
                }
            },
        );
        if value.as_str() != Some(selected.as_str()) {
            config
                .acp_options_mut()
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
            assert_eq!(config.access, Access::FilesOnly);
            assert!(!config.same_process(&before));
            let saved: Config =
                serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
            assert!(saved.files_only());
            assert!(!chat.connected());
            assert!(chat.messages.is_empty());
        }
    }

    #[test]
    fn model_only_network_is_selectable_and_persisted_without_starting_an_agent() {
        for size in [egui::vec2(1380., 900.), egui::vec2(620., 560.)] {
            let ctx = egui::Context::default();
            let mut config = Config::default();
            let before = config.clone();
            let chat = Chat::default();
            frame(&ctx, &mut config, &chat, size, vec![]);
            let output = frame(&ctx, &mut config, &chat, size, vec![]);
            let pos = position(&output, "Access: Project access");
            let output = click(&ctx, &mut config, &chat, size, pos);
            let pos = position(&output, "Model connection only");
            click(&ctx, &mut config, &chat, size, pos);
            assert_eq!(config.network, crate::agent::Network::ModelOnly);
            assert!(!config.same_process(&before));
            let saved: Config =
                serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
            assert_eq!(saved.network, crate::agent::Network::ModelOnly);
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
        frame_policy(ctx, config, chat, size, events, false, false)
    }

    fn frame_policy(
        ctx: &egui::Context,
        config: &mut Config,
        chat: &Chat,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        read_only: bool,
        isolated: bool,
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
                    ui.horizontal(|ui| buttons(ui, config, chat, None, read_only, isolated));
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
        click_policy(ctx, config, chat, size, pos, false, false)
    }

    fn click_policy(
        ctx: &egui::Context,
        config: &mut Config,
        chat: &Chat,
        size: egui::Vec2,
        pos: egui::Pos2,
        read_only: bool,
        isolated: bool,
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
                isolated,
            );
        }
        frame_policy(ctx, config, chat, size, vec![], read_only, isolated)
    }

    #[test]
    fn read_only_projects_allow_file_only_controls_without_enabling_writes() {
        let ctx = egui::Context::default();
        let mut config = Config::default();
        let chat = Chat::default();
        let size = egui::vec2(1100., 900.);
        frame_policy(&ctx, &mut config, &chat, size, vec![], true, false);
        let output = frame_policy(&ctx, &mut config, &chat, size, vec![], true, false);
        let pos = position(&output, "Access: Read only");
        let output = click_policy(&ctx, &mut config, &chat, size, pos, true, false);
        let pos = position(&output, "Edit project files only");
        click_policy(&ctx, &mut config, &chat, size, pos, true, false);
        assert!(config.files_only());
        let output = frame_policy(&ctx, &mut config, &chat, size, vec![], true, false);
        let pos = position(&output, "Access: Edit project files only");
        let output = click_policy(&ctx, &mut config, &chat, size, pos, true, false);
        let pos = position(&output, "Full access");
        click_policy(&ctx, &mut config, &chat, size, pos, true, false);
        assert!(config.files_only());
        assert!(!chat.connected());
    }

    #[test]
    fn active_sandboxes_disable_duplicate_choices_without_rewriting_normal_mode_preferences() {
        for provider in [Provider::Codex, Provider::Local] {
            for saved in [Access::Project, Access::Full] {
                for size in [egui::vec2(1100., 900.), egui::vec2(390., 600.)] {
                    let ctx = egui::Context::default();
                    let chat = Chat::default();
                    let mut config = Config {
                        provider,
                        access: saved,
                        ..Default::default()
                    };
                    frame_policy(&ctx, &mut config, &chat, size, vec![], false, true);
                    let output = frame_policy(&ctx, &mut config, &chat, size, vec![], false, true);
                    let pos = position(&output, "Access: Project sandbox");
                    let output = click_policy(&ctx, &mut config, &chat, size, pos, false, true);
                    position(
                        &output,
                        "RODE provides the sandbox. Duplicate agent sandbox settings are disabled.",
                    );
                    for choice in [Access::Project, Access::Full] {
                        let pos = position(&output, choice.label());
                        click_policy(&ctx, &mut config, &chat, size, pos, false, true);
                        assert_eq!(config.access, saved, "Disabled choice changed preferences");
                    }
                    // With no enforced outer sandbox (including a failed
                    // sandbox probe), the saved choice is still available.
                    let ctx = egui::Context::default();
                    frame(&ctx, &mut config, &chat, size, vec![]);
                    let output = frame(&ctx, &mut config, &chat, size, vec![]);
                    let pos = position(&output, &format!("Access: {}", saved.label()));
                    let output = click(&ctx, &mut config, &chat, size, pos);
                    let other = if saved == Access::Project {
                        Access::Full
                    } else {
                        Access::Project
                    };
                    let pos = position(&output, other.label());
                    click(&ctx, &mut config, &chat, size, pos);
                    assert_eq!(config.access, other);
                    assert!(!chat.connected());
                }
            }
        }
    }

    #[test]
    fn sandboxed_chats_keep_action_restrictions_and_can_return_to_the_project_policy() {
        for (read_only, network) in [
            (false, crate::agent::Network::Project),
            (true, crate::agent::Network::Project),
            (false, crate::agent::Network::ModelOnly),
            (true, crate::agent::Network::ModelOnly),
        ] {
            let ctx = egui::Context::default();
            let chat = Chat::default();
            let size = egui::vec2(390., 600.);
            let mut config = Config {
                access: Access::ReadOnly,
                approval: Approval::Never,
                network,
                ..Default::default()
            };
            frame_policy(&ctx, &mut config, &chat, size, vec![], read_only, true);
            for choice in [Access::FilesOnly, Access::ReadOnly, Access::FilesOnly] {
                let output = frame_policy(&ctx, &mut config, &chat, size, vec![], read_only, true);
                let pos = position(&output, &format!("Access: {}", config.access.label()));
                let output = click_policy(&ctx, &mut config, &chat, size, pos, read_only, true);
                let pos = position(&output, choice.label());
                click_policy(&ctx, &mut config, &chat, size, pos, read_only, true);
                assert_eq!(config.access, choice);
            }
            let output = frame_policy(&ctx, &mut config, &chat, size, vec![], read_only, true);
            let pos = position(&output, "Access: Edit project files only");
            let output = click_policy(&ctx, &mut config, &chat, size, pos, read_only, true);
            let pos = position(&output, "Use project sandbox");
            click_policy(&ctx, &mut config, &chat, size, pos, read_only, true);
            assert_eq!(
                config.access,
                if read_only {
                    Access::FilesOnly
                } else {
                    Access::Project
                }
            );
            assert_eq!(config.approval, Approval::Never);
            assert_eq!(config.network, network);
            assert!(!chat.connected());
        }
    }

    #[test]
    fn model_and_reasoning_menus_keep_advertised_choices_selectable() {
        for (size, hover_open) in [egui::vec2(1100., 900.), egui::vec2(390., 600.)]
            .into_iter()
            .flat_map(|size| [false, true].map(|hover_open| (size, hover_open)))
        {
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
            let output = if hover_open {
                // Native fixtures open submenus by hovering and click only the
                // actual choice after a subsequent layout pass.
                frame(
                    &ctx,
                    &mut config,
                    &chat,
                    size,
                    vec![egui::Event::PointerMoved(pos)],
                );
                frame(&ctx, &mut config, &chat, size, vec![])
            } else {
                click(&ctx, &mut config, &chat, size, pos)
            };
            let pos = position(&output, "ultra");
            click(&ctx, &mut config, &chat, size, pos);
            assert_eq!(config.reasoning_effort, "ultra");
            assert!(!chat.connected());
            assert!(chat.messages.is_empty());
        }
    }
}

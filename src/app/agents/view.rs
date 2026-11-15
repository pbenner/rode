//! Agent presentation consumes immutable application facts and returns explicit actions.
//! Rendering can mutate its own chat/composer state but cannot save documents,
//! switch projects or focus another application surface directly.
use super::*;

/// Read-only facts projected by the application before borrowing the agent view.
pub(super) struct Facts<'a> {
    pub palette: theme::Palette,
    pub root: String,
    pub host: String,
    pub environment: &'a controller::Environment,
    pub offline: bool,
    pub unsaved: usize,
    pub modal: bool,
}

/// Explicit user intents applied in the same order as the original UI commands.
#[derive(Default)]
pub(super) struct Actions {
    pub save: bool,
    pub attach: bool,
    pub discover_models: bool,
    pub send: bool,
    pub history: history::Action,
    pub refresh_documents: bool,
    pub running: bool,
    pub recover: Option<bool>,
}

fn mode_detail(environment: &controller::Environment) -> String {
    let plan = &environment.plan;
    if plan.active {
        format!(
            "{} is active. Project tools have {} network access.\n{}{}",
            plan.backend.label(),
            if plan.prefix.iter().any(|arg| arg == "--share-net") {
                "enabled"
            } else {
                "blocked"
            },
            if plan.project_read_only {
                "The project is read only.\n"
            } else {
                ""
            },
            plan.reason
        )
    } else if plan.requested {
        format!(
            "The requested {} sandbox is unavailable; the project is in normal mode. Agent access controls still apply.\n{}",
            plan.backend.label(),
            plan.reason
        )
    } else {
        "No outer project sandbox is active. Agent access controls still apply.".to_owned()
    }
}

impl Agents {
    /// Render cached conversation, approvals, settings and composer state; dispatch
    /// explicit actions only after capturing the current host, policy and source context.
    pub(super) fn render(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        facts: Facts<'_>,
    ) -> Option<Actions> {
        let Facts {
            palette,
            root,
            host,
            environment,
            offline,
            unsaved,
            modal,
        } = facts;
        let mode = environment.plan.label();
        // The chat can be detached from the main project banner. Keep the
        // applied grants and fallback reason reachable in that window too.
        let mode_detail = mode_detail(environment);
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        // A parent may retain a larger minimum width after a resize. Size all
        // chat controls against the visible panel rather than that stale width.
        let rect = rect.intersect(ui.clip_rect());
        if !rect.is_positive() {
            return None;
        }
        // Hover/focus frames extend beyond widget bounds. Keep that paint space
        // inside the panel instead of clipping it at the header or composer.
        let paint_margin = ui.visuals().clip_rect_margin;
        // Give history the full compact panel; retain Stop while a turn is running.
        self.composer.set_attachment_height(self.files.height());
        let answering = self
            .chat
            .prompts
            .front()
            .is_some_and(|p| !p.questions.is_empty());
        if answering {
            // Questions take precedence over a history browser opened while
            // the turn was running. Never hide an outstanding input request.
            self.history_open = false;
            self.browser = None;
        }
        let hide_composer = answering || self.history_open && !self.chat.busy;
        let read_only = environment.plan.project_read_only;
        let isolated = environment.plan.active;
        let layout = self.composer.layout(
            ui,
            rect.shrink(paint_margin),
            unsaved > 0,
            hide_composer,
            !modal,
        );
        let composer = layout.input;
        let mut content = layout.content;
        let queue_height = if hide_composer {
            0.
        } else {
            self.queue.height().min(content.height() * 0.45)
        };
        let queue_rect = egui::Rect::from_min_max(
            egui::pos2(content.left(), content.bottom() - queue_height),
            content.max,
        );
        if queue_height > 0. {
            content.max.y = (queue_rect.top() - 6.).max(content.top());
        }
        let mut send = false;
        let mut discover_models = false;
        let mut attach = false;
        let mut stop = false;
        let mut new_chat = false;
        let mut recover = None;
        let mut running = false;
        let mut save = false;
        let mut answer = None;
        let mut use_host_tone = None;
        let mut history_action = history::Action::default();
        let remote = environment.remote.clone();
        let agents = self;
        let installing = agents.runtime.installing();
        let install_enabled = !offline
            && !modal
            && !agents.chat.busy
            && matches!(agents.config.codex.trim(), "" | "auto" | "codex")
            && std::env::var_os("RODE_CODEX_BUNDLE").is_none();
        let automatic_codex = matches!(agents.config.codex.trim(), "" | "auto" | "codex");
        let codex_available = if agents.config.provider.is_acp() || !automatic_codex {
            Some(true)
        } else if offline {
            None
        } else {
            agents.runtime.check(ui, remote.as_ref())
        };
        let before = agents.config.clone();
        ui.scope_builder(egui::UiBuilder::new().max_rect(content),|ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(content.expand(paint_margin)));
            let new_chat_enabled = agents.can_start_new_chat();
            let actions = header::show(ui, &mut agents.config, agents.chat.busy, agents.chat.queue_pending(), offline, new_chat_enabled);
            if actions.provider_changed {
                agents.settings = true;
                agents.arguments = serde_json::to_string(agents.config.acp_arguments()).unwrap_or_else(|_| "[]".into());
                agents.argument_error.clear();
            }
            if actions.settings { agents.settings = !agents.settings; }
            new_chat |= actions.new_chat;
            ui.add(egui::Label::new(palette.tag(format!("{host} · {mode}"))).truncate()).on_hover_text(&mode_detail);
            ui.add(egui::Label::new(palette.tag(&root)).truncate()).on_hover_text(&root);
            if agents.settings && !answering {
                egui::ScrollArea::vertical().id_salt("agent-settings-scroll")
                    .max_height((ui.available_height()-90.).clamp(60.,240.)).show(ui,|ui| {
                    ui.add_enabled_ui(!agents.chat.busy && !agents.chat.queue_pending(),|ui| {
                        if agents.config.provider.is_acp() {
                            let claude = agents.config.provider == Provider::Claude;
                            ui.label(if claude { "Claude ACP adapter on this host" } else { "ACP executable on this host" });
                            let program = if claude { &mut agents.config.claude_program } else { &mut agents.config.acp_program };
                            let executable = ui.add(crate::keyboard::named("Agent executable", egui::TextEdit::singleline(program).desired_width(f32::INFINITY).hint_text(if claude { rode_agent_core::CLAUDE_ACP } else { "agent executable or absolute path" }).char_limit(4096)));
                            ui_probe::button(ui, if claude { "agent-claude-executable" } else { "agent-acp-executable" }, &executable);
                            ui.label("Arguments (JSON array)");
                            if ui.add(crate::keyboard::named("Agent arguments", egui::TextEdit::singleline(&mut agents.arguments).desired_width(f32::INFINITY).hint_text("[\"acp\"]").char_limit(16384))).changed() {
                                match serde_json::from_str::<Vec<String>>(&agents.arguments) {
                                    Ok(args) => { *agents.config.acp_arguments_mut()=args; agents.argument_error.clear(); }
                                    Err(_) => agents.argument_error="Use a JSON array of argument strings.".into(),
                                }
                            }
                            if !agents.argument_error.is_empty() { ui.colored_label(palette.error,&agents.argument_error); }
                            ui.label(palette.tag("Agent must provide its own file and command tools."));
                            if claude {
                                ui.add(egui::Label::new("Install claude-agent-acp and authenticate Claude on this project host. The adapter uses your Claude login or ANTHROPIC_API_KEY from the host environment.").wrap());
                                let setup = ui.hyperlink_to("Claude adapter setup", "https://github.com/agentclientprotocol/claude-agent-acp");
                                crate::keyboard::target(ui, "Claude adapter setup", &setup);
                            }
                            if crate::keyboard::small_button(ui, "Reset saved agent controls").clicked() {
                                agents.config.acp_options_mut().clear();
                                agents.queue.clear();
                                agents.chat.disconnect();
                                agents.chat.notice("Agent controls reset. The next message starts a fresh conversation.");
                            }
                        } else {
                            ui.label("Codex executable (auto: discover)");
                            let executable = ui.add(crate::keyboard::named("Codex executable", egui::TextEdit::singleline(&mut agents.config.codex).desired_width(f32::INFINITY).hint_text("auto, or /path/to/codex").char_limit(4096))).on_hover_text("Use Install Codex below, the optional rode-codex package or an installed executable. Sign in locally with rode --codex login. SSH projects use Codex and credentials on that host.");
                            ui_probe::button(ui, "agent-codex-executable", &executable);
                            if agents.config.provider == Provider::Local {
                                ui.label("Local model API base URL");
                                ui.add(crate::keyboard::named("Local model server URL", egui::TextEdit::singleline(&mut agents.config.local_url).desired_width(f32::INFINITY).char_limit(2048))).on_hover_text("An Ollama /v1 or compatible Responses endpoint reachable from the project host. localhost means this host.");
                                ui.label("Installed model name");
                                let model = ui.add(crate::keyboard::named("Installed model name", egui::TextEdit::singleline(&mut agents.config.local_model).desired_width(f32::INFINITY).hint_text("Model already available on the local server").char_limit(256)));
                                ui_probe::button(ui, "agent-local-model", &model);
                            } else {
                                ui.label("Model (blank: Codex default)");
                                ui.add(crate::keyboard::named("Model", egui::TextEdit::singleline(&mut agents.config.model).desired_width(f32::INFINITY).char_limit(256)));
                            }
                            agents.runtime.ui(ui, remote.as_ref(), install_enabled, codex_available);
                        }
                    });
                    ui.label(RichText::new("Settings are saved for this project. Model and Access controls are below. Executable/provider changes start a new conversation. Use the agent's login or environment for credentials.").small().color(palette.muted));
                });
            }
            ui.horizontal(|ui| { ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let history = ui.add_enabled(!offline && !installing && !answering,crate::keyboard::Button::new("History").selected(agents.history_open)).on_hover_text("Browse saved conversations for this project or other projects on this host");
                ui_probe::button(ui, "agent-history", &history);
                if history.clicked() {
                    agents.history_open = !agents.history_open;
                    if agents.history_open {history_action.refresh=true;} else {agents.browser=None;}
                }
                if ui.add_enabled(!offline, crate::keyboard::Button::new("Running")).on_hover_text("Reattach or stop background agent sessions on this host").clicked() { running = true; }
                ui.add(egui::Label::new(RichText::new(if agents.chat.status.is_empty() {"Ready to start"} else {&agents.chat.status}).size(12.).color(if offline {palette.error}else{palette.accent})).truncate());
            }); });
            if let Some(status) = agents.chat.response_style_status(agents.config.personality) {
                let mismatch = matches!(status, agent::response_style::Status::Mismatch { .. });
                let response = ui.add(egui::Label::new(RichText::new(status.message())
                    .small().color(if mismatch { palette.error } else { palette.muted })).wrap().selectable(true));
                ui_probe::button(ui, if mismatch { "agent-tone-mismatch" } else { "agent-tone-pending" }, &response);
                if let agent::response_style::Status::Mismatch { applied, .. } = status {
                    let accept = ui.add_enabled(!agents.chat.busy && !offline && !modal,
                        crate::keyboard::Button::new(format!("Use {} tone", applied.label())));
                    ui_probe::button(ui, "agent-tone-use-host", &accept);
                    if accept.clicked() { use_host_tone = Some(applied); }
                }
            }
            if answering {
                let working = agents.chat.busy;
                let action = questions::show(ui, agents.chat.prompts.front_mut().unwrap(), !offline && !modal, working, palette);
                answer = action.answer;
                stop = action.stop;
                return;
            }
            recover = recovery::ui(ui, &agents.chat, !offline && !modal && !installing, &agents.config);
            if agents.history_open {
                let panel=history::ui(ui,agents.browser.as_ref(),&mut agents.history_filter,!agents.chat.busy && !modal,agents.chat.conversation_id(),&mut agents.history_delete,palette);
                history_action.refresh |= panel.refresh;
                history_action.more=panel.more;
                history_action.scope=panel.scope;
                history_action.resume=panel.resume;
                history_action.running=panel.running;
                history_action.close=panel.close;
                history_action.delete=panel.delete;
                return;
            }
            if let Some(prompt) = agents.chat.prompts.front_mut() {
                egui::Frame::new().fill(palette.raised).stroke(Stroke::new(1.0_f32,palette.accent)).inner_margin(8).corner_radius(4).show(ui,|ui| {
                    egui::ScrollArea::vertical().id_salt("agent-request").max_height((ui.available_height()*0.65).clamp(60.,200.)).show(ui,|ui| {
                        ui.label(RichText::new(&prompt.title).strong().color(palette.accent));
                        if !prompt.summary.is_empty() {
                            ui.add(egui::Label::new(RichText::new(&prompt.summary).monospace().size(12.)).wrap().selectable(true));
                        }
                        if prompt.questions.is_empty() {
                            crate::keyboard::collapsing(ui, "Details",|ui| { ui.add(egui::Label::new(RichText::new(&prompt.detail).monospace().size(11.)).wrap().selectable(true)); });
                        }
                        ui.horizontal_wrapped(|ui| {
                            for (index, choice) in prompt.choices.iter().enumerate() {
                                let response = crate::keyboard::button(ui, &choice.label);
                                ui_probe::button(ui, &format!("agent-permission-choice-{index}"), &response);
                                if response.clicked() {answer=Some(choice.result.clone());}
                            }
                        });
                    });
                });
            }
            egui::ScrollArea::vertical().id_salt("agent-transcript").stick_to_bottom(true).auto_shrink([false,false]).max_height(ui.available_height().max(1.)).show(ui,|ui| {
                if !agents.config.provider.is_acp() && !agents.settings
                    && (agents.chat.messages.is_empty() || agents.runtime.started()) {
                    agents.runtime.ui(ui, remote.as_ref(), install_enabled, codex_available);
                }
                if agents.chat.messages.is_empty() {
                    ui.add_space(8.);
                    ui.label(RichText::new("Work on this project with an agent").strong());
                    ui.add(egui::Label::new("Ask a question, request a change, or investigate an issue. The agent works in the folder above.").wrap());
                    if agents.config.provider == Provider::Local { ui.add(egui::Label::new("Start Ollama or a compatible local server and select an installed model in settings.").wrap()); }
                    else if agents.config.provider == Provider::Codex { ui.add(egui::Label::new(if codex_available == Some(false) { "Install Codex above or use an existing executable. Authenticate on the project host before sending a message." } else { "Authenticate Codex on the project host before sending a message." }).wrap()); }
                    else if agents.config.provider == Provider::Claude { ui.add(egui::Label::new("Install claude-agent-acp and authenticate Claude on this host, then send a message. Model, effort and permission choices come from Claude.").wrap()); }
                    else { ui.add(egui::Label::new("Install and authenticate your agent on this host before sending a message.").wrap()); }
                }
                for message in &agents.chat.messages {
                    ui.push_id(&message.id,|ui| {
                        if message.role == Role::Tool {
                            crate::keyboard::collapsing(ui, RichText::new(&message.title).monospace().size(12.).color(palette.muted),|ui| {
                                ui.add(egui::Label::new(RichText::new(&message.text).monospace().size(11.)).wrap().selectable(true));
                            });
                        } else {
                            egui::Frame::new().fill(if message.role == Role::User {palette.raised}else{palette.background}).inner_margin(8).corner_radius(4).show(ui,|ui| {
                                let copy = ui.horizontal(|ui| {
                                    ui.label(RichText::new(&message.title).strong().size(11.).color(if message.role == Role::Notice {palette.muted}else{palette.accent}));
                                    if message.role == Role::Assistant {
                                        let button = icons::small(ui,Icon::Copy,"Copy response");
                                        if message.text.len() <= 64 * 1024 {
                                            ui_probe::value(ui, "agent-copy-response", &button, &message.text);
                                        }
                                        button.clicked()
                                    } else {
                                        false
                                    }
                                }).inner;
                                let output = render_message(ui,&message.text,palette,copy);
                                if message.role == Role::Assistant && message.text.len() <= 64 * 1024 {
                                    ui_probe::value(ui, "agent-response", &output.response, &message.text);
                                }
                            });
                        }
                    });
                    ui.add_space(5.);
                }
            });
        });
        if queue_height > 0. {
            ui.scope_builder(egui::UiBuilder::new().max_rect(queue_rect), |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(queue_rect.expand(paint_margin)));
                agents.queue.ui(
                    ui,
                    !offline
                        && !modal
                        && !agents.history_open
                        && agents.chat.queue_capacity()
                        && agents
                            .chat
                            .response_style_status(agents.config.personality)
                            .is_none(),
                    !agents.chat.recoverable(),
                );
                agents.flush_queue();
            });
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(composer), |ui| {
            if hide_composer {
                return;
            }
            ui.set_clip_rect(ui.clip_rect().intersect(composer.expand(paint_margin)));
            ui.separator();
            if unsaved > 0 {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{unsaved} unsaved · agent reads disk"))
                            .size(11.)
                            .color(palette.accent),
                    );
                    if crate::keyboard::small_button(ui, "Save all").clicked() {
                        save = true;
                    }
                });
            }
            let control_height = layout::control_height(ui);
            ui.add_enabled_ui(!agents.chat.busy && !offline, |ui| {
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), control_height), Layout::right_to_left(Align::Center), |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width().min(380.), control_height),
                        Layout::left_to_right(Align::Center),
                        |ui| discover_models = controls::buttons(ui, &mut agents.config, &agents.chat, agents.catalog.as_ref(), read_only, isolated),
                    );
                });
            });
            agents.files.ui(ui, !modal && !offline);
            let id = egui::Id::new("agent-composer");
            let shortcut = text_submit(ui, id, modal, &mut agents.composing);
            agents.composer.recall(
                ui, &mut agents.draft, id,
                !modal && !agents.composing,
            );
            let output = composer::editor(ui, &mut agents.draft, id, layout.stretch).inner;
            let at_start = output.cursor_range.is_some_and(|range| range.is_empty() && range.primary.index == 0);
            let response = output.response;
            if agents.focus && !modal {
                if ui.memory(|m| m.allows_interaction(ui.layer_id())) {
                    response.request_focus();
                    agents.focus = false;
                } else {
                    ctx.request_repaint();
                }
            }
            ui_probe::button(ui, "agent-composer", &response);
            crate::keyboard::field_target(ui, "Message", &response);
            if agents.draft.len() <= 64 * 1024 {
                ui_probe::value(ui, "agent-composer-value", &response, &agents.draft);
            }
            if agents.draft.is_empty() {
                ui_probe::button(ui, "agent-composer-empty", &response);
            }
            if agents.config.personality == before.personality
                && agents.chat.response_style_status(agents.config.personality).is_none() {
                ui_probe::button(ui, "agent-tone-ready", &response);
            }
            if at_start {
                ui_probe::button(ui, "agent-composer-at-start", &response);
            }
            if !agents.chat.busy && !agents.chat.work_pending() && !agents.chat.queue_pending()
                && !agents.queue.waiting_for_reply() && agents.queue.is_empty() && !agents.attaching
                && !agents.history_open && !offline && !modal
            {
                ui_probe::button(ui, "agent-idle", &response);
            }
            let action_height = layout::control_height(ui);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), action_height),
                Layout::right_to_left(Align::Center), |ui| {
                    background::persistence(ui, &mut agents.chat, palette, !offline && !modal);
                    let remaining = ui.available_width().max(1.);
                    ui.allocate_ui_with_layout(
                        egui::vec2(remaining, action_height),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            let enabled = !offline
                                && !modal
                                && !installing
                                && !agents.chat.stopping()
                                && !agents.chat.recoverable()
                                && agents.chat.response_style_status(agents.config.personality).is_none()
                                && agents.queue.can_push()
                                && agents.chat.queue_capacity()
                                && !agents.history_open
                                && !agents.attaching
                                && !agents.prompt_reader.as_ref().is_some_and(|reader| reader.anchoring())
                                && !agents.files.pending
                                && (!agents.draft.trim().is_empty() || !agents.files.files.is_empty())
                                && agents.draft.len() <= agent::PROMPT_LIMIT
                                && (!agents.config.provider.is_acp()
                                    || agents.argument_error.is_empty());
                            let send_button = ui
                                .add_enabled(enabled, crate::keyboard::Button::new(if agents.chat.busy || agents.queue.waiting_for_reply() || !agents.queue.is_empty() { "Queue" } else { "Send" }))
                                .on_hover_text("Enter sends when idle or queues a follow-up while working.\nShift+Enter inserts a new line.");
                            ui_probe::button(ui, "agent-send", &send_button);
                            if send_button.clicked()
                                || (shortcut && enabled)
                            {
                                send = true;
                            }
                            if agents.chat.busy {
                                let button = crate::keyboard::button(ui, if agents.chat.stopping() {
                                        "Close process"
                                    } else {
                                        "Stop"
                                    });
                                ui_probe::button(ui, "agent-stop", &button);
                                if button.clicked() { stop = true; }
                            }
                            if !agents.chat.busy
                                && agents.chat.connected()
                                && ui
                                    .add_enabled(!agents.chat.work_pending(), crate::keyboard::Button::new(if agents.chat.persistent() {
                                        "Detach"
                                    } else {
                                        "Disconnect"
                                    }))
                                    .clicked()
                            {
                                if agents.chat.persistent() {
                                    agents.chat.detach();
                                } else {
                                    agents.chat.disconnect();
                                }
                                agents.queue.clear();
                                agents
                        .chat
                        .notice("Agent disconnected. The next message starts a new conversation.");
                            }
                            let attach_button = ui.add_enabled(!offline && !modal && !agents.history_open && !agents.files.pending && agents.files.files.len() < rode_agent_core::attachment::MAX_ATTACHMENTS,
                                crate::keyboard::Button::new("Attach…")).on_hover_text("Attach images, PDF text, or UTF-8 documents. Files stay with this message, including when queued.");
                            ui_probe::button(ui, "agent-attach", &attach_button);
                            if attach_button.clicked() { attach = true; }
                            if ui.available_width() > 100. {
                                let hint = ui.add(
                                    egui::Label::new(palette.tag(if agents.composer.waiting_older() { "Loading earlier prompts…" } else { "Enter · Shift+Enter" })).truncate(),
                                )
                                .on_hover_text("Enter to send; Shift+Enter for a new line. At the start of the message, Up/Down recalls sent messages; Down returns to your draft.");
                                ui_probe::button(ui, if agents.composer.waiting_older() { "agent-prompt-recall-loading" } else { "agent-prompt-recall-ready" }, &hint);
                            }
                        },
                    );
            });
        });
        if let Some(style) = use_host_tone {
            agents.config.personality = style;
        }
        if agents.config != before {
            if !before.same_process(&agents.config) {
                agents.runtime.reset();
                agents.release_chat();
                agents.completed = 0;
            }
            agents.browser = None;
            agents.history_open = false;
            if agents.config.model() != before.model() {
                agents.config.reasoning_effort.clear();
            }
            agents.save_config();
            // A pure response-style edit must not configure operating modes or
            // revalidate unrelated ACP controls before there is another task.
            let mut execution_before = before;
            execution_before.personality = agents.config.personality;
            let style_only = execution_before == agents.config;
            if (agents.chat.native_id().is_some() || !style_only)
                && let Err(error) = agents.chat.configure_idle(&agents.config)
            {
                agents.queue.pause();
                if !style_only {
                    agents.chat.disconnect();
                }
                agents
                    .chat
                    .notice(&format!("Agent control was not applied: {error}"));
            }
        }
        let refresh = stop && agents.chat.stopping() && agents.chat.busy;
        if new_chat {
            agents.start_new_chat();
        }
        if stop {
            agents.queue.pause();
            if agents.chat.stopping() {
                agents.chat.completed += u64::from(agents.chat.busy);
                agents.chat.disconnect();
                agents.chat.notice("Agent process closed.");
            } else {
                agents.chat.cancel();
            }
        }
        if let Some(answer) = answer
            && let Err(error) = agents.chat.answer(answer)
        {
            agents.chat.notice(&error.to_string());
        }
        Some(Actions {
            save,
            attach,
            discover_models,
            send,
            history: history_action,
            refresh_documents: refresh,
            running,
            recover,
        })
    }
}

impl Agents {
    /// Apply a history action without access to document, workspace or session mutation.
    pub(super) fn apply_history(
        &mut self,
        history_action: history::Action,
        environment: &controller::Environment,
        enabled: bool,
        ctx: &egui::Context,
    ) {
        if history_action.close {
            self.browser = None;
            self.history_open = false;
            self.history_delete = None;
        } else if history_action.refresh {
            self.history_delete = None;
            if enabled {
                self.open_history(environment, ctx);
            }
        } else if let Some(browser) = &mut self.browser {
            let result = if let Some(scope) = history_action.scope {
                self.history_delete = None;
                self.history_scope = scope;
                browser.set_scope(scope)
            } else if let Some(selected) = history_action.delete {
                let protected = self
                    .chat
                    .conversation_id()
                    .map(str::to_owned)
                    .into_iter()
                    .collect();
                let id = match &selected {
                    history::Delete::One { id, .. } => Some(id.as_str()),
                    history::Delete::All => None,
                };
                browser.delete(id, protected)
            } else if let Some(id) = history_action.resume {
                browser.resume(&id)
            } else if history_action.more {
                browser.load_more()
            } else {
                Ok(())
            };
            if let Err(error) = result {
                self.chat.notice(&error.to_string());
            }
        }
    }

    /// Explicit model discovery uses captured execution authority and no project mutation.
    pub(super) fn discover_models(
        &mut self,
        environment: &controller::Environment,
        ctx: &egui::Context,
    ) {
        let context = ctx.clone();
        self.catalog = Some(agent::controls::discover(
            self.launch(environment),
            Arc::new(move || context.request_repaint()),
        ));
    }

    /// Reconnect the current agent view while keeping drafts and queued work with its owner.
    pub(super) fn recover(
        &mut self,
        environment: &controller::Environment,
        fresh: bool,
        ctx: &egui::Context,
    ) {
        let launch = self.launch(environment);
        let context = ctx.clone();
        if let Err(error) =
            self.chat
                .recover(launch, fresh, Arc::new(move || context.request_repaint()))
        {
            self.chat
                .notice(&format!("Agent recovery failed: {error:#}"));
        }
    }
}

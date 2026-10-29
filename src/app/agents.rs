//! Project-scoped agent chat and preferences. Processes/transcripts are kept
//! out of app preferences; authentication remains with the installed agent.
//!
//! The first submitted message starts a process. Workspace/settings/feature changes stop the
//! old process; approvals remain bounded by the enclosing sandbox. Replies may refresh clean
//! disk-backed documents but never overwrite dirty buffers.
use super::*;
use crate::agent::{
    self, Config, Launch, Preferences, Provider,
    protocol::{Chat, Role},
};
use serde_json::json;

pub(super) struct Agents {
    pub chat: Chat,
    preferences: Preferences,
    config: Config,
    key: String,
    draft: String,
    arguments: String,
    argument_error: String,
    settings: bool,
    pending: bool,
    pub focus: bool,
    completed: u64,
    local_root: String,
}
impl Agents {
    pub(super) fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let preferences = storage
            .and_then(|s| s.get_string("agent_preferences"))
            .filter(|s| s.len() <= 256 * 1024)
            .and_then(|s| serde_json::from_str::<Preferences>(&s).ok())
            .filter(|p| p.len() <= 128)
            .unwrap_or_default();
        Self {
            chat: Chat::default(),
            preferences,
            config: Config::default(),
            key: String::new(),
            draft: String::new(),
            arguments: "[]".into(),
            argument_error: String::new(),
            settings: false,
            pending: false,
            focus: false,
            completed: 0,
            local_root: std::env::current_dir()
                .unwrap_or_else(|_| files::expand_home("~"))
                .to_string_lossy()
                .into(),
        }
    }
    pub(super) fn store(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(data) = serde_json::to_string(&self.preferences) {
            storage.set_string("agent_preferences", data);
        }
    }
    fn save_config(&mut self) {
        if !self.key.is_empty() {
            self.preferences
                .insert(self.key.clone(), self.config.clone());
            while self.preferences.len() > 128
                || serde_json::to_vec(&self.preferences).is_ok_and(|v| v.len() > 256 * 1024)
            {
                let key = self
                    .preferences
                    .keys()
                    .find(|key| *key != &self.key)
                    .cloned();
                if let Some(key) = key {
                    self.preferences.remove(&key);
                } else {
                    break;
                }
            }
            self.pending = true;
        }
    }
}

impl EditorApp {
    fn agent_location(&self) -> (String, String, String) {
        let root = self
            .workspace
            .as_ref()
            .map(|w| w.root.clone())
            .unwrap_or_else(|| self.agents.local_root.clone());
        let host = self
            .workspace
            .as_ref()
            .and_then(|w| w.remote.as_ref())
            .map(|r| {
                format!(
                    "SSH {}{}",
                    r.profile.endpoint(),
                    if r.profile.port.is_empty() {
                        String::new()
                    } else {
                        format!(":{}", r.profile.port)
                    }
                )
            })
            .unwrap_or_else(|| "LOCAL".into());
        let key = serde_json::to_string(&(
            host.as_str(),
            &root,
            self.workspace
                .as_ref()
                .and_then(|w| w.remote.as_ref())
                .map(|r| r.profile.config.as_str())
                .unwrap_or_default(),
        ))
        .unwrap_or_default();
        (root, host, key)
    }

    pub(super) fn poll_agents(&mut self, ctx: &egui::Context) {
        let (_, _, key) = self.agent_location();
        if self.agents.key != key {
            self.agents.save_config();
            self.agents.chat = Chat::default();
            self.agents.completed = 0;
            self.agents.draft.clear();
            self.agents.config = self
                .agents
                .preferences
                .get(&key)
                .cloned()
                .unwrap_or_default();
            self.agents.arguments = serde_json::to_string(&self.agents.config.acp_arguments)
                .unwrap_or_else(|_| "[]".into());
            self.agents.argument_error.clear();
            self.agents.key = key;
        }
        self.agents.chat.poll();
        if self.agents.completed != self.agents.chat.completed {
            self.agents.completed = self.agents.chat.completed;
            self.reload_clean_documents(ctx);
        }
        if self.agents.chat.busy {
            // Needed only for startup/cancellation deadlines. Streamed output
            // wakes the window directly; idle agent sessions do not poll it.
            if !self.agents.chat.connected() || self.agents.chat.stopping() {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
    }

    pub(super) fn stop_project_agent(&mut self) {
        if self.agents.chat.connected() || self.agents.chat.busy {
            self.agents.chat.disconnect();
            self.agents.chat.notice(
                "Project session changed. The next message starts a new agent conversation.",
            );
        }
    }

    pub(super) fn persist_agent_preferences(
        &mut self,
        storage: Option<&mut (dyn eframe::Storage + 'static)>,
    ) {
        if !self.agents.pending {
            return;
        }
        self.agents.pending = false;
        if let Some(storage) = storage {
            self.agents.store(storage);
            storage.flush();
        } else {
            self.notify(
                "Agent settings applied, but preference storage is unavailable.",
                true,
            );
        }
    }

    pub(super) fn agent_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let (root, host, _) = self.agent_location();
        let mode = self
            .workspace
            .as_ref()
            .map(|w| w.plan.label())
            .unwrap_or("NORMAL MODE");
        let offline = self
            .workspace
            .as_ref()
            .is_some_and(|w| !w.ready || (w.remote.is_some() && !w.connected));
        let unsaved = self.documents.iter().filter(|d| d.dirty()).count();
        let modal = self.dialog != Dialog::None || self.pending.is_some() || self.workbench_modal();
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let composer_height = if unsaved > 0 { 132. } else { 110. };
        let composer = egui::Rect::from_min_max(
            egui::pos2(
                rect.left(),
                (rect.bottom() - composer_height).max(rect.top()),
            ),
            rect.max,
        );
        let content = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.right(), (composer.top() - 6.).max(rect.top())),
        );
        let mut send = false;
        let mut stop = false;
        let mut new_chat = false;
        let mut save = false;
        let mut answer = None;
        let agents = &mut self.agents;
        let before = agents.config.clone();
        ui.scope_builder(egui::UiBuilder::new().max_rect(content),|ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(content));
            ui.horizontal(|ui| {
                let old = agents.config.provider;
                let width = (ui.available_width()-160.).max(80.);
                ui.allocate_ui_with_layout(egui::vec2(width,26.), Layout::left_to_right(Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("agent-provider").width(width)
                        .truncate().selected_text(agents.config.provider.label()).show_ui(ui,|ui| {
                            for provider in [Provider::Codex,Provider::Local,Provider::Acp] {
                                ui.add_enabled_ui(!agents.chat.busy,|ui|ui.selectable_value(&mut agents.config.provider,provider,provider.label()));
                            }
                        }).response.on_hover_text(agents.config.provider.label());
                });
                if old != agents.config.provider { agents.settings = true; }
                if icons::small(ui,Icon::Settings,"Agent settings").clicked() { agents.settings = !agents.settings; }
                if ui.small_button("New chat").on_hover_text("Stop this agent and start a fresh conversation").clicked() { new_chat = true; }
            });
            ui.add(egui::Label::new(palette.tag(format!("{host} · {mode}"))).truncate());
            ui.add(egui::Label::new(palette.tag(&root)).truncate()).on_hover_text(&root);
            if agents.settings {
                egui::ScrollArea::vertical().id_salt("agent-settings-scroll")
                    .max_height((ui.available_height()-90.).clamp(60.,240.)).show(ui,|ui| {
                    ui.add_enabled_ui(!agents.chat.busy,|ui| {
                        if agents.config.provider == Provider::Acp {
                            ui.label("ACP executable on this host");
                            ui.add(egui::TextEdit::singleline(&mut agents.config.acp_program).desired_width(f32::INFINITY).hint_text("agent executable or absolute path").char_limit(4096));
                            ui.label("Arguments (JSON array)");
                            if ui.add(egui::TextEdit::singleline(&mut agents.arguments).desired_width(f32::INFINITY).hint_text("[\"acp\"]").char_limit(16384)).changed() {
                                match serde_json::from_str::<Vec<String>>(&agents.arguments) {
                                    Ok(args) => { agents.config.acp_arguments=args; agents.argument_error.clear(); }
                                    Err(_) => agents.argument_error="Use a JSON array of argument strings.".into(),
                                }
                            }
                            if !agents.argument_error.is_empty() { ui.colored_label(palette.error,&agents.argument_error); }
                            ui.label(palette.tag("Agent must provide its own file and command tools."));
                        } else {
                            ui.label("Codex executable on this host");
                            ui.add(egui::TextEdit::singleline(&mut agents.config.codex).desired_width(f32::INFINITY).hint_text("codex or /path/to/codex").char_limit(4096));
                            if agents.config.provider == Provider::Local {
                                ui.label("Local model API base URL");
                                ui.add(egui::TextEdit::singleline(&mut agents.config.local_url).desired_width(f32::INFINITY).char_limit(2048)).on_hover_text("An Ollama /v1 or compatible Responses endpoint reachable from the project host. localhost means this host.");
                                ui.label("Installed model name");
                                ui.add(egui::TextEdit::singleline(&mut agents.config.local_model).desired_width(f32::INFINITY).hint_text("Model already available on the local server").char_limit(256));
                            } else {
                                ui.label("Model (blank: Codex default)");
                                ui.add(egui::TextEdit::singleline(&mut agents.config.model).desired_width(f32::INFINITY).char_limit(256));
                            }
                            ui.checkbox(&mut agents.config.read_only,"Start Codex tools in read-only mode");
                        }
                    });
                    ui.label(RichText::new("Settings are saved for this project. Changes start a new conversation. Use the agent's login or environment for credentials.").small().color(palette.muted));
                });
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(if agents.chat.status.is_empty() {"Ready to start"} else {&agents.chat.status}).size(12.).color(if offline {palette.error}else{palette.accent}));
            });
            if let Some(prompt) = agents.chat.prompts.front_mut() {
                egui::Frame::new().fill(palette.raised).stroke(Stroke::new(1.0_f32,palette.accent)).inner_margin(8).corner_radius(4).show(ui,|ui| {
                    egui::ScrollArea::vertical().id_salt("agent-request").max_height((ui.available_height()*0.65).clamp(60.,200.)).show(ui,|ui| {
                        ui.label(RichText::new(&prompt.title).strong().color(palette.accent));
                        if !prompt.summary.is_empty() {
                            ui.add(egui::Label::new(RichText::new(&prompt.summary).monospace().size(12.)).wrap().selectable(true));
                        }
                        if prompt.questions.is_empty() {
                            ui.collapsing("Details",|ui| { ui.add(egui::Label::new(RichText::new(&prompt.detail).monospace().size(11.)).wrap().selectable(true)); });
                        }
                        for (index, question) in prompt.questions.iter_mut().enumerate() {
                            ui.push_id(index,|ui| {
                                ui.label(&question.text);
                                for option in &question.options { if ui.selectable_label(question.answer==*option,option).clicked() {question.answer=option.clone();} }
                                ui.add(egui::TextEdit::singleline(&mut question.answer).password(question.secret).desired_width(f32::INFINITY).char_limit(8192));
                            });
                        }
                        if !prompt.questions.is_empty() && ui.button("Send answers").clicked() {
                            let values: serde_json::Map<String,serde_json::Value> = prompt.questions.iter().map(|q|(q.id.clone(),json!({"answers":[q.answer]}))).collect();
                            answer=Some(json!({"answers":values}));
                        }
                        ui.horizontal_wrapped(|ui| {
                            for choice in &prompt.choices { if ui.button(&choice.label).clicked() {answer=Some(choice.result.clone());} }
                        });
                    });
                });
            }
            egui::ScrollArea::vertical().id_salt("agent-transcript").stick_to_bottom(true).auto_shrink([false,false]).max_height(ui.available_height().max(1.)).show(ui,|ui| {
                if agents.chat.messages.is_empty() {
                    ui.add_space(8.);
                    ui.label(RichText::new("Work on this project with an agent").strong());
                    ui.add(egui::Label::new("Ask a question, request a change, or investigate an issue. The agent works in the folder above.").wrap());
                    if agents.config.provider == Provider::Local { ui.add(egui::Label::new("Start Ollama or a compatible local server and select an installed model in settings.").wrap()); }
                    else { ui.add(egui::Label::new("Install and authenticate your agent on this host before sending a message.").wrap()); }
                }
                for message in &agents.chat.messages {
                    ui.push_id(&message.id,|ui| {
                        if message.role == Role::Tool {
                            ui.collapsing(RichText::new(&message.title).monospace().size(12.).color(palette.muted),|ui| {
                                ui.add(egui::Label::new(RichText::new(&message.text).monospace().size(11.)).wrap().selectable(true));
                            });
                        } else {
                            egui::Frame::new().fill(if message.role == Role::User {palette.raised}else{palette.background}).inner_margin(8).corner_radius(4).show(ui,|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&message.title).strong().size(11.).color(if message.role == Role::Notice {palette.muted}else{palette.accent}));
                                    if message.role == Role::Assistant && icons::small(ui,Icon::Copy,"Copy response").clicked() {ctx.copy_text(message.text.clone());}
                                });
                                render_message(ui,&message.text,palette);
                            });
                        }
                    });
                    ui.add_space(5.);
                }
            });
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(composer), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(composer));
            ui.separator();
            if unsaved > 0 {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{unsaved} unsaved · agent reads disk"))
                            .size(11.)
                            .color(palette.accent),
                    );
                    if ui.small_button("Save all").clicked() {
                        save = true;
                    }
                });
            }
            let id = egui::Id::new("agent-composer");
            let shortcut = !modal
                && ui.memory(|m| m.has_focus(id))
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter));
            let response = ui.add(
                egui::TextEdit::multiline(&mut agents.draft)
                    .id(id)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Message the agent…")
                    .char_limit(agent::PROMPT_LIMIT),
            );
            if agents.focus && !modal {
                if ui.memory(|m| m.allows_interaction(ui.layer_id())) {
                    response.request_focus();
                    agents.focus = false;
                } else {
                    ctx.request_repaint();
                }
            }
            ui.horizontal(|ui| {
                let enabled = !offline
                    && !agents.chat.busy
                    && !agents.draft.trim().is_empty()
                    && agents.draft.len() <= agent::PROMPT_LIMIT
                    && (agents.config.provider != Provider::Acp
                        || agents.argument_error.is_empty());
                if ui
                    .add_enabled(enabled, egui::Button::new("Send"))
                    .on_hover_text("Send · Ctrl+Enter")
                    .clicked()
                    || (shortcut && enabled)
                {
                    send = true;
                }
                if agents.chat.busy
                    && ui
                        .button(if agents.chat.stopping() {
                            "Close process"
                        } else {
                            "Stop"
                        })
                        .clicked()
                {
                    stop = true;
                }
                if !agents.chat.busy && agents.chat.connected() && ui.button("Disconnect").clicked()
                {
                    agents.chat.disconnect();
                    agents
                        .chat
                        .notice("Agent disconnected. The next message starts a new conversation.");
                }
                ui.label(palette.tag("Ctrl+Enter"));
            });
        });
        if agents.config != before {
            agents.save_config();
        }
        let refresh = (new_chat || (stop && agents.chat.stopping())) && agents.chat.busy;
        if new_chat {
            agents.chat = Chat::default();
            agents.completed = 0;
            agents.focus = true;
        }
        if stop {
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
        if save {
            let files: Vec<_> = self
                .documents
                .iter()
                .filter(|d| d.dirty() && d.path.is_some())
                .map(|d| (d.id, d.path.clone().unwrap_or_default()))
                .collect();
            for (id, path) in files {
                self.save_document(id, path, false, ctx);
            }
            if self.documents.iter().any(|d| d.path.is_none()) {
                self.notify(
                    "Save untitled files with Ctrl+S to choose their paths.",
                    false,
                );
            }
        }
        if send {
            self.send_agent(ctx);
        }
        if refresh {
            self.reload_clean_documents(ctx);
        }
    }

    fn send_agent(&mut self, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Agents) {
            return;
        }
        let (root, _, _) = self.agent_location();
        let (plan, remote) = self
            .workspace
            .as_ref()
            .map(|w| (w.plan.clone(), w.remote.clone()))
            .unwrap_or_else(|| {
                (
                    sandbox::Plan {
                        root,
                        ..Default::default()
                    },
                    None,
                )
            });
        let launch = Launch {
            config: self.agents.config.clone(),
            plan,
            remote,
            python: self.agent_python(),
        };
        let context = ctx.clone();
        match self.agents.chat.submit(
            launch,
            self.agents.draft.clone(),
            Arc::new(move || context.request_repaint()),
        ) {
            Ok(()) => {
                self.agents.draft.clear();
                self.agents.focus = true;
            }
            Err(error) => {
                self.agents.chat.notice(&format!("{error:#}"));
                self.agents.settings = true;
            }
        }
    }
}

fn render_message(ui: &mut egui::Ui, text: &str, palette: theme::Palette) {
    for (index, part) in text.split("```").enumerate() {
        if part.is_empty() {
            continue;
        }
        if index % 2 == 0 {
            ui.add(egui::Label::new(part.trim()).wrap().selectable(true));
        } else {
            let code = part.split_once('\n').map(|(_, code)| code).unwrap_or(part);
            egui::Frame::new()
                .fill(palette.raised)
                .inner_margin(6)
                .corner_radius(3)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(code.trim_end()).monospace().size(12.))
                            .wrap()
                            .selectable(true),
                    );
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Storage(BTreeMap<String, String>);
    impl eframe::Storage for Storage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.into(), value);
        }
        fn flush(&mut self) {}
    }
    #[test]
    fn project_preferences_restore_without_persisting_conversation_content() {
        let mut agents = Agents::load(None);
        agents.key = "host /project".into();
        agents.config.provider = Provider::Local;
        agents.config.local_model = "installed-model".into();
        agents.chat.notice("private conversation content");
        agents.save_config();
        let mut storage = Storage::default();
        agents.store(&mut storage);
        let restored = Agents::load(Some(&storage));
        assert_eq!(
            restored.preferences["host /project"].local_model,
            "installed-model"
        );
        assert!(!storage.0["agent_preferences"].contains("private conversation content"));
    }
}

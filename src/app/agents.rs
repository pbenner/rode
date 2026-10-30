//! Project-scoped agent chat and preferences. Processes/transcripts are kept
//! out of app preferences; authentication remains with the installed agent.
//!
//! A submitted message or explicit History starts a process. Workspace/settings/feature changes stop the
//! old process; approvals remain bounded by the enclosing sandbox. Replies may refresh clean
//! disk-backed documents but never overwrite dirty buffers.
use super::*;
use crate::agent::{
    self, Config, Launch, Preferences, Provider,
    protocol::{Chat, Role},
};
use serde_json::json;
mod controls;
mod history;

pub(super) struct Agents {
    pub chat: Chat,
    preferences: Preferences,
    config: Config,
    key: String,
    draft: String,
    composing: bool,
    arguments: String,
    argument_error: String,
    settings: bool,
    pending: bool,
    pub focus: bool,
    completed: u64,
    local_root: String,
    browser: Option<agent::history::Browser>,
    history_open: bool,
    history_filter: String,
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
            composing: false,
            arguments: "[]".into(),
            argument_error: String::new(),
            settings: false,
            pending: false,
            focus: false,
            completed: 0,
            browser: None,
            history_open: false,
            history_filter: String::new(),
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
            self.agents.browser = None;
            self.agents.history_open = false;
            self.agents.history_filter.clear();
            self.agents.save_config();
            self.agents.chat = Chat::default();
            self.agents.completed = 0;
            self.agents.draft.clear();
            self.agents.composing = false;
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
        if self.workbench.panel != Some(Panel::Agent)
            || !self.navigation.visible(navigation::Pane::Tools)
            || self
                .agents
                .browser
                .as_ref()
                .is_some_and(|browser| !browser.uses_interpreter(&self.agent_python()))
        {
            self.agents.browser = None;
            self.agents.history_open = false;
            self.agents.composing = false;
        }
        self.agents.chat.poll();
        if let Some(browser) = &mut self.agents.browser {
            browser.poll();
            if browser.busy {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
            if !self.agents.chat.busy
                && let Some(restored) = browser.take_restored()
            {
                self.agents.chat.restore(restored);
                self.agents.browser = None;
                self.agents.history_open = false;
                self.agents.focus = true;
            }
        }
        if self.agents.completed != self.agents.chat.completed {
            self.agents.completed = self.agents.chat.completed;
            self.reload_clean_documents(ctx);
        }
        if self.agents.chat.busy {
            // Needed only for startup/cancellation deadlines. Streamed output
            // wakes the window directly; idle agent sessions do not poll it.
            if self.agents.chat.has_deadline() {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
    }

    pub(super) fn stop_project_agent(&mut self) {
        self.agents.browser = None;
        self.agents.history_open = false;
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
        // Give history the full compact panel; retain Stop while a turn is running.
        let hide_composer = self.agents.history_open && !self.agents.chat.busy;
        let composer_height = if hide_composer {
            0.
        } else if unsaved > 0 {
            162.
        } else {
            140.
        };
        let read_only = self
            .workspace
            .as_ref()
            .is_some_and(|w| w.plan.project_read_only);
        let isolated = self.workspace.as_ref().is_some_and(|w| w.plan.active);
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
        let mut history_action = history::Action::default();
        let agents = &mut self.agents;
        let before = agents.config.clone();
        ui.scope_builder(egui::UiBuilder::new().max_rect(content),|ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(content));
            ui.horizontal(|ui| {
                let old = agents.config.provider;
                let width = (ui.available_width()-176.).max(80.);
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
                controls::personality(ui, &mut agents.config, !agents.chat.busy && !offline);
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
                            if ui.small_button("Reset saved agent controls").clicked() {
                                agents.config.acp_options.clear();
                                agents.chat.disconnect();
                                agents.chat.notice("Agent controls reset. The next message starts a fresh conversation.");
                            }
                        } else {
                            ui.label("Codex executable (auto: discover)");
                            ui.add(egui::TextEdit::singleline(&mut agents.config.codex).desired_width(f32::INFINITY).hint_text("auto, or /path/to/codex").char_limit(4096)).on_hover_text("Install the optional rode-codex package or use an installed Codex. Sign in locally with rode --codex login. SSH projects use Codex and credentials on that host.");
                            if agents.config.provider == Provider::Local {
                                ui.label("Local model API base URL");
                                ui.add(egui::TextEdit::singleline(&mut agents.config.local_url).desired_width(f32::INFINITY).char_limit(2048)).on_hover_text("An Ollama /v1 or compatible Responses endpoint reachable from the project host. localhost means this host.");
                                ui.label("Installed model name");
                                ui.add(egui::TextEdit::singleline(&mut agents.config.local_model).desired_width(f32::INFINITY).hint_text("Model already available on the local server").char_limit(256));
                            } else {
                                ui.label("Model (blank: Codex default)");
                                ui.add(egui::TextEdit::singleline(&mut agents.config.model).desired_width(f32::INFINITY).char_limit(256));
                            }
                        }
                    });
                    ui.label(RichText::new("Settings are saved for this project. Model and Access controls are below. Executable/provider changes start a new conversation. Use the agent's login or environment for credentials.").small().color(palette.muted));
                });
            }
            ui.horizontal(|ui| { ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(!offline,egui::Button::new("History").small().selected(agents.history_open)).on_hover_text("Browse saved conversations on this host for this project").clicked() {
                    agents.history_open = !agents.history_open;
                    if agents.history_open {history_action.refresh=true;} else {agents.browser=None;}
                }
                ui.add(egui::Label::new(RichText::new(if agents.chat.status.is_empty() {"Ready to start"} else {&agents.chat.status}).size(12.).color(if offline {palette.error}else{palette.accent})).truncate());
            }); });
            if agents.history_open {
                let panel=history::ui(ui,agents.browser.as_ref(),&mut agents.history_filter,!agents.chat.busy && !modal);
                history_action.refresh |= panel.refresh;
                history_action.more=panel.more;
                history_action.resume=panel.resume;
                history_action.close=panel.close;
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
                    else if agents.config.provider == Provider::Codex { ui.add(egui::Label::new("Use the optional rode-codex package or an installed Codex. Authenticate on the project host before sending a message.").wrap()); }
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
            if hide_composer {
                return;
            }
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
            ui.add_enabled_ui(!agents.chat.busy && !offline, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 26.),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        controls::buttons(
                            ui,
                            &mut agents.config,
                            &agents.chat,
                            read_only,
                            isolated,
                        );
                    },
                );
            });
            let id = egui::Id::new("agent-composer");
            let shortcut = composer_submit(ui, id, modal, &mut agents.composing);
            let response = ui.add(
                egui::TextEdit::multiline(&mut agents.draft)
                    .id(id)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Message the agent…")
                    .return_key(egui::KeyboardShortcut::new(
                        egui::Modifiers::SHIFT,
                        Key::Enter,
                    ))
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
                    && !agents.history_open
                    && !agents.draft.trim().is_empty()
                    && agents.draft.len() <= agent::PROMPT_LIMIT
                    && (agents.config.provider != Provider::Acp
                        || agents.argument_error.is_empty());
                if ui
                    .add_enabled(enabled, egui::Button::new("Send"))
                    .on_hover_text("Send · Enter\nShift+Enter for a new line")
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
                ui.add(egui::Label::new(palette.tag("Enter · Shift+Enter")).truncate())
                    .on_hover_text("Enter to send; Shift+Enter for a new line");
            });
        });
        if agents.config != before {
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
            if execution_before != agents.config
                && let Err(error) = agents.chat.configure_idle(&agents.config)
            {
                agents.chat.disconnect();
                agents
                    .chat
                    .notice(&format!("Agent control was not applied: {error}"));
            }
        }
        let refresh = (new_chat || (stop && agents.chat.stopping())) && agents.chat.busy;
        if new_chat {
            agents.browser = None;
            agents.history_open = false;
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
        if history_action.close {
            self.agents.browser = None;
            self.agents.history_open = false;
        } else if history_action.refresh {
            self.open_agent_history(ctx);
        } else if let Some(browser) = &mut self.agents.browser {
            let result = if let Some(id) = history_action.resume {
                browser.resume(&id)
            } else if history_action.more {
                browser.load_more()
            } else {
                Ok(())
            };
            if let Err(error) = result {
                self.agents.chat.notice(&error.to_string());
            }
        }
        if refresh {
            self.reload_clean_documents(ctx);
        }
    }

    fn send_agent(&mut self, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Agents) {
            return;
        }
        let launch = self.agent_launch();
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

    fn open_agent_history(&mut self, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Agents) {
            return;
        }
        let launch = self.agent_launch();
        let context = ctx.clone();
        match agent::history::Browser::open(launch, Arc::new(move || context.request_repaint())) {
            Ok(browser) => {
                self.agents.browser = Some(browser);
                self.agents.history_open = true;
            }
            Err(error) => {
                self.agents
                    .chat
                    .notice(&format!("Cannot open history: {error:#}"));
                self.agents.history_open = false;
            }
        }
    }

    fn agent_launch(&self) -> Launch {
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
        Launch {
            config: self.agents.config.clone(),
            plan,
            remote,
            python: self.agent_python(),
        }
    }
}

/// Consume only a fresh, unmodified Enter in the focused composer. egui's
/// consume_key ignores extra Shift/Alt, which would also consume newlines.
/// IME confirmation belongs to the text editor, including a commit+Enter frame.
fn composer_submit(ui: &mut egui::Ui, id: egui::Id, modal: bool, composing: &mut bool) -> bool {
    if modal || !ui.memory(|m| m.has_focus(id)) {
        *composing = false;
        return false;
    }
    ui.input_mut(|input| {
        let mut ime_frame = *composing;
        for event in &input.events {
            if let egui::Event::Ime(event) = event {
                ime_frame = true;
                *composing = matches!(event, egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_));
            }
        }
        let mut send = false;
        input.events.retain(|event| {
            if !ime_frame
                && let egui::Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                    repeat,
                    ..
                } = event
            {
                send |= !repeat;
                false
            } else {
                true
            }
        });
        send
    })
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

    #[test]
    fn composer_sends_enter_preserves_multiline_text_and_confirms_ime_without_sending() {
        let ctx = egui::Context::default();
        let mut draft = String::new();
        let mut composing = false;
        let id = egui::Id::new("agent-composer");
        let mut frame = |mut events: Vec<egui::Event>, focused: bool, modal: bool| {
            events.push(egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            let mut send = false;
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.memory_mut(|m| {
                            m.request_focus(if focused { id } else { id.with("other") })
                        });
                        send = composer_submit(ui, id, modal, &mut composing);
                        ui.add(egui::TextEdit::multiline(&mut draft).id(id).return_key(
                            egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, Key::Enter),
                        ));
                    });
                },
            );
            (send, draft.clone())
        };
        let enter = |modifiers, repeat| egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        };
        assert!(!frame(vec![egui::Event::Text("one".into())], true, false).0);
        assert_eq!(
            frame(vec![enter(egui::Modifiers::SHIFT, false)], true, false),
            (false, "one\n".into())
        );
        assert_eq!(
            frame(vec![egui::Event::Paste("two\nthree".into())], true, false),
            (false, "one\ntwo\nthree".into())
        );
        assert_eq!(
            frame(vec![enter(egui::Modifiers::NONE, false)], true, false),
            (true, "one\ntwo\nthree".into())
        );
        // egui derives repeat from its held-key state, not the raw event flag.
        ctx.input_mut(|input| {
            input.keys_down.insert(Key::Enter);
        });
        assert!(!frame(vec![enter(egui::Modifiers::NONE, true)], true, false).0);
        assert!(!frame(vec![enter(egui::Modifiers::NONE, false)], true, true).0);
        assert!(!frame(vec![enter(egui::Modifiers::NONE, false)], false, false).0);
        assert!(!frame(vec![egui::Event::Ime(egui::ImeEvent::Enabled)], true, false).0);
        assert!(
            !frame(
                vec![
                    egui::Event::Ime(egui::ImeEvent::Preedit("候".into())),
                    enter(egui::Modifiers::NONE, false)
                ],
                true,
                false
            )
            .0
        );
        assert!(
            !frame(
                vec![
                    egui::Event::Ime(egui::ImeEvent::Commit("候".into())),
                    enter(egui::Modifiers::NONE, false)
                ],
                true,
                false
            )
            .0
        );
        assert!(frame(vec![enter(egui::Modifiers::NONE, false)], true, false).0);
    }

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

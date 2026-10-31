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
pub(super) mod attachments;
mod background;
mod composer;
mod controls;
mod history;
mod message;
mod queue;
mod runtime;

pub(super) struct Agents {
    pub chat: Chat,
    preferences: Preferences,
    config: Config,
    key: String,
    draft: String,
    composing: bool,
    composer: composer::State,
    queue: queue::Queue,
    pub(super) files: attachments::State,
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
    history_delete: Option<history::Delete>,
    runtime: runtime::Runtime,
    running: background::Running,
    retired: Vec<Chat>,
    attaching: bool,
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
            composer: composer::State::default(),
            queue: queue::Queue::default(),
            files: attachments::State::default(),
            arguments: "[]".into(),
            argument_error: String::new(),
            settings: false,
            pending: false,
            focus: false,
            completed: 0,
            browser: None,
            history_open: false,
            history_filter: String::new(),
            history_delete: None,
            runtime: runtime::Runtime::default(),
            running: Default::default(),
            retired: vec![],
            attaching: false,
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
    fn flush_queue(&mut self) {
        if !self.queue.hosted() {
            return;
        }
        while self.chat.queue_capacity() {
            let Some(operation) = self.queue.command().cloned() else {
                break;
            };
            match self.chat.queue_action(operation) {
                Ok(()) => self.queue.command_sent(),
                Err(error) => {
                    self.chat
                        .notice(&format!("Queue changes were not saved: {error:#}"));
                    break;
                }
            }
        }
    }
    fn replace_chat(&mut self) {
        self.release_chat();
        self.files.clear();
    }
    fn release_chat(&mut self) {
        self.flush_queue();
        self.composer.clear_history();
        self.queue.clear();
        let old = std::mem::take(&mut self.chat);
        if old.work_pending() {
            old.cancel_startup_for_exit();
            self.retired.push(old);
        }
        self.attaching = false;
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
        self.agents.runtime.poll();
        let (_, _, key) = self.agent_location();
        if self.agents.key != key {
            self.agents.runtime.reset();
            self.agents.browser = None;
            self.agents.history_open = false;
            self.agents.history_filter.clear();
            self.agents.history_delete = None;
            self.agents.save_config();
            self.agents.replace_chat();
            self.agents.running = Default::default();
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
        if !self.tools_visible()
            || self.workbench.panel != Some(Panel::Agent)
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
        let mut retirement_error = None;
        for chat in &mut self.agents.retired {
            chat.poll();
            if let Some(error) = chat.take_persistence_error() {
                retirement_error = Some(error);
            }
        }
        self.agents.retired.retain(Chat::work_pending);
        if let Some(error) = retirement_error {
            self.abort_session_quit(error, ctx);
        }
        let was_busy = self.agents.chat.busy;
        self.agents.chat.poll();
        if let Some(error) = self.agents.chat.take_persistence_error() {
            self.abort_session_quit(error, ctx);
        }
        if self.agents.attaching
            && let Some(config) = self.agents.chat.attached_config().cloned()
        {
            self.agents.config = config;
            self.agents.composer.restore_history(
                self.agents
                    .chat
                    .messages
                    .iter()
                    .filter(|message| message.role == Role::User)
                    .map(|message| message.text.clone()),
            );
            self.agents.arguments = serde_json::to_string(&self.agents.config.acp_arguments)
                .unwrap_or_else(|_| "[]".into());
            self.agents.save_config();
            self.agents.attaching = false;
        }
        self.poll_background_agents(ctx);
        if let Some(browser) = &mut self.agents.browser {
            browser.poll();
            if browser.busy {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
            if !self.agents.chat.busy
                && let Some(restored) = browser.take_restored()
            {
                self.agents.replace_chat();
                self.agents.chat.restore(restored);
                self.agents.composer.restore_history(
                    self.agents
                        .chat
                        .messages
                        .iter()
                        .filter(|message| message.role == Role::User)
                        .map(|message| message.text.clone()),
                );
                self.agents.browser = None;
                self.agents.history_open = false;
                self.agents.focus = true;
            }
        }
        if !self.agents.chat.queue_pending()
            && self.agents.queue.command().is_none()
            && let Some(view) = self.agents.chat.queue_view()
        {
            self.agents.queue.sync(view, self.agents.chat.busy);
        }
        if self.agents.completed != self.agents.chat.completed {
            self.agents.completed = self.agents.chat.completed;
            self.agents.queue.finished();
            if !self.agents.chat.last_turn_succeeded {
                self.agents.queue.pause();
            }
            self.reload_clean_documents(ctx);
        }
        if (was_busy || self.agents.queue.waiting_for_reply())
            && !self.agents.chat.busy
            && !self.agents.chat.connected()
        {
            self.agents.queue.finished();
            self.agents.queue.pause();
        }
        self.agents.flush_queue();
        self.dispatch_agent_queue(ctx);
        if self.agents.chat.work_pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
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
        self.agents.queue.clear();
        self.agents.runtime.reset();
        self.agents.browser = None;
        self.agents.history_open = false;
        if self.agents.chat.connected() || self.agents.chat.busy {
            self.agents.chat.disconnect();
            self.agents.chat.notice(
                "Project session changed. The next message starts a new agent conversation.",
            );
        }
    }

    pub(super) fn leave_project_agent(&mut self) {
        self.agents.runtime.reset();
        self.agents.browser = None;
        self.agents.history_open = false;
        self.agents.replace_chat();
    }

    /// Clean exit lets owned setup workers release temporary files and locks.
    pub(super) fn cancel_agent_installation(&self) {
        self.agents.runtime.cancel_all();
        self.agents.chat.cancel_startup_for_exit();
        for chat in &self.agents.retired {
            chat.cancel_startup_for_exit();
        }
    }

    pub(super) fn agent_installation_pending(&self) -> bool {
        self.agents.runtime.pending()
            || self.agents.running.mutating
            || self.agents.chat.work_pending()
            || self.agents.retired.iter().any(Chat::work_pending)
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
        self.agents
            .composer
            .set_attachment_height(self.agents.files.height());
        let hide_composer = self.agents.history_open && !self.agents.chat.busy;
        let read_only = self
            .workspace
            .as_ref()
            .is_some_and(|w| w.plan.project_read_only);
        let isolated = self.workspace.as_ref().is_some_and(|w| w.plan.active);
        let layout = self
            .agents
            .composer
            .layout(ui, rect, unsaved > 0, hide_composer, !modal);
        let composer = layout.input;
        let mut content = layout.content;
        let queue_height = if hide_composer {
            0.
        } else {
            self.agents.queue.height().min(content.height() * 0.45)
        };
        let queue_rect = egui::Rect::from_min_max(
            egui::pos2(content.left(), content.bottom() - queue_height),
            content.max,
        );
        if queue_height > 0. {
            content.max.y = (queue_rect.top() - 6.).max(content.top());
        }
        let mut send = false;
        let mut attach = false;
        let mut stop = false;
        let mut new_chat = false;
        let mut running = false;
        let mut save = false;
        let mut answer = None;
        let mut history_action = history::Action::default();
        let remote = self.workspace.as_ref().and_then(|w| w.remote.clone());
        let agents = &mut self.agents;
        let installing = agents.runtime.installing();
        let install_enabled = !offline
            && !modal
            && !agents.chat.busy
            && matches!(agents.config.codex.trim(), "" | "auto" | "codex")
            && std::env::var_os("RODE_CODEX_BUNDLE").is_none();
        let automatic_codex = matches!(agents.config.codex.trim(), "" | "auto" | "codex");
        let codex_available = if agents.config.provider == Provider::Acp || !automatic_codex {
            Some(true)
        } else if offline {
            None
        } else {
            agents.runtime.check(ui, remote.as_ref())
        };
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
                                ui.add_enabled_ui(!agents.chat.busy && !agents.chat.queue_pending(),|ui|ui.selectable_value(&mut agents.config.provider,provider,provider.label()));
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
                    ui.add_enabled_ui(!agents.chat.busy && !agents.chat.queue_pending(),|ui| {
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
                                agents.queue.clear();
                                agents.chat.disconnect();
                                agents.chat.notice("Agent controls reset. The next message starts a fresh conversation.");
                            }
                        } else {
                            ui.label("Codex executable (auto: discover)");
                            ui.add(egui::TextEdit::singleline(&mut agents.config.codex).desired_width(f32::INFINITY).hint_text("auto, or /path/to/codex").char_limit(4096)).on_hover_text("Use Install Codex below, the optional rode-codex package or an installed executable. Sign in locally with rode --codex login. SSH projects use Codex and credentials on that host.");
                            if agents.config.provider == Provider::Local {
                                ui.label("Local model API base URL");
                                ui.add(egui::TextEdit::singleline(&mut agents.config.local_url).desired_width(f32::INFINITY).char_limit(2048)).on_hover_text("An Ollama /v1 or compatible Responses endpoint reachable from the project host. localhost means this host.");
                                ui.label("Installed model name");
                                ui.add(egui::TextEdit::singleline(&mut agents.config.local_model).desired_width(f32::INFINITY).hint_text("Model already available on the local server").char_limit(256));
                            } else {
                                ui.label("Model (blank: Codex default)");
                                ui.add(egui::TextEdit::singleline(&mut agents.config.model).desired_width(f32::INFINITY).char_limit(256));
                            }
                            agents.runtime.ui(ui, remote.as_ref(), install_enabled, codex_available);
                        }
                    });
                    ui.label(RichText::new("Settings are saved for this project. Model and Access controls are below. Executable/provider changes start a new conversation. Use the agent's login or environment for credentials.").small().color(palette.muted));
                });
            }
            ui.horizontal(|ui| { ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(!offline && !installing,egui::Button::new("History").small().selected(agents.history_open)).on_hover_text("Browse saved conversations on this host for this project").clicked() {
                    agents.history_open = !agents.history_open;
                    if agents.history_open {history_action.refresh=true;} else {agents.browser=None;}
                }
                if ui.add_enabled(!offline, egui::Button::new("Running").small()).on_hover_text("Reattach or stop background agent sessions on this host").clicked() { running = true; }
                ui.add(egui::Label::new(RichText::new(if agents.chat.status.is_empty() {"Ready to start"} else {&agents.chat.status}).size(12.).color(if offline {palette.error}else{palette.accent})).truncate());
            }); });
            if agents.history_open {
                let panel=history::ui(ui,agents.browser.as_ref(),&mut agents.history_filter,!agents.chat.busy && !modal,agents.chat.conversation_id(),&mut agents.history_delete);
                history_action.refresh |= panel.refresh;
                history_action.more=panel.more;
                history_action.resume=panel.resume;
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
                if agents.config.provider != Provider::Acp && !agents.settings
                    && (agents.chat.messages.is_empty() || agents.runtime.started()) {
                    agents.runtime.ui(ui, remote.as_ref(), install_enabled, codex_available);
                }
                if agents.chat.messages.is_empty() {
                    ui.add_space(8.);
                    ui.label(RichText::new("Work on this project with an agent").strong());
                    ui.add(egui::Label::new("Ask a question, request a change, or investigate an issue. The agent works in the folder above.").wrap());
                    if agents.config.provider == Provider::Local { ui.add(egui::Label::new("Start Ollama or a compatible local server and select an installed model in settings.").wrap()); }
                    else if agents.config.provider == Provider::Codex { ui.add(egui::Label::new(if codex_available == Some(false) { "Install Codex above or use an existing executable. Authenticate on the project host before sending a message." } else { "Authenticate Codex on the project host before sending a message." }).wrap()); }
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
                                let copy = ui.horizontal(|ui| {
                                    ui.label(RichText::new(&message.title).strong().size(11.).color(if message.role == Role::Notice {palette.muted}else{palette.accent}));
                                    message.role == Role::Assistant && icons::small(ui,Icon::Copy,"Copy response").clicked()
                                }).inner;
                                let output = render_message(ui,&message.text,palette);
                                if copy {ctx.copy_text(output.galley.text().to_owned());}
                            });
                        }
                    });
                    ui.add_space(5.);
                }
            });
        });
        if queue_height > 0. {
            ui.scope_builder(egui::UiBuilder::new().max_rect(queue_rect), |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(queue_rect));
                agents.queue.ui(
                    ui,
                    !offline && !modal && !agents.history_open && agents.chat.queue_capacity(),
                );
                agents.flush_queue();
            });
        }
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
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 26.), Layout::right_to_left(Align::Center), |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width().min(380.), 26.),
                        Layout::left_to_right(Align::Center),
                        |ui| controls::buttons(ui, &mut agents.config, &agents.chat, read_only, isolated),
                    );
                });
            });
            agents.files.ui(ui, !modal && !offline);
            let id = egui::Id::new("agent-composer");
            let shortcut = composer_submit(ui, id, modal, &mut agents.composing);
            agents.composer.recall(
                ui, &mut agents.draft, id,
                !modal && !agents.composing,
            );
            let response = composer::editor(ui, &mut agents.draft, id, layout.stretch)
                .inner
                .response;
            if agents.focus && !modal {
                if ui.memory(|m| m.allows_interaction(ui.layer_id())) {
                    response.request_focus();
                    agents.focus = false;
                } else {
                    ctx.request_repaint();
                }
            }
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    background::persistence(ui, &mut agents.chat, palette, !offline && !modal);
                    let remaining = ui.available_width().max(1.);
                    ui.allocate_ui_with_layout(
                        egui::vec2(remaining, ui.spacing().interact_size.y),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            let enabled = !offline
                                && !modal
                                && !installing
                                && !agents.chat.stopping()
                                && agents.queue.can_push()
                                && agents.chat.queue_capacity()
                                && !agents.history_open
                                && !agents.attaching
                                && !agents.files.pending
                                && (!agents.draft.trim().is_empty() || !agents.files.files.is_empty())
                                && agents.draft.len() <= agent::PROMPT_LIMIT
                                && (agents.config.provider != Provider::Acp
                                    || agents.argument_error.is_empty());
                            if ui
                                .add_enabled(enabled, egui::Button::new(if agents.chat.busy || agents.queue.waiting_for_reply() || !agents.queue.is_empty() { "Queue" } else { "Send" }))
                                .on_hover_text("Enter sends when idle or queues a follow-up while working.\nShift+Enter inserts a new line.")
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
                            if !agents.chat.busy
                                && agents.chat.connected()
                                && ui
                                    .add_enabled(!agents.chat.work_pending(), egui::Button::new(if agents.chat.persistent() {
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
                            if ui.add_enabled(!offline && !modal && !agents.history_open && !agents.files.pending && agents.files.files.len() < rode_agent_core::attachment::MAX_ATTACHMENTS,
                                egui::Button::new("Attach…")).on_hover_text("Attach images, PDF text, or UTF-8 documents. Files stay with this message, including when queued.").clicked() { attach = true; }
                            if ui.available_width() > 100. {
                                ui.add(
                                    egui::Label::new(palette.tag("Enter · Shift+Enter")).truncate(),
                                )
                                .on_hover_text("Enter to send; Shift+Enter for a new line. At the start of the message, Up/Down recalls sent messages; Down returns to your draft.");
                            }
                        },
                    );
                });
            });
        });
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
            if execution_before != agents.config
                && let Err(error) = agents.chat.configure_idle(&agents.config)
            {
                agents.queue.pause();
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
            agents.replace_chat();
            agents.completed = 0;
            agents.focus = true;
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
        if attach {
            self.set_dialog(Dialog::AgentAttachLocal);
        }
        if send {
            if self.agents.runtime.installing() {
                self.notify(
                    "Wait for Codex installation to finish or cancel it before sending.",
                    false,
                );
            } else {
                self.send_agent(ctx);
            }
        }
        if history_action.close {
            self.agents.browser = None;
            self.agents.history_open = false;
            self.agents.history_delete = None;
        } else if history_action.refresh {
            self.agents.history_delete = None;
            self.open_agent_history(ctx);
        } else if let Some(browser) = &mut self.agents.browser {
            let result = if let Some(selected) = history_action.delete {
                let protected = self
                    .agents
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
                self.agents.chat.notice(&error.to_string());
            }
        }
        if refresh {
            self.reload_clean_documents(ctx);
        }
        if running {
            self.open_background_agents(ctx);
        }
        self.agents.flush_queue();
    }

    fn send_agent(&mut self, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Agents) {
            return;
        }
        if self.agents.files.pending || self.agents.attaching {
            return;
        }
        let text = self.agents.draft.clone();
        let files = self.agents.files.files.clone();
        let result = if self.agents.chat.busy
            || self.agents.queue.waiting_for_reply()
            || !self.agents.queue.is_empty()
        {
            if !self.agents.chat.supports_queue() {
                Err(anyhow::anyhow!(
                    "This running agent predates persistent queues. Start a new chat to queue prompts without losing them on restart."
                ))
            } else if !self.agents.chat.queue_capacity() {
                Err(anyhow::anyhow!("Wait for queued changes to be saved."))
            } else {
                let result = self.agents.queue.push_with_attachments(text, files);
                self.agents.flush_queue();
                result
            }
        } else {
            self.submit_agent_input(text, files, ctx)
        };
        match result {
            Ok(()) => {
                self.agents.composer.reset_recall();
                self.agents.draft.clear();
                self.agents.files.clear();
                self.agents.focus = true;
            }
            Err(error) => {
                self.agents.chat.notice(&format!("{error:#}"));
                self.agents.settings = true;
            }
        }
    }

    fn submit_agent_input(
        &mut self,
        text: String,
        files: Vec<rode_agent_core::attachment::Attachment>,
        ctx: &egui::Context,
    ) -> anyhow::Result<()> {
        let launch = self.agent_launch();
        self.agents.chat.use_service();
        let context = ctx.clone();
        self.agents.chat.submit_with_attachments(
            launch,
            text.clone(),
            files,
            Arc::new(move || context.request_repaint()),
        )?;
        self.agents.composer.remember_prompt(text);
        self.agents.queue.set_hosted();
        // An older idle snapshot can arrive before the service acknowledges
        // Submit. Wait for a completed-turn counter change before sending again.
        self.agents.queue.submitted();
        Ok(())
    }

    fn dispatch_agent_queue(&mut self, ctx: &egui::Context) {
        if self.agents.chat.busy
            || self.agents.chat.queue_pending()
            || self.agents.queue.command().is_some()
            || self.agents.attaching
            || self.agents.history_open
            || self.agents.browser.is_some()
            || self.agents.runtime.installing()
            || self.quitting
            || self.dialog != Dialog::None
            || self.pending.is_some()
            || self.workbench_modal()
            || !self.feature(crate::extensions::Feature::Agents)
            || self
                .workspace
                .as_ref()
                .is_some_and(|w| !w.ready || !w.connected && w.remote.is_some())
        {
            return;
        }
        let Some(text) = self.agents.queue.next().map(str::to_owned) else {
            return;
        };
        let result = if self.agents.queue.hosted() {
            let id = self.agents.queue.head_id().unwrap();
            self.agents
                .chat
                .dispatch_queued(id, self.agents.config.clone())
                .map(|()| {
                    self.agents.composer.remember_prompt(text);
                    self.agents.queue.submitted();
                })
        } else {
            let files = self.agents.queue.next_attachments().to_vec();
            self.submit_agent_input(text, files, ctx)
        };
        match result {
            Ok(()) => self.agents.queue.sent(),
            Err(error) => {
                self.agents.queue.pause();
                self.agents
                    .chat
                    .notice(&format!("Queued prompt was not sent: {error:#}"));
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
        match agent::history::Browser::open_service(
            launch,
            Arc::new(move || context.request_repaint()),
        ) {
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

/// Use one immutable text buffer so selection can span prose and fenced code.
fn render_message(
    ui: &mut egui::Ui,
    text: &str,
    palette: theme::Palette,
) -> egui::text_edit::TextEditOutput {
    let job = message::layout(ui, text, palette);
    // A parent can request more width than the visible transcript, especially
    // during resizing. Wrap to the visible edge rather than that larger layout.
    let visible_width = ui
        .available_width()
        .min(ui.clip_rect().right() - ui.next_widget_position().x)
        .max(1.);
    let mut display = job.text.as_str();
    let mut layouter = |ui: &egui::Ui, _: &dyn egui::TextBuffer, width: f32| {
        let mut job = job.clone();
        job.wrap.max_width = width.min(visible_width).max(1.);
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    let id = ui.make_persistent_id("message-text");
    let previous_selection =
        if ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary)) {
            egui::TextEdit::load_state(ui.ctx(), id)
                .and_then(|state| state.cursor.char_range())
                .filter(|range| !range.is_empty())
        } else {
            None
        };
    let mut output = egui::TextEdit::multiline(&mut display)
        .id(id)
        .frame(false)
        .margin(egui::Vec2::ZERO)
        .desired_width(visible_width)
        .desired_rows(1)
        .layouter(&mut layouter)
        .show(ui);
    // TextEdit moves the caret on any pointer press, including a context-menu click.
    if output.response.contains_pointer()
        && let Some(selection) = previous_selection
    {
        output.state.cursor.set_char_range(Some(selection));
        output.cursor_range = output.state.cursor.range(&output.galley);
        output.state.clone().store(ui.ctx(), id);
    }
    output.response.context_menu(|ui| {
        // The popup can take keyboard focus; use the stored, clamped selection.
        let selection = output
            .state
            .cursor
            .range(&output.galley)
            .filter(|range| !range.is_empty());
        if ui
            .add_enabled(selection.is_some(), egui::Button::new("Copy selection"))
            .clicked()
        {
            if let Some(selection) = selection {
                ui.ctx().copy_text(selection.slice_str(display).to_owned());
            }
            ui.close();
        }
    });
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn long_responses_wrap_to_the_visible_chat_width_after_resizing() {
        for (body, code) in [
            (
                "A long response should remain readable in the chat panel. ".repeat(80),
                false,
            ),
            (
                format!("https://example.invalid/{}", "longpath".repeat(200)),
                false,
            ),
            (format!("    result = '{}'", "value".repeat(200)), true),
        ] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.agents.config.provider = Provider::Acp;
            app.agents
                .chat
                .messages
                .push_back(agent::protocol::Message {
                    id: "wrapped-response".into(),
                    role: Role::Assistant,
                    title: "Agent".into(),
                    text: if code {
                        format!("```text\n{body}\n```")
                    } else {
                        body.clone()
                    },
                });
            for (width, oversized) in [
                (1100., false),
                (500., false),
                (320., false),
                (1100., true),
                (500., true),
            ] {
                let mut output = None;
                for _ in 0..2 {
                    output = Some(ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 800.),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                if oversized {
                                    ui.set_min_width(width * 2.);
                                }
                                app.agent_ui(ui, ctx);
                            });
                        },
                    ));
                }
                let output = output.unwrap();
                let (shape, text) = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == body.trim_end() => {
                            Some((shape, text))
                        }
                        _ => None,
                    })
                    .expect("rendered response text");
                assert!(text.galley.rows.len() > 1);
                assert!(
                    text.pos.x + text.galley.size().x <= shape.clip_rect.right() + 1.,
                    "response overruns visible chat: width={width}, right={}, clip={}",
                    text.pos.x + text.galley.size().x,
                    shape.clip_rect.right()
                );
                assert!(!text.galley.elided);
            }
        }
    }

    #[test]
    fn copy_response_copies_displayed_content_without_code_fences() {
        for (source, expected) in [
            (
                "```text\nCommit title\n\n- change\n```",
                "Commit title\n\n- change",
            ),
            (
                "Intro\n\n```rust\n  let value = `literal`;\n```\n\nOutro",
                "Intro\n\n  let value = `literal`;\n\nOutro",
            ),
        ] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.agents.config.provider = Provider::Acp;
            app.agents
                .chat
                .messages
                .push_back(agent::protocol::Message {
                    id: "copy-response-test".into(),
                    role: Role::Assistant,
                    title: "Agent".into(),
                    text: source.into(),
                });
            let mut frame = |events| {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(900., 760.),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| app.agent_ui(ui, ctx));
                    },
                )
            };
            let output = frame(vec![]);
            let point = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == "Agent" => Some(
                        text.pos
                            + egui::vec2(
                                text.galley.size().x + 8. + 13.,
                                text.galley.size().y / 2.,
                            ),
                    ),
                    _ => None,
                })
                .expect("assistant message header");
            let pointer = |pressed| egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(vec![egui::Event::PointerMoved(point), pointer(true)]);
            let output = frame(vec![pointer(false)]);
            assert!(
                output.platform_output.commands.iter().any(|command| {
                    matches!(command, egui::OutputCommand::CopyText(text) if text == expected)
                }),
                "Copy response did not copy the displayed text"
            );
        }
    }

    #[test]
    fn busy_submission_queues_without_starting_another_turn_and_replacement_clears_it() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.agents.chat.busy = true;
        app.agents.draft = "Follow up after this task".into();
        app.send_agent(&ctx);
        assert_eq!(app.agents.queue.next(), Some("Follow up after this task"));
        assert!(app.agents.draft.is_empty());
        assert!(!app.agents.chat.connected());
        assert!(app.agents.chat.messages.is_empty());
        app.agents.chat.busy = false;
        app.dialog = Dialog::Shortcuts;
        app.dispatch_agent_queue(&ctx);
        assert_eq!(app.agents.queue.next(), Some("Follow up after this task"));
        app.agents.replace_chat();
        assert!(app.agents.queue.is_empty());
    }

    #[test]
    fn failed_queue_submission_keeps_the_prompt_and_draft_and_pauses_dispatch() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.agents.config.codex = "invalid\0executable".into();
        app.agents.queue.push("retained follow-up".into()).unwrap();
        app.agents.draft = "unfinished draft".into();
        app.dispatch_agent_queue(&ctx);
        assert!(!app.agents.queue.is_empty());
        assert!(app.agents.queue.next().is_none());
        assert_eq!(app.agents.draft, "unfinished draft");
        assert!(!app.agents.chat.busy);
        assert!(
            app.agents
                .chat
                .messages
                .iter()
                .any(|message| message.text.contains("Queued prompt was not sent"))
        );
    }

    #[test]
    fn response_selection_copies_across_prose_and_code_without_editing() {
        let ctx = egui::Context::default();
        let source = "**Alpha** *βeta*\n\n```rust\nlet value = 1;\n```\n\nOmega end";
        let frame = |events| {
            let mut output = None;
            let full = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        output = Some(render_message(ui, source, theme::Palette::default()));
                    });
                },
            );
            (full, output.unwrap())
        };
        let (_, output) = frame(vec![]);
        let display = output.galley.text().to_owned();
        assert_eq!(display, "Alpha βeta\n\nlet value = 1;\n\nOmega end");
        let start = 6;
        let end = display.chars().count() - 4;
        let position = |index| {
            output.galley_pos
                + output
                    .galley
                    .pos_from_cursor(egui::text::CCursor::new(index))
                    .center()
                    .to_vec2()
        };
        let from = position(start);
        let to = position(end);
        let pointer = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(from), pointer(from, true)]);
        frame(vec![egui::Event::PointerMoved(to)]);
        let (_, output) = frame(vec![pointer(to, false)]);
        assert!(output.response.has_focus());
        assert_eq!(
            output.cursor_range.unwrap().as_sorted_char_range(),
            start..end
        );
        let (full, _) = frame(vec![egui::Event::Copy]);
        let expected: String = display.chars().skip(start).take(end - start).collect();
        assert!(full.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &expected)
        ));
        let secondary = |pressed| egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![secondary(true)]);
        frame(vec![secondary(false)]);
        let (full, output) = frame(vec![]);
        assert_eq!(
            output
                .state
                .cursor
                .range(&output.galley)
                .unwrap()
                .as_sorted_char_range(),
            start..end
        );
        let copy_position = full
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Copy selection" => {
                    Some(text.pos + text.galley.rect.center().to_vec2())
                }
                _ => None,
            })
            .expect("copy selection menu item");
        frame(vec![
            egui::Event::PointerMoved(copy_position),
            pointer(copy_position, true),
        ]);
        let (full, _) = frame(vec![pointer(copy_position, false)]);
        assert!(full.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &expected)
        ));
        output.response.request_focus();
        let (_, output) = frame(vec![
            egui::Event::Text("replacement".into()),
            egui::Event::Paste("injected".into()),
            egui::Event::Cut,
        ]);
        assert!(output.response.has_focus());
        assert_eq!(output.galley.text(), display);
    }

    #[test]
    fn response_selection_survives_streaming_updates() {
        let ctx = egui::Context::default();
        let frame = |text: &str, events| {
            let mut output = None;
            let full = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        output = Some(render_message(ui, text, theme::Palette::default()));
                    });
                },
            );
            (full, output.unwrap())
        };
        let (_, mut output) = frame("Alpha **βeta**", vec![]);
        output
            .state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(6),
                egui::text::CCursor::new(10),
            )));
        output.state.store(&ctx, output.response.id);
        output.response.request_focus();
        let (full, output) = frame(
            "Alpha **βeta**\n\n```rust\nlet value = 1;\n```",
            vec![egui::Event::Copy],
        );
        assert_eq!(output.cursor_range.unwrap().as_sorted_char_range(), 6..10);
        assert!(full.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "βeta")
        ));
    }

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

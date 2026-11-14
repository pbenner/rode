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
#[cfg(test)]
use serde_json::json;
pub(super) mod attachments;
mod background;
mod composer;
mod controls;
mod header;
mod history;
mod message;
mod prompt_recall;
mod questions;
mod queue;
mod recovery;
mod runtime;

/// Window-owned chat presentation, host-keyed configuration and disposable backend views.
/// Only bounded settings enter preferences; live transcripts and queued attachments
/// belong to the controller, and retired views retain unfinished lifetime acknowledgements.
pub(super) struct Agents {
    pub chat: Chat,
    catalog: Option<agent::controls::Catalog>,
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
    history_scope: agent::history::Scope,
    history_delete: Option<history::Delete>,
    runtime: runtime::Runtime,
    running: background::Running,
    retired: Vec<Chat>,
    attaching: bool,
    prompt_reader: Option<agent::prompt_history::Reader>,
    prompt_reader_failed: bool,
}
impl Agents {
    /// Restore a bounded configuration map and initialize inert chat/history/setup views.
    pub(super) fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let preferences = storage
            .and_then(|s| s.get_string("agent_preferences"))
            .filter(|s| s.len() <= 256 * 1024)
            .and_then(|s| serde_json::from_str::<Preferences>(&s).ok())
            .filter(|p| p.len() <= 128)
            .unwrap_or_default();
        Self {
            chat: Chat::default(),
            catalog: None,
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
            history_scope: agent::history::Scope::Project,
            history_delete: None,
            runtime: runtime::Runtime::default(),
            running: Default::default(),
            retired: vec![],
            attaching: false,
            prompt_reader: None,
            prompt_reader_failed: false,
            local_root: std::env::current_dir()
                .unwrap_or_else(|_| files::expand_home("~"))
                .to_string_lossy()
                .into(),
        }
    }
    /// Write only agent configuration preferences, excluding transcripts, credentials and live jobs.
    pub(super) fn store(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(data) = serde_json::to_string(&self.preferences) {
            storage.set_string("agent_preferences", data);
        }
    }
    /// Record the current host/project configuration while keeping the saved map within
    /// its entry and serialized-byte budgets; mark the preference write pending.
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
    /// Transfer pending queue edits to a hosted chat while transport capacity permits.
    /// Remove a command only after it is accepted; failures remain visible and retryable.
    fn flush_queue(&mut self) {
        if !self.queue.hosted() {
            return;
        }
        while self.chat.queue_capacity() {
            let Some(operation) = self.queue.command().cloned() else {
                break;
            };
            if agent::response_style::requires_confirmation(&operation)
                && self
                    .chat
                    .response_style_status(self.config.personality)
                    .is_some()
            {
                break;
            }
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
    /// New chat must not replace a working controller or an unfinished Stop.
    fn can_start_new_chat(&self) -> bool {
        !self.chat.busy && !self.chat.stopping()
    }

    /// Reset an idle conversation without starting a backend. Recheck here so
    /// a stale UI action cannot bypass the disabled header control.
    fn start_new_chat(&mut self) -> bool {
        if !self.can_start_new_chat() {
            return false;
        }
        self.browser = None;
        self.history_open = false;
        self.replace_chat();
        self.completed = 0;
        self.focus = true;
        true
    }

    /// Release the old conversation view and invalidate pending composer attachment reads.
    fn replace_chat(&mut self) {
        self.release_chat();
        self.files.clear();
    }
    /// Flush queued edits before dropping presentation state; retain a retiring chat
    /// while startup or lifetime work still requires acknowledgement.
    fn release_chat(&mut self) {
        self.flush_queue();
        self.composer.clear_history();
        self.prompt_reader = None;
        self.prompt_reader_failed = false;
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
    /// Derive the execution root, display host and configuration key from the selected
    /// workspace; SSH configuration participates in the key to separate transport contexts.
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

    /// Drain setup, discovery, history and chat updates, rejecting changed host/interpreter
    /// contexts and retaining old lifetime work until it is safe to release.
    pub(super) fn poll_agents(&mut self, ctx: &egui::Context) {
        self.agents.runtime.poll();
        let (_, _, key) = self.agent_location();
        if self.agents.key != key {
            self.agents.runtime.reset();
            self.agents.browser = None;
            self.agents.history_open = false;
            self.agents.history_filter.clear();
            self.agents.history_scope = agent::history::Scope::Project;
            self.agents.history_delete = None;
            self.agents.save_config();
            self.agents.replace_chat();
            self.agents.running.reset_context();
            self.agents.completed = 0;
            self.agents.draft.clear();
            self.agents.composing = false;
            self.agents.config = self
                .agents
                .preferences
                .get(&key)
                .cloned()
                .unwrap_or_default();
            self.agents.arguments = serde_json::to_string(self.agents.config.acp_arguments())
                .unwrap_or_else(|_| "[]".into());
            self.agents.argument_error.clear();
            self.agents.key = key;
            self.agents.catalog = None;
        }
        if !self.panel_visible(Panel::Agent)
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
            self.abort_lifecycle(error, ctx);
        }
        let was_busy = self.agents.chat.busy;
        self.agents.chat.poll();
        if self
            .agents
            .catalog
            .as_ref()
            .is_some_and(|catalog| !catalog.matches(&self.agents.config))
        {
            self.agents.catalog = None;
        }
        if let Some(catalog) = &mut self.agents.catalog {
            catalog.poll();
            if catalog.busy() {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
        if let Some(error) = self.agents.chat.take_persistence_error() {
            self.abort_lifecycle(error, ctx);
        }
        if self.agents.attaching
            && let Some(config) = self.agents.chat.attached_config().cloned()
        {
            self.agents.config = config;
            self.agents
                .composer
                .restore_history(self.agents.chat.input_history().map(str::to_owned));
            self.agents.arguments = serde_json::to_string(self.agents.config.acp_arguments())
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
                self.agents
                    .composer
                    .restore_history(self.agents.chat.input_history().map(str::to_owned));
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
        while let Some(prompt) = self.agents.queue.take_dispatched() {
            self.agents.composer.remember_prompt(prompt);
        }
        self.poll_prompt_recall(ctx);
        if self.agents.completed != self.agents.chat.completed {
            self.agents.completed = self.agents.chat.completed;
            self.agents.queue.finished();
            if !self.agents.chat.automatic_queue() && !self.agents.chat.last_turn_succeeded {
                self.agents.queue.pause();
            }
            self.reload_clean_documents(ctx);
        }
        if (was_busy || self.agents.queue.waiting_for_reply())
            && !self.agents.chat.busy
            && !self.agents.chat.connected()
        {
            self.agents.queue.finished();
            if !self.agents.chat.automatic_queue() {
                self.agents.queue.pause();
            }
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

    /// Invalidate discovery/queue state and disconnect the current controller after project settings change.
    pub(super) fn stop_project_agent(&mut self) {
        self.agents.catalog = None;
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

    /// Release the current conversation on a context change while retaining pending cleanup work.
    pub(super) fn leave_project_agent(&mut self) {
        self.agents.catalog = None;
        self.agents.runtime.reset();
        self.agents.browser = None;
        self.agents.history_open = false;
        self.agents.replace_chat();
    }

    /// Clean exit lets owned setup workers release temporary files and locks.
    pub(super) fn cancel_agent_installation(&mut self) {
        self.agents.catalog = None;
        self.agents.runtime.cancel_all();
        self.agents.chat.cancel_startup_for_exit();
        for chat in &self.agents.retired {
            chat.cancel_startup_for_exit();
        }
    }

    /// Include setup, confirmed background mutations and current/retired chat lifetime
    /// work when deciding whether normal application exit must wait.
    pub(super) fn agent_installation_pending(&self) -> bool {
        self.agents.runtime.pending()
            || self.agents.running.mutating
            || self.agents.chat.work_pending()
            || self.agents.retired.iter().any(Chat::work_pending)
    }

    /// Queue changed configuration preferences and a storage flush, reporting unavailable storage.
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

    /// Render cached conversation, approvals, settings and composer state; dispatch
    /// explicit actions only after capturing the current host, policy and source context.
    pub(super) fn agent_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let (root, host, _) = self.agent_location();
        let mode = self
            .workspace
            .as_ref()
            .map(|w| w.plan.label())
            .unwrap_or("NORMAL MODE");
        // The chat can be detached from the main project banner. Keep the
        // applied grants and fallback reason reachable in that window too.
        let mode_detail = self.workspace.as_ref().map_or_else(
            || "No outer project sandbox is active. Agent access controls still apply.".to_owned(),
            |workspace| {
                let plan = &workspace.plan;
                if plan.active {
                    format!(
                        "{} is active. Project tools have {} network access.\n{}{}",
                        plan.backend.label(),
                        if plan.prefix.iter().any(|arg| arg == "--share-net") { "enabled" } else { "blocked" },
                        if plan.project_read_only { "The project is read only.\n" } else { "" },
                        plan.reason,
                    )
                } else if plan.requested {
                    format!("The requested {} sandbox is unavailable; the project is in normal mode. Agent access controls still apply.\n{}", plan.backend.label(), plan.reason)
                } else {
                    "No outer project sandbox is active. Agent access controls still apply.".to_owned()
                }
            },
        );
        let offline = self
            .workspace
            .as_ref()
            .is_some_and(|w| !w.ready || (w.remote.is_some() && !w.connected));
        let unsaved = self.documents.iter().filter(|d| d.dirty()).count();
        let modal = self.dialog != Dialog::None || self.pending.modal() || self.workbench_modal();
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        // A parent may retain a larger minimum width after a resize. Size all
        // chat controls against the visible panel rather than that stale width.
        let rect = rect.intersect(ui.clip_rect());
        if !rect.is_positive() {
            return;
        }
        // Hover/focus frames extend beyond widget bounds. Keep that paint space
        // inside the panel instead of clipping it at the header or composer.
        let paint_margin = ui.visuals().clip_rect_margin;
        // Give history the full compact panel; retain Stop while a turn is running.
        self.agents
            .composer
            .set_attachment_height(self.agents.files.height());
        let answering = self
            .agents
            .chat
            .prompts
            .front()
            .is_some_and(|p| !p.questions.is_empty());
        if answering {
            // Questions take precedence over a history browser opened while
            // the turn was running. Never hide an outstanding input request.
            self.agents.history_open = false;
            self.agents.browser = None;
        }
        let hide_composer = answering || self.agents.history_open && !self.agents.chat.busy;
        let read_only = self
            .workspace
            .as_ref()
            .is_some_and(|w| w.plan.project_read_only);
        let isolated = self.workspace.as_ref().is_some_and(|w| w.plan.active);
        let layout = self.agents.composer.layout(
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
        let remote = self.workspace.as_ref().and_then(|w| w.remote.clone());
        let agents = &mut self.agents;
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
                            let executable = ui.add(egui::TextEdit::singleline(program).desired_width(f32::INFINITY).hint_text(if claude { rode_agent_core::CLAUDE_ACP } else { "agent executable or absolute path" }).char_limit(4096));
                            ui_probe::button(ui, if claude { "agent-claude-executable" } else { "agent-acp-executable" }, &executable);
                            ui.label("Arguments (JSON array)");
                            if ui.add(egui::TextEdit::singleline(&mut agents.arguments).desired_width(f32::INFINITY).hint_text("[\"acp\"]").char_limit(16384)).changed() {
                                match serde_json::from_str::<Vec<String>>(&agents.arguments) {
                                    Ok(args) => { *agents.config.acp_arguments_mut()=args; agents.argument_error.clear(); }
                                    Err(_) => agents.argument_error="Use a JSON array of argument strings.".into(),
                                }
                            }
                            if !agents.argument_error.is_empty() { ui.colored_label(palette.error,&agents.argument_error); }
                            ui.label(palette.tag("Agent must provide its own file and command tools."));
                            if claude {
                                ui.add(egui::Label::new("Install claude-agent-acp and authenticate Claude on this project host. The adapter uses your Claude login or ANTHROPIC_API_KEY from the host environment.").wrap());
                                ui.hyperlink_to("Claude adapter setup", "https://github.com/agentclientprotocol/claude-agent-acp");
                            }
                            if ui.small_button("Reset saved agent controls").clicked() {
                                agents.config.acp_options_mut().clear();
                                agents.queue.clear();
                                agents.chat.disconnect();
                                agents.chat.notice("Agent controls reset. The next message starts a fresh conversation.");
                            }
                        } else {
                            ui.label("Codex executable (auto: discover)");
                            let executable = ui.add(egui::TextEdit::singleline(&mut agents.config.codex).desired_width(f32::INFINITY).hint_text("auto, or /path/to/codex").char_limit(4096)).on_hover_text("Use Install Codex below, the optional rode-codex package or an installed executable. Sign in locally with rode --codex login. SSH projects use Codex and credentials on that host.");
                            ui_probe::button(ui, "agent-codex-executable", &executable);
                            if agents.config.provider == Provider::Local {
                                ui.label("Local model API base URL");
                                ui.add(egui::TextEdit::singleline(&mut agents.config.local_url).desired_width(f32::INFINITY).char_limit(2048)).on_hover_text("An Ollama /v1 or compatible Responses endpoint reachable from the project host. localhost means this host.");
                                ui.label("Installed model name");
                                let model = ui.add(egui::TextEdit::singleline(&mut agents.config.local_model).desired_width(f32::INFINITY).hint_text("Model already available on the local server").char_limit(256));
                                ui_probe::button(ui, "agent-local-model", &model);
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
                let history = ui.add_enabled(!offline && !installing && !answering,egui::Button::new("History").selected(agents.history_open)).on_hover_text("Browse saved conversations for this project or other projects on this host");
                ui_probe::button(ui, "agent-history", &history);
                if history.clicked() {
                    agents.history_open = !agents.history_open;
                    if agents.history_open {history_action.refresh=true;} else {agents.browser=None;}
                }
                if ui.add_enabled(!offline, egui::Button::new("Running")).on_hover_text("Reattach or stop background agent sessions on this host").clicked() { running = true; }
                ui.add(egui::Label::new(RichText::new(if agents.chat.status.is_empty() {"Ready to start"} else {&agents.chat.status}).size(12.).color(if offline {palette.error}else{palette.accent})).truncate());
            }); });
            if let Some(status) = agents.chat.response_style_status(agents.config.personality) {
                let mismatch = matches!(status, agent::response_style::Status::Mismatch { .. });
                let response = ui.add(egui::Label::new(RichText::new(status.message())
                    .small().color(if mismatch { palette.error } else { palette.muted })).wrap().selectable(true));
                ui_probe::button(ui, if mismatch { "agent-tone-mismatch" } else { "agent-tone-pending" }, &response);
                if let agent::response_style::Status::Mismatch { applied, .. } = status {
                    let accept = ui.add_enabled(!agents.chat.busy && !offline && !modal,
                        egui::Button::new(format!("Use {} tone", applied.label())));
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
                            ui.collapsing("Details",|ui| { ui.add(egui::Label::new(RichText::new(&prompt.detail).monospace().size(11.)).wrap().selectable(true)); });
                        }
                        ui.horizontal_wrapped(|ui| {
                            for (index, choice) in prompt.choices.iter().enumerate() {
                                let response = ui.button(&choice.label);
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
                            ui.collapsing(RichText::new(&message.title).monospace().size(12.).color(palette.muted),|ui| {
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
                    if ui.small_button("Save all").clicked() {
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
                                .add_enabled(enabled, egui::Button::new(if agents.chat.busy || agents.queue.waiting_for_reply() || !agents.queue.is_empty() { "Queue" } else { "Send" }))
                                .on_hover_text("Enter sends when idle or queues a follow-up while working.\nShift+Enter inserts a new line.");
                            ui_probe::button(ui, "agent-send", &send_button);
                            if send_button.clicked()
                                || (shortcut && enabled)
                            {
                                send = true;
                            }
                            if agents.chat.busy {
                                let button = ui.button(if agents.chat.stopping() {
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
                            let attach_button = ui.add_enabled(!offline && !modal && !agents.history_open && !agents.files.pending && agents.files.files.len() < rode_agent_core::attachment::MAX_ATTACHMENTS,
                                egui::Button::new("Attach…")).on_hover_text("Attach images, PDF text, or UTF-8 documents. Files stay with this message, including when queued.");
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
        if save {
            self.save_all(ctx);
        }
        if attach {
            self.set_dialog(Dialog::AgentAttachLocal);
        }
        if discover_models {
            let context = ctx.clone();
            self.agents.catalog = Some(agent::controls::discover(
                self.agent_launch(),
                Arc::new(move || context.request_repaint()),
            ));
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
            let result = if let Some(scope) = history_action.scope {
                self.agents.history_delete = None;
                self.agents.history_scope = scope;
                browser.set_scope(scope)
            } else if let Some(selected) = history_action.delete {
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
        if running || history_action.running {
            self.open_background_agents(ctx);
        }
        if let Some(fresh) = recover {
            let launch = self.agent_launch();
            let context = ctx.clone();
            if let Err(error) =
                self.agents
                    .chat
                    .recover(launch, fresh, Arc::new(move || context.request_repaint()))
            {
                self.agents
                    .chat
                    .notice(&format!("Agent recovery failed: {error:#}"));
            }
        }
        self.agents.flush_queue();
    }

    fn send_agent(&mut self, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Agents) {
            return;
        }
        if let Some(status) = self
            .agents
            .chat
            .response_style_status(self.agents.config.personality)
        {
            self.agents.chat.notice(&status.message());
            return;
        }
        if self.agents.files.pending
            || self.agents.attaching
            || self
                .agents
                .prompt_reader
                .as_ref()
                .is_some_and(|reader| reader.anchoring())
        {
            return;
        }
        let text = self.agents.draft.clone();
        let files = self.agents.files.files.clone();
        let result = if self.agents.chat.busy
            || self.agents.queue.waiting_for_reply()
            || !self.agents.queue.is_empty()
        {
            if !self.agents.chat.queue_capacity() {
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
        anyhow::ensure!(
            !self.agents.config.files_only() || self.workspace.is_some(),
            "Open a project folder before using Edit project files only."
        );
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
        if self.agents.chat.automatic_queue() {
            return;
        }
        if self.agents.chat.busy
            || self
                .agents
                .chat
                .response_style_status(self.agents.config.personality)
                .is_some()
            || self.agents.chat.queue_pending()
            || self.agents.queue.command().is_some()
            || self.agents.attaching
            || self.agents.history_open
            || self.agents.browser.is_some()
            || self.agents.runtime.installing()
            || self.exit.closing()
            || self.dialog != Dialog::None
            || self.pending.modal()
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
        match agent::history::Browser::open_service_scoped(
            launch,
            self.agents.history_scope,
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

/// Shared by the composer and question fields: consume only a fresh, unmodified Enter. egui's
/// consume_key ignores extra Shift/Alt, which would also consume newlines.
/// IME confirmation belongs to the text editor, including a commit+Enter frame.
fn text_submit(ui: &mut egui::Ui, id: egui::Id, modal: bool, composing: &mut bool) -> bool {
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
    copy_response: bool,
) -> egui::text_edit::TextEditOutput {
    let layout = message::layout(ui, text, palette);
    // A parent can request more width than the visible transcript, especially
    // during resizing. Wrap to the visible edge rather than that larger layout.
    let visible_width = ui
        .available_width()
        .min(ui.clip_rect().right() - ui.next_widget_position().x)
        .max(1.);
    let id = ui.make_persistent_id("message-text");
    let viewport =
        egui::Rect::from_min_size(ui.next_widget_position(), egui::vec2(visible_width, 0.));
    let render = |ui: &mut egui::Ui| {
        render_message_layout(ui, &layout, palette, copy_response, viewport, id)
    };
    if layout.has_tables() {
        egui::ScrollArea::horizontal()
            .id_salt(id.with("table-scroll"))
            .max_width(visible_width)
            .auto_shrink([false, true])
            .show(ui, render)
            .inner
    } else {
        render(ui)
    }
}

/// Keep one selection and clipboard path for wrapped prose and aligned table rows.
fn render_message_layout(
    ui: &mut egui::Ui,
    layout: &message::MessageLayout,
    palette: theme::Palette,
    copy_response: bool,
    viewport: egui::Rect,
    id: egui::Id,
) -> egui::text_edit::TextEditOutput {
    let job = &layout.job;
    let visible_width = viewport.width();
    let mut display = job.text.as_str();
    let mut layouter = |ui: &egui::Ui, _: &dyn egui::TextBuffer, width: f32| {
        let prose_offset = (viewport.left() - ui.next_widget_position().x).max(0.);
        layout.galley(ui, width.min(visible_width).max(1.), prose_offset)
    };
    let previous_selection =
        if ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary)) {
            egui::TextEdit::load_state(ui.ctx(), id)
                .and_then(|state| state.cursor.char_range())
                .filter(|range| !range.is_empty())
        } else {
            None
        };
    let commands_before = ui.ctx().output(|output| output.commands.len());
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
    layout.paint(ui, &output, palette);
    // TextEdit owns keyboard selection/copy. Replace only commands it just emitted,
    // expanding the single-character formula slots into portable TeX source.
    if let Some(range) = output
        .state
        .cursor
        .char_range()
        .filter(|range| !range.is_empty())
    {
        ui.ctx().output_mut(|output| {
            for command in output.commands.iter_mut().skip(commands_before) {
                if let egui::OutputCommand::CopyText(text) = command {
                    *text = layout.copy(range.as_sorted_char_range());
                }
            }
        });
    }
    if copy_response {
        ui.ctx().copy_text(layout.copy(0..job.text.chars().count()));
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
                ui.ctx()
                    .copy_text(layout.copy(selection.as_sorted_char_range()));
            }
            ui.close();
        }
    });
    output
}

#[cfg(test)]
mod tests;

//! Agent lifecycle and prompt admission own no application coordinator reference.
//! The UI supplies captured execution facts and modal admission, then applies returned
//! document-refresh/lifecycle effects. Transcripts, queue order and retired workers stay here.
use super::{Agents, Chat, Launch, agent};
use crate::{sandbox, ssh::Remote};
use eframe::egui;
use std::{sync::Arc, time::Duration};

/// Captured project authority and interpreter selection for one controller operation.
/// Configuration remains with the agent owner and is read when constructing a launch.
pub(super) struct Environment {
    pub key: String,
    pub plan: sandbox::Plan,
    pub remote: Option<Remote>,
    pub python: String,
    pub has_project: bool,
}

/// Cross-domain consequences applied by the composition root after polling the agent owner.
#[derive(Default)]
pub(super) struct Effects {
    pub failures: Vec<String>,
    pub refresh_documents: bool,
}

impl Agents {
    /// Build a launch from captured host authority and the currently selected agent configuration.
    pub(super) fn launch(&self, environment: &Environment) -> Launch {
        Launch {
            config: self.config.clone(),
            plan: environment.plan.clone(),
            remote: environment.remote.clone(),
            python: environment.python.clone(),
        }
    }

    /// Wake only for pending acknowledgements and explicit backend deadlines.
    pub(super) fn schedule_poll(&self, ctx: &egui::Context) {
        if self.chat.work_pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.chat.busy && self.chat.has_deadline() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Invalidate discovery/queue state and disconnect the current controller after project settings change.
    pub(super) fn stop_project(&mut self) {
        self.catalog = None;
        self.queue.clear();
        self.runtime.reset();
        self.browser = None;
        self.history_open = false;
        if self.chat.connected() || self.chat.busy {
            self.chat.disconnect();
            self.chat.notice(
                "Project session changed. The next message starts a new agent conversation.",
            );
        }
    }

    /// Release the current conversation on a context change while retaining pending cleanup work.
    pub(super) fn leave_project(&mut self) {
        self.catalog = None;
        self.runtime.reset();
        self.browser = None;
        self.history_open = false;
        self.replace_chat();
    }

    /// Clean exit lets owned setup workers release temporary files and locks.
    pub(super) fn cancel_installation(&mut self) {
        self.catalog = None;
        self.runtime.cancel_all();
        self.chat.cancel_startup_for_exit();
        for chat in &self.retired {
            chat.cancel_startup_for_exit();
        }
    }

    /// Include setup, confirmed background mutations and current/retired chat lifetime
    /// work when deciding whether normal application exit must wait.
    pub(super) fn installation_pending(&self) -> bool {
        self.runtime.pending()
            || self.running.mutating
            || self.chat.work_pending()
            || self.retired.iter().any(Chat::work_pending)
    }

    fn submit_input(
        &mut self,
        environment: &Environment,
        text: String,
        files: Vec<rode_agent_core::attachment::Attachment>,
        ctx: &egui::Context,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.config.files_only() || environment.has_project,
            "Open a project folder before using Edit project files only."
        );
        let launch = self.launch(environment);
        self.chat.use_service();
        let context = ctx.clone();
        self.chat.submit_with_attachments(
            launch,
            text.clone(),
            files,
            Arc::new(move || context.request_repaint()),
        )?;
        self.composer.remember_prompt(text);
        self.queue.set_hosted();
        // An older idle snapshot can arrive before the service acknowledges
        // Submit. Wait for a completed-turn counter change before sending again.
        self.queue.submitted();
        Ok(())
    }

    /// Submit or queue the current draft; failures preserve the draft and open its settings.
    pub(super) fn send(&mut self, environment: &Environment, ctx: &egui::Context) {
        if let Some(status) = self.chat.response_style_status(self.config.personality) {
            self.chat.notice(&status.message());
            return;
        }
        if self.files.pending
            || self.attaching
            || self
                .prompt_reader
                .as_ref()
                .is_some_and(|reader| reader.anchoring())
        {
            return;
        }
        let text = self.draft.clone();
        let files = self.files.files.clone();
        let result = if self.chat.busy || self.queue.waiting_for_reply() || !self.queue.is_empty() {
            if !self.chat.queue_capacity() {
                Err(anyhow::anyhow!("Wait for queued changes to be saved."))
            } else {
                let result = self.queue.push_with_attachments(text, files);
                self.flush_queue();
                result
            }
        } else {
            self.submit_input(environment, text, files, ctx)
        };
        match result {
            Ok(()) => {
                self.composer.reset_recall();
                self.draft.clear();
                self.files.clear();
                self.focus = true;
            }
            Err(error) => {
                self.chat.notice(&format!("{error:#}"));
                self.settings = true;
            }
        }
    }

    /// Send at most the next admitted prompt after local and application barriers clear.
    pub(super) fn dispatch_queue(
        &mut self,
        environment: &Environment,
        allowed: bool,
        ctx: &egui::Context,
    ) {
        if self.chat.automatic_queue() {
            return;
        }
        if self.chat.busy
            || self
                .chat
                .response_style_status(self.config.personality)
                .is_some()
            || self.chat.queue_pending()
            || self.queue.command().is_some()
            || self.attaching
            || self.history_open
            || self.browser.is_some()
            || self.runtime.installing()
            || !allowed
        {
            return;
        }
        let Some(text) = self.queue.next().map(str::to_owned) else {
            return;
        };
        let result = if self.queue.hosted() {
            let id = self.queue.head_id().unwrap();
            self.chat
                .dispatch_queued(id, self.config.clone())
                .map(|()| {
                    self.composer.remember_prompt(text);
                    self.queue.submitted();
                })
        } else {
            let files = self.queue.next_attachments().to_vec();
            self.submit_input(environment, text, files, ctx)
        };
        match result {
            Ok(()) => self.queue.sent(),
            Err(error) => {
                self.queue.pause();
                self.chat
                    .notice(&format!("Queued prompt was not sent: {error:#}"));
                self.settings = true;
            }
        }
        self.schedule_poll(ctx);
    }

    /// Start read-only history discovery with captured host and interpreter authority.
    pub(super) fn open_history(&mut self, environment: &Environment, ctx: &egui::Context) {
        let launch = self.launch(environment);
        let context = ctx.clone();
        match agent::history::Browser::open_service_scoped(
            launch,
            self.history_scope,
            Arc::new(move || context.request_repaint()),
        ) {
            Ok(browser) => {
                self.browser = Some(browser);
                self.history_open = true;
            }
            Err(error) => {
                self.chat.notice(&format!("Cannot open history: {error:#}"));
                self.history_open = false;
            }
        }
    }

    /// Drain setup, discovery, history and chat updates, rejecting changed host/interpreter
    /// contexts and retaining old lifetime work until it is safe to release.
    pub(super) fn poll(
        &mut self,
        environment: &Environment,
        visible: bool,
        background_open: bool,
        ctx: &egui::Context,
    ) -> Effects {
        let mut effects = Effects::default();
        self.runtime.poll();
        let key = &environment.key;
        if self.key != *key {
            self.runtime.reset();
            self.browser = None;
            self.history_open = false;
            self.history_filter.clear();
            self.history_scope = agent::history::Scope::Project;
            self.history_delete = None;
            self.save_config();
            self.replace_chat();
            self.running.reset_context();
            self.completed = 0;
            self.draft.clear();
            self.composing = false;
            self.config = self.preferences.get(key).cloned().unwrap_or_default();
            self.arguments =
                serde_json::to_string(self.config.acp_arguments()).unwrap_or_else(|_| "[]".into());
            self.argument_error.clear();
            self.key = key.clone();
            self.catalog = None;
        }
        if !visible
            || self
                .browser
                .as_ref()
                .is_some_and(|browser| !browser.uses_interpreter(&environment.python))
        {
            self.browser = None;
            self.history_open = false;
            self.composing = false;
        }
        let mut retirement_error = None;
        for chat in &mut self.retired {
            chat.poll();
            if let Some(error) = chat.take_persistence_error() {
                retirement_error = Some(error);
            }
        }
        self.retired.retain(Chat::work_pending);
        if let Some(error) = retirement_error {
            effects.failures.push(error);
        }
        let was_busy = self.chat.busy;
        self.chat.poll();
        if self
            .catalog
            .as_ref()
            .is_some_and(|catalog| !catalog.matches(&self.config))
        {
            self.catalog = None;
        }
        if let Some(catalog) = &mut self.catalog {
            catalog.poll();
            if catalog.busy() {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
        if let Some(error) = self.chat.take_persistence_error() {
            effects.failures.push(error);
        }
        if self.attaching
            && let Some(config) = self.chat.attached_config().cloned()
        {
            self.config = config;
            self.composer
                .restore_history(self.chat.input_history().map(str::to_owned));
            self.arguments =
                serde_json::to_string(self.config.acp_arguments()).unwrap_or_else(|_| "[]".into());
            self.save_config();
            self.attaching = false;
        }
        if let Some(error) = self.poll_background(&self.launch(environment), background_open, ctx) {
            effects.failures.push(error);
        }
        if let Some(browser) = &mut self.browser {
            browser.poll();
            if browser.busy {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
            if !self.chat.busy
                && let Some(restored) = browser.take_restored()
            {
                self.replace_chat();
                self.chat.restore(restored);
                self.composer
                    .restore_history(self.chat.input_history().map(str::to_owned));
                self.browser = None;
                self.history_open = false;
                self.focus = true;
            }
        }
        if !self.chat.queue_pending()
            && self.queue.command().is_none()
            && let Some(view) = self.chat.queue_view()
        {
            self.queue.sync(view, self.chat.busy);
        }
        while let Some(prompt) = self.queue.take_dispatched() {
            self.composer.remember_prompt(prompt);
        }
        self.poll_prompt_recall(environment, ctx);
        if self.completed != self.chat.completed {
            self.completed = self.chat.completed;
            self.queue.finished();
            if !self.chat.automatic_queue() && !self.chat.last_turn_succeeded {
                self.queue.pause();
            }
            effects.refresh_documents = true;
        }
        if (was_busy || self.queue.waiting_for_reply()) && !self.chat.busy && !self.chat.connected()
        {
            self.queue.finished();
            if !self.chat.automatic_queue() {
                self.queue.pause();
            }
        }
        effects
    }
}

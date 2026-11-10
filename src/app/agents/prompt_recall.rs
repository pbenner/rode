//! Load older prompts only when Up reaches the end of the local recall cache.
//! A separate read-only connection never resumes or restarts the live agent.
use super::*;

impl EditorApp {
    /// Advance asynchronous prompt recall and deliver pages to the current composer.
    /// Cancelled anchoring releases the send hold; read failures surface as chat notices.
    pub(super) fn poll_prompt_recall(&mut self, ctx: &egui::Context) {
        let requested = self.agents.composer.take_older_request();
        // Before the first page anchors its cursor, metadata loading holds
        // new sends. Cancelling navigation must release that hold immediately,
        // even if startup or the history request is slow or unreachable.
        if !self.agents.composer.waiting_older()
            && self
                .agents
                .prompt_reader
                .as_ref()
                .is_some_and(|reader| reader.anchoring())
        {
            self.agents.prompt_reader = None;
        }
        if requested && self.agents.prompt_reader_failed {
            self.agents.prompt_reader = None;
            self.agents.prompt_reader_failed = false;
        }
        if self.agents.composer.waiting_older()
            && self.agents.prompt_reader.is_none()
            && !self.agents.prompt_reader_failed
            && !self.agents.attaching
        {
            if self.agents.config.provider.is_acp() {
                self.agents.composer.finish_older();
                return;
            }
            if let Some(thread) = self.agents.chat.conversation_id().map(str::to_owned) {
                let context = ctx.clone();
                match self.agent_launch().prompt_history(
                    &thread,
                    self.agents.composer.history().cloned().collect(),
                    Arc::new(move || context.request_repaint()),
                ) {
                    Ok(reader) => self.agents.prompt_reader = Some(reader),
                    Err(error) => {
                        self.agents
                            .chat
                            .notice(&format!("Cannot read earlier prompts: {error:#}"));
                        self.agents.prompt_reader_failed = true;
                        self.agents.composer.finish_older();
                    }
                }
            } else {
                self.agents.composer.finish_older();
            }
        } else if requested
            && let Some(reader) = &mut self.agents.prompt_reader
            && let Err(error) = reader.load_more()
        {
            self.agents
                .chat
                .notice(&format!("Cannot read earlier prompts: {error:#}"));
            self.agents.composer.finish_older();
        }
        if let Some(reader) = &mut self.agents.prompt_reader {
            reader.poll();
            let prompts = reader.take_prompts();
            if !prompts.is_empty() {
                self.agents
                    .composer
                    .prepend_older(prompts, &mut self.agents.draft, ctx);
            }
            if let Some(error) = reader.take_error() {
                self.agents.chat.notice(&error);
                self.agents.prompt_reader_failed = true;
            }
            if !reader.more() && !reader.busy() {
                self.agents.composer.finish_older();
            }
            if reader.busy() {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
    }
}

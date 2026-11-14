//! Load older prompts only when Up reaches the end of the local recall cache.
//! A separate read-only connection never resumes or restarts the live agent.
use super::*;

impl Agents {
    /// Advance asynchronous prompt recall and deliver pages to the current composer.
    /// Cancelled anchoring releases the send hold; read failures surface as chat notices.
    pub(super) fn poll_prompt_recall(
        &mut self,
        environment: &controller::Environment,
        ctx: &egui::Context,
    ) {
        let requested = self.composer.take_older_request();
        // Before the first page anchors its cursor, metadata loading holds
        // new sends. Cancelling navigation must release that hold immediately,
        // even if startup or the history request is slow or unreachable.
        if !self.composer.waiting_older()
            && self
                .prompt_reader
                .as_ref()
                .is_some_and(|reader| reader.anchoring())
        {
            self.prompt_reader = None;
        }
        if requested && self.prompt_reader_failed {
            self.prompt_reader = None;
            self.prompt_reader_failed = false;
        }
        if self.composer.waiting_older()
            && self.prompt_reader.is_none()
            && !self.prompt_reader_failed
            && !self.attaching
        {
            if self.config.provider.is_acp() {
                self.composer.finish_older();
                return;
            }
            if let Some(thread) = self.chat.conversation_id().map(str::to_owned) {
                let context = ctx.clone();
                match self.launch(environment).prompt_history(
                    &thread,
                    self.composer.history().cloned().collect(),
                    Arc::new(move || context.request_repaint()),
                ) {
                    Ok(reader) => self.prompt_reader = Some(reader),
                    Err(error) => {
                        self.chat
                            .notice(&format!("Cannot read earlier prompts: {error:#}"));
                        self.prompt_reader_failed = true;
                        self.composer.finish_older();
                    }
                }
            } else {
                self.composer.finish_older();
            }
        } else if requested
            && let Some(reader) = &mut self.prompt_reader
            && let Err(error) = reader.load_more()
        {
            self.chat
                .notice(&format!("Cannot read earlier prompts: {error:#}"));
            self.composer.finish_older();
        }
        if let Some(reader) = &mut self.prompt_reader {
            reader.poll();
            let prompts = reader.take_prompts();
            if !prompts.is_empty() {
                self.composer.prepend_older(prompts, &mut self.draft, ctx);
            }
            if let Some(error) = reader.take_error() {
                self.chat.notice(&error);
                self.prompt_reader_failed = true;
            }
            if !reader.more() && !reader.busy() {
                self.composer.finish_older();
            }
            if reader.busy() {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
    }
}

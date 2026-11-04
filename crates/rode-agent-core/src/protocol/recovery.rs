//! Explicit transport recovery preserves display state and never repeats a turn.
use super::*;

impl Chat {
    /// A failed connection can be reopened without discarding transcript or queued work.
    pub fn recoverable(&self) -> bool {
        self.recovery.is_some()
    }

    /// Reopen the captured backend, optionally choosing a new context. Reconnecting
    /// restores only the saved conversation; it never resubmits the interrupted prompt.
    pub fn recover(
        &mut self,
        launch: Launch,
        fresh: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<()> {
        ensure!(
            self.recoverable() && self.session.is_none(),
            "This agent has no failed connection to recover"
        );
        ensure!(
            !self.busy && !self.file_jobs.pending(),
            "The previous agent is still stopping"
        );
        ensure!(
            self.config.same_process(&launch.config) && self.root == launch.plan.root,
            "Recover using the original backend, project and access mode"
        );
        ensure!(
            fresh
                || !launch.config.files_only()
                || self.recovery.as_deref().is_none_or(str::is_empty),
            "File-only agents require Start new context to recreate their confined file tools. Your draft and queued prompts are kept."
        );
        launch.config.argv()?;
        self.read_only = launch.plan.project_read_only
            || (launch.config.provider != Provider::Acp
                && matches!(launch.config.access(), Access::ReadOnly | Access::FilesOnly));
        self.external = launch.plan.active && !self.read_only;
        self.network = launch.plan.prefix.iter().any(|arg| arg == "--share-net");
        self.config = launch.config.clone();
        self.fresh_recovery = fresh;
        self.start_session(launch, wake)?;
        self.busy = true;
        self.last_turn_succeeded = false;
        self.status = "Recovering agent connection…".into();
        Ok(())
    }

    pub(super) fn start_session(
        &mut self,
        launch: Launch,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<()> {
        let preparing_remote_bundle =
            launch.config.argv()?[0] == crate::AUTO_CODEX && launch.remote;
        self.file_factory = self.config.files_only().then(|| launch.factory.clone());
        self.file_wake = self.config.files_only().then(|| wake.clone());
        self.root = launch.plan.root.clone();
        self.session = Some(Session::start(launch, wake)?);
        self.status = if preparing_remote_bundle {
            "Preparing Codex on SSH host…"
        } else {
            "Starting agent…"
        }
        .into();
        self.deadline = Some(
            Instant::now() + Duration::from_secs(if preparing_remote_bundle { 420 } else { 60 }),
        );
        Ok(())
    }

    pub(super) fn resume_id(&self) -> Option<&str> {
        self.recovery
            .as_deref()
            .filter(|id| !self.fresh_recovery && !id.is_empty())
    }

    pub(super) fn start_conversation(&mut self, mut params: Value) -> Result<()> {
        if let Some(id) = self.resume_id() {
            params["threadId"] = id.into();
            params["excludeTurns"] = true.into();
            self.call("thread/resume", params, Call::Conversation)
        } else {
            self.call("thread/start", params, Call::Conversation)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_context_does_not_merge_reused_backend_item_ids() {
        for fresh in [false, true] {
            let mut chat = Chat {
                recovery: Some("saved-thread".into()),
                fresh_recovery: fresh,
                ..Default::default()
            };
            chat.push(
                "answer-1".into(),
                Role::Assistant,
                "Agent".into(),
                "Before failure".into(),
            );
            chat.calls.insert(1, Call::Conversation);
            chat.receive(json!({"id":1,"result":{"thread":{"id":if fresh {"new-thread"} else {"saved-thread"}}}})).unwrap();
            chat.append("answer-1", Role::Assistant, "Agent", "After recovery");
            let replies: Vec<_> = chat
                .messages
                .iter()
                .filter(|m| m.role == Role::Assistant)
                .map(|m| m.text.as_str())
                .collect();
            assert_eq!(
                replies,
                if fresh {
                    vec!["Before failure", "After recovery"]
                } else {
                    vec!["Before failureAfter recovery"]
                }
            );
            assert!(!chat.busy);
            assert!(!chat.recoverable());
        }
    }
}

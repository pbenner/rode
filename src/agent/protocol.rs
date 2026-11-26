//! GUI view of the shared controller. Standalone library clients retain the
//! direct transport API; editor chats opt into the native host service before
//! Send. Persistence belongs to the live conversation, never shared preferences.
use super::{Config, Launch, background::Client, history::Restored};
use anyhow::{Result, ensure};
use rode_agent_core::attachment::{self, Attachment};
pub use rode_agent_core::protocol::{Choice, Message, Prompt, Question, Role, ToolState, clip};
use rode_agent_core::service::{Command, Persistence};
use serde_json::Value;
use std::sync::Arc;

/// GUI-facing conversation with host-owned execution and disposable attachment.
/// Core state is readable; mutations must pass through this controller.
///
/// ```compile_fail
/// let mut chat = rode::agent::protocol::Chat::default();
/// chat.busy = true; // cannot bypass native lifecycle admission
/// ```
#[derive(Default)]
pub struct Chat {
    inner: rode_agent_core::protocol::Chat,
    native: Option<Client>,
    managed: bool,
    desired: Persistence,
    persistence_error: Option<String>,
    undo_pending: Option<(u64, u64)>,
}

impl std::ops::Deref for Chat {
    type Target = rode_agent_core::protocol::Chat;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl Chat {
    /// Edit only the answer drafts of the current form. The renderer receives a
    /// disposable copy: request identity, delivery state and permission choices
    /// remain owned by the controller, even if the form edits other fields.
    pub fn edit_answers<R>(&mut self, edit: impl FnOnce(&mut Prompt) -> R) -> Option<R> {
        let prompt = self.inner.prompts.front_mut()?;
        let mut draft = prompt.clone();
        let result = edit(&mut draft);
        if !prompt.sending {
            for question in &mut prompt.questions {
                if !question.answered
                    && let Some(answer) = draft.questions.iter().find(|q| q.id == question.id)
                {
                    question.answer.clone_from(&answer.answer);
                }
            }
        }
        Some(result)
    }

    /// Add a bounded presentation notice without mutating the host's lifecycle.
    pub fn notice(&mut self, text: &str) {
        self.inner.notice(text);
    }

    /// Acknowledge a stopped turn once before explicitly closing its process.
    /// Keep this metadata change with the wrapper that owns managed sessions.
    pub(crate) fn force_close(&mut self) {
        if self.inner.busy && self.native.as_ref().is_none_or(|client| !client.ending) {
            self.inner.completed += 1;
        }
        self.disconnect();
        self.notice("Agent process closed.");
    }

    /// Synthetic protocol state belongs to fixtures, never production UI code.
    #[cfg(test)]
    pub(crate) fn fixture_mut(&mut self) -> &mut rode_agent_core::protocol::Chat {
        &mut self.inner
    }

    /// Host-owned reversible changes; an absent view means no prepared undo record.
    pub fn undo_view(&self) -> Option<&rode_agent_core::undo::View> {
        self.native.as_ref()?.state.as_ref()?.chat.undo.as_ref()
    }
    /// Ask the original host to undo one exact completed prompt. Keep admission
    /// busy until this request's file-refresh revision is received. Stale idle
    /// snapshots and errors belonging to an earlier request cannot acknowledge it.
    pub fn undo_last_prompt(&mut self, launch: &Launch) -> Result<()> {
        ensure!(
            !self.inner.busy && self.undo_pending.is_none(),
            "Wait for the agent to finish"
        );
        let client = self
            .native
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Undo requires a native agent session"))?;
        ensure!(
            client.matches(launch),
            "Open this agent's original project and policy before undoing"
        );
        let view = self
            .undo_view()
            .filter(|view| !view.files.is_empty())
            .ok_or_else(|| anyhow::anyhow!("No reversible file edits are available"))?;
        let token = (view.turn, view.revision);
        client.send(Command::Undo { turn: token.0 })?;
        self.undo_pending = Some(token);
        self.inner.busy = true;
        self.inner.status = "Undoing the last prompt's file edits…".into();
        Ok(())
    }
    /// Build a synthetic chat whose selected response style differs from the applied turn style,
    /// for view/admission tests.
    #[cfg(test)]
    pub(crate) fn test_style_view(
        selected: super::Personality,
        applied: super::Personality,
    ) -> Self {
        let client = Client::test_style_view(selected, applied);
        let mut inner = rode_agent_core::protocol::Chat::default();
        inner.apply_view(client.state.as_ref().unwrap().chat.clone());
        Self {
            inner,
            native: Some(client),
            managed: true,
            desired: Persistence::On,
            persistence_error: None,
            undo_pending: None,
        }
    }
    /// Select the native controller before the first message; this starts no process.
    pub fn use_service(&mut self) {
        self.managed = true;
    }
    /// Submit through the captured local/SSH host adapter.
    pub fn submit(
        &mut self,
        launch: Launch,
        text: String,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<()> {
        self.submit_with_attachments(launch, text, vec![], wake)
    }
    /// Submit captured file contents through the same local/SSH controller.
    pub fn submit_with_attachments(
        &mut self,
        launch: Launch,
        text: String,
        attachments: Vec<Attachment>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<()> {
        attachment::validate(&text, &attachments)?;
        if !self.managed {
            return self
                .inner
                .submit_with_attachments(launch.core(), text, attachments, wake);
        }
        ensure!(
            !self.inner.busy,
            "The agent is still working. Stop it or wait for its reply."
        );
        launch.config.argv()?;
        if let Some(client) = &self.native {
            // A follow-up belongs to this controller. A changed launch context
            // or lost bridge must never implicitly submit it to a new chat.
            ensure!(
                !client.closed() && !client.ending,
                "This conversation is disconnected. Reattach it from Running, or choose New chat before sending another message. Your draft has been kept."
            );
            ensure!(
                client.matches(&launch),
                "The agent backend, project policy or interpreter changed. Choose New chat before sending with the new settings. Your draft has been kept."
            );
            client.send(Command::Submit {
                text,
                attachments,
                config: Box::new(launch.config),
            })?;
            self.inner.busy = true;
            self.inner.last_turn_succeeded = false;
            self.inner.status = "Working…".into();
            return Ok(());
        }
        let summary = attachment::summary(&text, &attachments);
        let client = Client::start(launch, Some(text), attachments, self.desired, wake)?;
        self.native = Some(client);
        self.inner.messages.push_back(Message {
            id: "pending-native-submit".into(),
            role: Role::User,
            title: "You".into(),
            text: summary,
            tool_state: None,
        });
        self.inner.busy = true;
        self.inner.last_turn_succeeded = false;
        self.inner.status = "Starting agent…".into();
        self.inner.changed = true;
        Ok(())
    }
    /// Host-owned queue metadata for this controlling view.
    pub fn queue_view(&self) -> Option<&rode_agent_core::queue::View> {
        self.native.as_ref().and_then(Client::queue_view)
    }
    /// Whether queue edits are waiting for host acknowledgement.
    pub fn queue_pending(&self) -> bool {
        self.native.as_ref().is_some_and(Client::queue_pending)
    }
    /// Whether the current controller can accept another queue edit.
    pub fn queue_capacity(&self) -> bool {
        self.native.as_ref().is_none_or(Client::queue_capacity)
    }
    /// The host must confirm the selected response style before another submission.
    pub fn response_style_status(
        &self,
        selected: super::Personality,
    ) -> Option<super::response_style::Status> {
        self.native
            .as_ref()
            .and_then(|client| client.response_style_status(selected))
    }
    /// Whether automatic queue advancement belongs to the host service.
    pub fn automatic_queue(&self) -> bool {
        self.native.is_some()
    }
    /// Transfer one queue mutation to the controlling host.
    pub fn queue_action(&mut self, operation: rode_agent_core::queue::Action) -> Result<()> {
        self.native
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("No native agent owns this queue"))?
            .queue_action(operation)
    }
    /// Dispatch a host-owned queue head atomically, without retransferring its files.
    pub fn dispatch_queued(&mut self, id: u64, config: Config) -> Result<()> {
        config.argv()?;
        self.queue_action(rode_agent_core::queue::Action::Dispatch {
            id,
            config: Box::new(config),
        })?;
        self.inner.busy = true;
        self.inner.last_turn_succeeded = false;
        self.inner.status = "Sending queued prompt…".into();
        Ok(())
    }
    /// Follow authoritative updates while keeping user question drafts local.
    pub fn poll(&mut self) {
        let Some(client) = &mut self.native else {
            self.inner.poll();
            return;
        };
        let was_pending = client.work_pending();
        if let Some(state) = client.poll() {
            if let Some((turn, revision)) = self.undo_pending
                && (state.chat.completed != turn
                    || state
                        .chat
                        .undo
                        .as_ref()
                        .is_some_and(|u| u.revision != revision))
            {
                self.undo_pending = None;
            }
            self.desired = state.info.persistent;
            self.inner.apply_view(state.chat);
            if self.undo_pending.is_some() {
                self.inner.busy = true;
                self.inner.status = "Undoing the last prompt's file edits…".into();
            }
            if !state.error.is_empty() {
                self.inner.notice(&state.error);
            }
            if client.ending && state.info.alive {
                self.inner.busy = true;
                self.inner.status = "Closing agent…".into();
            }
        }
        if let Some(error) = client.error.take() {
            if was_pending {
                self.persistence_error = Some(error.clone());
            }
            self.inner.notice(&error);
        }
        if client.closed() {
            self.undo_pending = None;
            self.inner.busy = false;
            self.inner.last_turn_succeeded = false;
            self.inner.status = if client.ending { "Stopped" } else { "Detached" }.into();
        }
    }
    /// Reopen a failed connection under its captured policy, keeping the host queue.
    pub fn recover(
        &mut self,
        launch: Launch,
        fresh: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<()> {
        ensure!(
            !self.inner.busy && !self.stopping(),
            "The previous agent is still stopping"
        );
        if let Some(client) = &mut self.native {
            ensure!(
                client.matches(&launch),
                "This agent cannot recover with the selected backend or project policy"
            );
            client.queue_action(rode_agent_core::queue::Action::Recover {
                fresh,
                config: Box::new(launch.config),
            })?;
            self.inner.busy = true;
            self.inner.status = "Recovering agent connection…".into();
            Ok(())
        } else {
            self.inner.recover(launch.core(), fresh, wake)
        }
    }
    /// Reply only to the current host-owned permission/question card.
    pub fn answer(&mut self, result: Value) -> Result<()> {
        if let Some(client) = &self.native {
            let prompt = self
                .inner
                .prompts
                .front()
                .ok_or_else(|| anyhow::anyhow!("No current agent question"))?;
            client.send(Command::Answer {
                id: prompt.id.clone(),
                result,
            })
        } else {
            self.inner.answer(result)
        }
    }
    /// Stop a turn; host-side deadlines still apply while the desktop is absent.
    pub fn cancel(&mut self) {
        if let Some(client) = &mut self.native {
            if client.state.is_none() {
                client.end();
            } else if let Err(error) = client.send(Command::Cancel) {
                self.inner.notice(&error.to_string());
            }
        } else {
            self.inner.cancel();
        }
    }
    /// Explicitly stop this agent, even if persistence is enabled.
    pub fn disconnect(&mut self) {
        if let Some(client) = &mut self.native {
            client.end();
            self.inner.busy = !client.closed();
            self.inner.status = "Closing agent…".into();
        } else {
            self.inner.disconnect();
        }
    }
    /// Leave the view under its acknowledged lifetime policy. This is used by
    /// project switches; explicit Stop and encrypted locking use disconnect.
    pub fn detach(&mut self) {
        self.undo_pending = None;
        self.native = None;
        self.desired = Persistence::default();
        self.inner.disconnect();
        self.inner.status = "Detached".into();
    }
    /// Whether initialization completed on the current controlling connection.
    pub fn connected(&self) -> bool {
        self.native
            .as_ref()
            .map(|c| !c.closed() && !c.ending && c.state.as_ref().is_some_and(|s| s.chat.connected))
            .unwrap_or_else(|| self.inner.connected())
    }
    /// Whether the current Stop is pending.
    pub fn stopping(&self) -> bool {
        self.native
            .as_ref()
            // Retain the End intent for status/history, but do not let its
            // final snapshot keep New chat disabled after the view has closed.
            .map(|c| !c.closed() && (c.ending || c.state.as_ref().is_some_and(|s| s.chat.stopping)))
            .unwrap_or_else(|| self.inner.stopping())
    }
    /// UI-side pending setup/lifetime work; agent protocol deadlines live on the host.
    pub fn has_deadline(&self) -> bool {
        self.native
            .as_ref()
            .map(Client::work_pending)
            .unwrap_or_else(|| self.inner.has_deadline())
    }
    /// Whether clean exit must wait for setup or a lifetime acknowledgement.
    pub fn work_pending(&self) -> bool {
        self.undo_pending.is_some() || self.native.as_ref().is_some_and(Client::work_pending)
    }
    /// Request cancellation of nonpersistent startup on clean exit.
    pub fn cancel_startup_for_exit(&self) {
        if let Some(client) = &self.native {
            client.cancel_startup_for_exit();
        }
    }
    /// Report a failed pending lifetime/start request to the guarded exit path.
    pub fn take_persistence_error(&mut self) -> Option<String> {
        self.persistence_error.take()
    }
    /// Last acknowledged persistence, or the explicit next-chat choice before Send.
    pub fn persistent(&self) -> bool {
        self.persistence().enabled()
    }
    /// Last acknowledged lifetime mode; new chats default to Auto without starting a process.
    pub fn persistence(&self) -> Persistence {
        self.native
            .as_ref()
            .map(Client::persistence)
            .unwrap_or(self.desired)
    }
    /// Change lifetime without restarting the model process or current turn.
    pub fn set_persistent(&mut self, persistent: bool) -> Result<()> {
        self.set_persistence(persistent.into())
    }
    /// Change Off/Auto/On in place, retaining host acknowledgement before a clean exit.
    pub fn set_persistence(&mut self, persistent: Persistence) -> Result<()> {
        if let Some(client) = &mut self.native {
            client.set_persistence(persistent)?;
        } else {
            ensure!(
                !self.inner.connected(),
                "Start a new chat to use native persistence"
            );
            self.managed = true;
            self.desired = persistent;
        }
        Ok(())
    }
    /// Whether the UI may safely change lifetime now.
    pub fn persistence_ready(&self) -> bool {
        self.native
            .as_ref()
            .is_none_or(|c| !c.work_pending() && !c.closed())
    }
    /// Current native job ID, absent before Send or for direct library clients.
    pub fn native_id(&self) -> Option<&str> {
        self.native.as_ref().map(Client::id)
    }
    /// Backend settings from a newly reattached controller.
    pub fn attached_config(&self) -> Option<&Config> {
        self.native.as_ref().and_then(Client::config)
    }
    /// Apply idle controls and response styles through the supported host protocol.
    pub fn configure_idle(&mut self, config: &Config) -> Result<()> {
        if let Some(client) = &mut self.native {
            client.configure(config)
        } else {
            self.inner.configure_idle(config)
        }
    }
    /// Adopt acknowledged saved history, stopping/detaching only the old view.
    pub fn restore(&mut self, restored: Restored) {
        self.undo_pending = None;
        match restored {
            Restored::Direct(restored) => {
                self.native = None;
                self.inner.restore(*restored);
            }
            Restored::Native(client) => {
                self.managed = true;
                if let Some(state) = &client.state {
                    self.desired = state.info.persistent;
                    self.inner.apply_view(state.chat.clone());
                }
                self.native = Some(*client);
            }
        }
    }
    /// Informational attachment generation for a saved workspace's confirmed takeover.
    pub fn native_controller(&self) -> Option<String> {
        self.native
            .as_ref()
            .and_then(|c| c.state.as_ref())
            .and_then(|s| s.info.controller.as_ref())
            .map(|c| c.id.clone())
    }
    /// Reattach a live worker without sending a model prompt.
    pub fn attach(
        &mut self,
        launch: Launch,
        info: rode_agent_core::service::Info,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.attach_client(info.persistent, Client::attach(launch, info, wake));
    }
    /// Explicitly take over the exact attachment generation shown in Running agents.
    pub fn take_over(
        &mut self,
        launch: Launch,
        info: rode_agent_core::service::Info,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.attach_client(info.persistent, Client::take_over(launch, info, wake));
    }
    fn attach_client(&mut self, persistent: super::Persistence, client: Client) {
        self.undo_pending = None;
        self.managed = true;
        self.desired = persistent;
        self.native = Some(client);
        self.inner.busy = true;
        self.inner.status = "Reattaching agent…".into();
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn answer_edits_cannot_replace_request_or_acknowledged_delivery_state() {
        let mut chat = Chat::default();
        let form: Prompt = serde_json::from_value(serde_json::json!({
            "id":401, "title":"Questions", "summary":"", "detail":"",
            "blocking":true, "sending":false, "error":"kept", "partial_answers":false, "async_message":false,
            "choices":[], "cancel":null, "questions":[
                {"id":"open", "header":"", "text":"Question", "options":[], "descriptions":[], "answer":"draft", "secret":false, "answered":false},
                {"id":"done", "header":"", "text":"Question", "options":[], "descriptions":[], "answer":"acknowledged", "secret":false, "answered":true}
            ]
        })).unwrap();
        chat.inner.prompts.push_back(form);
        assert_eq!(
            chat.edit_answers(|draft| {
                draft.id = Value::Null;
                draft.sending = true;
                draft.error.clear();
                draft.questions[0].answer = "edited".into();
                draft.questions[0].answered = true;
                draft.questions[1].answer = "replaced".into();
                7
            }),
            Some(7)
        );
        let prompt = &chat.prompts[0];
        assert_eq!(prompt.id, 401);
        assert!(!prompt.sending);
        assert_eq!(prompt.error, "kept");
        assert_eq!(prompt.questions[0].answer, "edited");
        assert!(!prompt.questions[0].answered);
        assert_eq!(prompt.questions[1].answer, "acknowledged");
        chat.inner.prompts[0].sending = true;
        chat.edit_answers(|draft| draft.questions[0].answer.clear());
        assert_eq!(chat.prompts[0].questions[0].answer, "edited");
        chat.inner.prompts.clear();
        assert!(
            chat.edit_answers(|_| panic!("Absent form must not render"))
                .is_none()
        );
    }

    #[test]
    fn replaced_views_release_their_old_undo_acknowledgement() {
        let mut chat = Chat {
            undo_pending: Some((7, 2)),
            ..Default::default()
        };
        chat.inner.busy = true;
        assert!(chat.work_pending());
        chat.detach();
        chat.poll();
        assert!(!chat.work_pending());
        assert!(!chat.busy);

        chat.undo_pending = Some((7, 2));
        chat.restore(Restored::Native(Box::new(Client::test_style_view(
            super::super::Personality::Default,
            super::super::Personality::Default,
        ))));
        assert!(chat.undo_pending.is_none());
        assert!(!chat.work_pending());
    }

    #[test]
    fn auto_is_the_inert_default_and_each_new_conversation_resets_to_it() {
        let mut chat = Chat::default();
        assert_eq!(chat.persistence(), Persistence::Auto);
        assert!(chat.native_id().is_none());
        assert!(!chat.connected());
        for mode in Persistence::ALL {
            chat.set_persistence(mode).unwrap();
            assert_eq!(chat.persistence(), mode);
            assert!(
                chat.native_id().is_none(),
                "Selecting lifetime must not start a process"
            );
            assert!(!chat.work_pending());
        }
        chat.detach();
        assert_eq!(chat.persistence(), Persistence::Auto);
    }
}

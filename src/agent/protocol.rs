//! GUI view of the shared controller. Standalone library clients retain the
//! direct transport API; editor chats opt into the native host service before
//! Send. Persistence belongs to the live conversation, never shared preferences.
use super::{Config, Launch, background::Client, history::Restored};
use anyhow::{Result, ensure};
pub use rode_agent_core::protocol::{Choice, Message, Prompt, Question, Role, clip};
use rode_agent_core::service::Command;
use serde_json::Value;
use std::sync::Arc;

/// GUI-facing conversation with host-owned execution and disposable attachment.
#[derive(Default)]
pub struct Chat {
    inner: rode_agent_core::protocol::Chat,
    native: Option<Client>,
    managed: bool,
    desired: bool,
    persistence_error: Option<String>,
}
impl std::ops::Deref for Chat {
    type Target = rode_agent_core::protocol::Chat;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl std::ops::DerefMut for Chat {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
impl Chat {
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
        if !self.managed {
            return self.inner.submit(launch.core(), text, wake);
        }
        ensure!(
            !self.inner.busy,
            "The agent is still working. Stop it or wait for its reply."
        );
        ensure!(
            !self.native.as_ref().is_some_and(|c| c.closed()
                && c.persistent()
                && c.state.as_ref().is_some_and(|s| s.info.alive)),
            "Reattach the background agent from Running, or choose New chat before sending another message"
        );
        launch.config.argv()?;
        ensure!(
            !text.trim().is_empty() && text.len() <= super::PROMPT_LIMIT,
            "Agent messages must contain 1–32768 bytes"
        );
        if let Some(client) = &self.native
            && client.matches(&launch)
        {
            client.send(Command::Submit {
                text,
                config: launch.config,
            })?;
            self.inner.busy = true;
            self.inner.status = "Working…".into();
            return Ok(());
        }
        let client = Client::start(launch, Some(text.clone()), self.desired, wake)?;
        self.native = Some(client);
        self.inner.messages.push_back(Message {
            id: "pending-native-submit".into(),
            role: Role::User,
            title: "You".into(),
            text,
        });
        self.inner.busy = true;
        self.inner.status = "Starting agent…".into();
        self.inner.changed = true;
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
            self.desired = state.info.persistent;
            self.inner.apply_view(state.chat);
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
            self.inner.busy = false;
            self.inner.status = if client.ending { "Stopped" } else { "Detached" }.into();
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
        self.native = None;
        self.desired = false;
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
            .map(|c| c.ending || c.state.as_ref().is_some_and(|s| s.chat.stopping))
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
        self.native.as_ref().is_some_and(Client::work_pending)
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
        self.native
            .as_ref()
            .map(Client::persistent)
            .unwrap_or(self.desired)
    }
    /// Change lifetime without restarting the model process or current turn.
    pub fn set_persistent(&mut self, persistent: bool) -> Result<()> {
        if let Some(client) = &mut self.native {
            client.set_persistent(persistent)?;
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
    /// Apply idle ACP selectors using the same acknowledged backend policy.
    pub fn configure_idle(&mut self, config: &Config) -> Result<()> {
        if let Some(client) = &self.native {
            client.send(Command::Configure {
                config: config.clone(),
            })
        } else {
            self.inner.configure_idle(config)
        }
    }
    /// Adopt acknowledged saved history, stopping/detaching only the old view.
    pub fn restore(&mut self, restored: Restored) {
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
    /// Reattach a live worker without sending a model prompt.
    pub fn attach(
        &mut self,
        launch: Launch,
        info: rode_agent_core::service::Info,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.managed = true;
        self.desired = info.persistent;
        self.native = Some(Client::attach(launch, info, wake));
        self.inner.busy = true;
        self.inner.status = "Reattaching agent…".into();
    }
}

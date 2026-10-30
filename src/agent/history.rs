//! Saved backend history, either through a direct library connection or a
//! disposable native-service browser. Restoration transfers the controlling
//! connection only after the backend has acknowledged it.
use super::{Launch, background::Client};
use anyhow::{Context, Result};
pub use rode_agent_core::history::Entry;
use rode_agent_core::service::{Command, HistoryView};
use std::sync::Arc;
/// Acknowledged restored conversation; consumed exactly once by Chat.
pub enum Restored {
    /// Direct library client restoration.
    Direct(Box<rode_agent_core::history::Restored>),
    /// Native host-owned controller restoration.
    Native(Box<Client>),
}
/// Explicit project-host history browser; never requests model inference to list.
pub struct Browser {
    view: HistoryView,
    direct: Option<rode_agent_core::history::Browser>,
    native: Option<Client>,
    interpreter: String,
}
impl std::ops::Deref for Browser {
    type Target = HistoryView;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}
impl Browser {
    /// Start through the direct local/SSH adapter for standalone library clients.
    pub fn open(launch: Launch, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let interpreter = launch.python.clone();
        let inner = rode_agent_core::history::Browser::open(launch.core(), wake)?;
        Ok(Self {
            view: inner.view(),
            direct: Some(inner),
            native: None,
            interpreter,
        })
    }
    /// Open a disposable browser on the native host service, without inference.
    pub fn open_service(launch: Launch, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let interpreter = launch.python.clone();
        let native = Client::start(launch, None, false, wake)?;
        Ok(Self {
            view: HistoryView {
                busy: true,
                status: "Loading saved conversations…".into(),
                ..Default::default()
            },
            direct: None,
            native: Some(native),
            interpreter,
        })
    }
    /// Drain bounded browser updates and preserve the current chat on failure.
    pub fn poll(&mut self) {
        if let Some(direct) = &mut self.direct {
            direct.poll();
            self.view = direct.view();
        }
        if let Some(native) = &mut self.native {
            if let Some(state) = native.poll() {
                if let Some(view) = state.history {
                    self.view = view;
                } else if !state.chat.connected {
                    self.view.status = state.error;
                    self.view.busy = false;
                } else {
                    self.view.busy = false;
                }
            }
            if let Some(error) = native.error.take() {
                self.view.status = error;
                self.view.busy = false;
                self.view.can_resume = false;
            }
        }
    }
    /// Transfer a successfully restored connection into the current chat.
    pub fn take_restored(&mut self) -> Option<Restored> {
        if let Some(direct) = &mut self.direct {
            return direct
                .take_restored()
                .map(|restored| Restored::Direct(Box::new(restored)));
        }
        if self.native.as_ref().is_some_and(|client| {
            client
                .state
                .as_ref()
                .is_some_and(|s| s.history.is_none() && s.chat.connected)
        }) {
            return self
                .native
                .take()
                .map(|client| Restored::Native(Box::new(client)));
        }
        None
    }
    /// Request another bounded saved-conversation page.
    pub fn load_more(&mut self) -> Result<()> {
        if let Some(direct) = &mut self.direct {
            direct.load_more()?;
            self.view = direct.view();
            Ok(())
        } else {
            self.native
                .as_ref()
                .context("History browser is closed")?
                .send(Command::HistoryMore)?;
            self.view.busy = true;
            Ok(())
        }
    }
    /// Restore exactly the backend row selected by the user.
    pub fn resume(&mut self, id: &str) -> Result<()> {
        if let Some(direct) = &mut self.direct {
            direct.resume(id)?;
            self.view = direct.view();
            Ok(())
        } else {
            self.native
                .as_ref()
                .context("History browser is closed")?
                .send(Command::HistoryResume { id: id.into() })?;
            self.view.busy = true;
            Ok(())
        }
    }
    /// Invalidate browsing when the interpreter context changes.
    pub fn uses_interpreter(&self, interpreter: &str) -> bool {
        self.interpreter == interpreter
    }
}

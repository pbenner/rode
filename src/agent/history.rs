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
    diagnostics: String,
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
            diagnostics: String::new(),
        })
    }
    /// Open a disposable browser on the native host service, without inference.
    pub fn open_service(launch: Launch, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let interpreter = launch.python.clone();
        let native = Client::start(launch, None, vec![], false, wake)?;
        Ok(Self {
            view: HistoryView {
                busy: true,
                status: "Loading saved conversations…".into(),
                ..Default::default()
            },
            direct: None,
            native: Some(native),
            interpreter,
            diagnostics: String::new(),
        })
    }
    /// Drain bounded browser updates and preserve the current chat on failure.
    pub fn poll(&mut self) {
        let mut updated = false;
        if let Some(direct) = &mut self.direct {
            direct.poll();
            self.view = direct.view();
            updated = true;
        }
        if let Some(native) = &mut self.native {
            if let Some(state) = native.poll() {
                updated = true;
                if let Some(view) = state.history {
                    self.view = view;
                    if !state.error.is_empty() {
                        self.view.status = state.error;
                        self.view.busy = false;
                        self.view.deleting = false;
                    }
                } else if !state.chat.connected {
                    self.view.status = state.error;
                    self.view.busy = false;
                } else {
                    self.view.busy = false;
                }
            }
            if let Some(error) = native.error.take() {
                updated = true;
                self.view.status = error;
                self.view.busy = false;
                self.view.can_resume = false;
            }
        }
        if updated {
            self.describe_status();
        }
    }
    /// Raw backend error and startup warnings behind an explained history failure.
    pub fn diagnostics(&self) -> &str {
        &self.diagnostics
    }
    fn describe_status(&mut self) {
        self.diagnostics.clear();
        if let Some(explanation) = writer_conflict(&self.view.status) {
            self.diagnostics = std::mem::replace(&mut self.view.status, explanation);
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
    /// Delete one saved chat or all pages for this project/backend, keeping open chats.
    pub fn delete(&mut self, id: Option<&str>, protected: Vec<String>) -> Result<()> {
        if let Some(direct) = &mut self.direct {
            direct.delete(id, protected)?;
            self.view = direct.view();
        } else {
            anyhow::ensure!(
                !self.view.busy && self.view.can_delete,
                "History deletion is unavailable"
            );
            self.native
                .as_ref()
                .context("History browser is closed")?
                .send(Command::HistoryDelete {
                    id: id.map(str::to_owned),
                    protected,
                })?;
            self.view.busy = true;
            self.view.deleting = true;
            self.view.status = "Deleting saved chats…".into();
        }
        self.diagnostics.clear();
        Ok(())
    }
    /// Invalidate browsing when the interpreter context changes.
    pub fn uses_interpreter(&self, interpreter: &str) -> bool {
        self.interpreter == interpreter
    }
}

/// Interpret the deletion failure at the desktop boundary, including replies
/// from older services which may still own detached agents after an upgrade.
fn writer_conflict(status: &str) -> Option<String> {
    let primary = status.lines().next()?;
    let (count, error) = primary
        .strip_prefix("Deleted ")?
        .split_once(" chats before this operation stopped. ")?;
    let count: usize = count.parse().ok()?;
    if !error
        .strip_prefix("Agent history: ")?
        .contains("already has an active writer")
    {
        return None;
    }
    let progress = match count {
        0 => "No chats were deleted.".into(),
        1 => "Deleted 1 chat before stopping. Remaining chats were kept.".into(),
        count => format!("Deleted {count} chats before stopping. Remaining chats were kept."),
    };
    Some(format!(
        "{progress} An agent process still has this chat or a child chat open, even if idle or detached. In Running, find the owning agent and choose End; for a session in another client, end it there. Then refresh history and retry. Detaching or closing a persistent window leaves the session running."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_conflicts_explain_detached_sessions_and_preserve_deletion_counts() {
        for (count, progress) in [
            (0, "No chats were deleted."),
            (1, "Deleted 1 chat before stopping."),
            (4, "Deleted 4 chats before stopping."),
        ] {
            let raw = format!(
                "Deleted {count} chats before this operation stopped. Agent history: thread example-thread already has an active writer\nERROR: bubblewrap was not found; using the bundled copy"
            );
            let explanation = writer_conflict(&raw).unwrap();
            assert!(explanation.starts_with(progress));
            assert!(explanation.contains("idle or detached"));
            assert!(explanation.contains("In Running, find the owning agent and choose End"));
            assert!(explanation.contains("closing a persistent window leaves the session running"));
            assert!(explanation.contains("refresh history and retry"));
            assert!(!explanation.contains("bubblewrap"));
            assert!(!explanation.contains("example-thread"));
        }
    }

    #[test]
    fn unrelated_errors_and_warnings_do_not_claim_a_writer_conflict() {
        for status in [
            "Deleted 0 chats before this operation stopped. Agent history: Delete failed",
            "Deleted 0 chats before this operation stopped. Agent history: Permission denied\nthread example-thread already has an active writer",
            "Deleted 1 saved chats. Refresh failed. Agent history: thread example-thread already has an active writer",
        ] {
            assert!(writer_conflict(status).is_none());
        }
    }
}

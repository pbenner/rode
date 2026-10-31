//! Host-owned pending prompts. Views carry metadata only, so reattachment never
//! retransfers attachment payloads and every queued submission is removed atomically.
use crate::{
    Config,
    attachment::{self, Attachment},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Maximum waiting prompts per conversation.
pub const LIMIT: usize = 16;
/// One queue row, excluding the host-owned file contents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    /// Conversation-local, never reused identity.
    pub id: u64,
    /// Original user prompt.
    pub text: String,
    /// Attached filenames for display.
    pub files: Vec<String>,
}
/// Bounded queue presentation and acknowledgement, restored on reattachment.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct View {
    /// Ordered waiting prompts.
    pub items: Vec<Item>,
    /// Highest allocated item identity.
    pub serial: u64,
    /// Explicit or failure-induced pause.
    pub paused: bool,
    /// An item held while its controlling view edits it.
    pub editing: Option<u64>,
    /// Last processed queue request, including rejected requests.
    pub acknowledged: String,
}
/// A queue mutation, ordered and acknowledged by its host controller.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Action {
    /// Capture a complete immutable prompt on the host.
    Push {
        /// New item identity allocated by the controlling view.
        id: u64,
        /// Exact prompt text.
        text: String,
        /// Portable file snapshots.
        attachments: Vec<Attachment>,
    },
    /// Pause or explicitly resume dispatch.
    Pause {
        /// Requested state.
        paused: bool,
    },
    /// Hold the queue before editing an item.
    Edit {
        /// Existing item identity.
        id: u64,
    },
    /// Commit an edited prompt without changing its file snapshots.
    Save {
        /// Existing item identity.
        id: u64,
        /// Replacement prompt text.
        text: String,
    },
    /// Release the edit hold without changing the prompt.
    CancelEdit {
        /// Existing item identity.
        id: u64,
    },
    /// Delete exactly one waiting item.
    Remove {
        /// Existing item identity.
        id: u64,
    },
    /// Move an item by one place.
    Move {
        /// Existing item identity.
        id: u64,
        /// Move toward the front when true.
        earlier: bool,
    },
    /// Submit the current head using the controlling view's current settings.
    Dispatch {
        /// Expected queue head, preventing stale or duplicate sends.
        id: u64,
        /// Current agent settings.
        config: Box<Config>,
    },
}
struct Prompt {
    row: Item,
    attachments: Vec<Attachment>,
}
/// In-memory queue belonging to the live agent, including its file snapshots.
#[derive(Default)]
pub struct Queue {
    items: VecDeque<Prompt>,
    view: View,
}
impl Queue {
    /// Snapshot only bounded display data; payloads stay on the host.
    pub fn view(&self) -> View {
        View {
            items: self.items.iter().map(|p| p.row.clone()).collect(),
            ..self.view.clone()
        }
    }
    /// Record request receipt even when its operation was rejected.
    pub fn acknowledge(&mut self, request: String) {
        self.view.acknowledged = request;
    }
    /// Pause remaining work after Stop, a failed turn or a failed dispatch.
    pub fn pause(&mut self) {
        if !self.items.is_empty() {
            self.view.paused = true;
        }
    }
    /// Inspect an eligible head; its identity must still match at dispatch time.
    pub fn head(&self, id: u64) -> Result<(String, Vec<Attachment>)> {
        ensure!(
            !self.view.paused && self.view.editing.is_none(),
            "The prompt queue is paused or being edited."
        );
        let item = self
            .items
            .front()
            .filter(|p| p.row.id == id)
            .ok_or_else(|| {
                anyhow::anyhow!("The queued prompt changed; refresh its current state.")
            })?;
        Ok((item.row.text.clone(), item.attachments.clone()))
    }
    /// Remove the head only after the agent accepted the same prompt.
    pub fn sent(&mut self, id: u64) {
        if self.items.front().is_some_and(|p| p.row.id == id) {
            self.items.pop_front();
        }
        if self.items.is_empty() {
            self.view.paused = false;
        }
    }
    /// Apply a queue edit. Dispatch is performed atomically by the controller.
    pub fn apply(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Push {
                id,
                text,
                attachments,
            } => {
                attachment::validate(&text, &attachments)?;
                ensure!(
                    self.items.len() < LIMIT,
                    "The prompt queue is full (16 messages)."
                );
                ensure!(
                    id > self.view.serial,
                    "This queue item identity was already used."
                );
                self.view.serial = id;
                self.items.push_back(Prompt {
                    row: Item {
                        id,
                        text,
                        files: attachments.iter().map(|f| f.name().into()).collect(),
                    },
                    attachments,
                });
            }
            Action::Pause { paused } => self.view.paused = paused,
            Action::Edit { id } => {
                ensure!(
                    self.items.iter().any(|p| p.row.id == id),
                    "This queued prompt is no longer waiting."
                );
                ensure!(
                    self.view.editing.is_none(),
                    "Another queued prompt is being edited."
                );
                self.view.editing = Some(id);
            }
            Action::Save { id, text } => {
                ensure!(
                    self.view.editing == Some(id),
                    "This queued prompt is no longer being edited."
                );
                let item = self
                    .items
                    .iter_mut()
                    .find(|p| p.row.id == id)
                    .ok_or_else(|| anyhow::anyhow!("This queued prompt is no longer waiting."))?;
                attachment::validate(&text, &item.attachments)?;
                item.row.text = text;
                self.view.editing = None;
            }
            Action::CancelEdit { id } => {
                if self.view.editing == Some(id) {
                    self.view.editing = None;
                }
            }
            Action::Remove { id } => {
                self.items.retain(|p| p.row.id != id);
                if self.view.editing == Some(id) {
                    self.view.editing = None;
                }
                if self.items.is_empty() {
                    self.view.paused = false;
                }
            }
            Action::Move { id, earlier } => {
                let index = self
                    .items
                    .iter()
                    .position(|p| p.row.id == id)
                    .ok_or_else(|| anyhow::anyhow!("This queued prompt is no longer waiting."))?;
                let other = if earlier {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(self.items.len() - 1)
                };
                self.items.swap(index, other);
            }
            Action::Dispatch { .. } => anyhow::bail!("Dispatch requires the agent controller."),
        }
        Ok(())
    }
}

//! Bounded native-service messages. Context and permissions are captured on
//! creation; a later attachment cannot change the host or enclosing sandbox.
use crate::{
    Config, Plan, Provider, controls,
    history::Entry,
    protocol::{Message, Prompt},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

/// Captured host-side launch, prepared by the desktop's existing project adapter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spec {
    /// Random native agent identity.
    pub id: String,
    /// Human-readable initial title, without a transcript copy on disk.
    pub name: String,
    /// Expected prepared-policy fingerprint.
    pub policy: String,
    /// Actual sandbox mode label.
    pub mode: String,
    /// Backend and user-selected access preferences.
    pub config: Config,
    /// Actual host-side sandbox prefix and root.
    pub plan: Plan,
    /// Fully resolved installed executable, including a verified optional runtime.
    pub executable: String,
    /// Complete caller environment; each agent is independent of the first service client.
    pub env: BTreeMap<String, String>,
    /// Project interpreter identity for history invalidation.
    pub python: String,
    /// Explicit opt-in lifetime after the controlling view closes.
    pub persistent: bool,
    /// First prompt, absent only for explicit history browsing.
    pub text: Option<String>,
    /// Immutable file contents for the first prompt; absent for older clients.
    #[serde(default)]
    pub attachments: Vec<crate::attachment::Attachment>,
}
/// Discoverable live agent metadata, separate from backend-owned saved chats.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    /// Whether this host controller accepts portable prompt attachments.
    #[serde(default)]
    pub supports_attachments: bool,
    /// Whether queued prompts are held by this live controller.
    #[serde(default)]
    pub supports_queue: bool,
    /// Whether this controller dispatches queued prompts independently of desktop views.
    #[serde(default)]
    pub automatic_queue: bool,
    /// Native agent identity.
    pub id: String,
    /// Display title.
    pub name: String,
    /// Original host project folder.
    pub root: String,
    /// Original policy fingerprint required for attachment.
    pub policy: String,
    /// Actual launch mode.
    pub mode: String,
    /// Backend protocol/provider.
    pub provider: Provider,
    /// Whether the worker/backend is live.
    pub alive: bool,
    /// Whether a turn is in progress.
    pub busy: bool,
    /// Compact current status.
    pub status: String,
    /// Whether a controlling desktop is attached.
    pub attached: bool,
    /// Keep running after that view closes.
    pub persistent: bool,
    /// Creation time in Unix seconds.
    pub created: u64,
}
/// Current chat presentation; bounded exactly as the foreground controller.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChatView {
    /// Backend conversation identity, used to protect open chats from history deletion.
    #[serde(default)]
    pub conversation: String,
    /// Recent messages, never saved as RODE preferences.
    pub messages: VecDeque<Message>,
    /// Permission/questions waiting for explicit input.
    pub prompts: VecDeque<Prompt>,
    /// Current status.
    pub status: String,
    /// Whether a turn/start is in progress.
    pub busy: bool,
    /// Completed turns used for clean-buffer refresh.
    pub completed: u64,
    /// False for older services, failures and interrupted turns.
    #[serde(default)]
    pub last_turn_succeeded: bool,
    /// Backend-advertised models.
    pub models: Vec<controls::Model>,
    /// Backend-advertised ACP settings.
    pub controls: Vec<controls::Control>,
    /// Whether conversation initialization has completed.
    pub connected: bool,
    /// Whether cancellation is awaiting acknowledgement/escalation.
    pub stopping: bool,
    /// Acknowledged/requested configuration for a reattached UI.
    pub config: Config,
}
/// Explicit history browsing state; no inference is requested by listing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HistoryView {
    /// Bounded saved-chat rows.
    pub entries: Vec<Entry>,
    /// Current support/load/error status.
    pub status: String,
    /// Whether listing/restoring is in progress.
    pub busy: bool,
    /// Whether the backend supports restoring a row.
    pub can_resume: bool,
    /// Whether the backend provides saved-conversation deletion.
    #[serde(default)]
    pub can_delete: bool,
    /// Whether an explicit delete operation is pending.
    #[serde(default)]
    pub deleting: bool,
    /// Whether another page can be requested.
    pub more: bool,
}
/// Current authoritative state streamed by the process host to one controlling UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    /// Host-owned waiting prompts, excluding attachment data.
    #[serde(default)]
    pub queue: crate::queue::View,
    /// Live worker identity and lifetime policy.
    pub info: Info,
    /// Current chat, or the current history browser when present.
    pub chat: ChatView,
    /// Present only during explicit history work.
    pub history: Option<HistoryView>,
    /// Error for the most recent rejected UI action; never auto-retried.
    pub error: String,
}
/// Streamed view update. Only changed messages are repeated; ordering also
/// removes trimmed items, keeping long SSH conversations bounded and efficient.
#[derive(Serialize, Deserialize)]
pub struct Update {
    /// Current metadata/controls and changed transcript items.
    pub state: State,
    /// Complete bounded message order after applying the changed items.
    pub order: Vec<String>,
}
impl Update {
    /// Reassemble a full view from one ordered, reliable stream.
    pub fn apply(mut self, previous: Option<&State>) -> anyhow::Result<State> {
        anyhow::ensure!(
            self.order.len() <= 128 && self.state.chat.messages.len() <= 128,
            "Agent transcript update exceeds its item limit"
        );
        let mut messages: BTreeMap<_, _> = previous
            .into_iter()
            .flat_map(|s| s.chat.messages.iter())
            .map(|m| (m.id.clone(), m.clone()))
            .collect();
        for message in std::mem::take(&mut self.state.chat.messages) {
            messages.insert(message.id.clone(), message);
        }
        for id in self.order {
            self.state.chat.messages.push_back(
                messages
                    .remove(&id)
                    .ok_or_else(|| anyhow::anyhow!("Agent transcript update is missing an item"))?,
            );
        }
        Ok(self.state)
    }
}
/// Explicit user actions; permissions and host policy cannot be widened by attachment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Command {
    /// Save or dispatch a conversation-owned queued prompt.
    Queue {
        /// Unique controlling-view request used to acknowledge durable ownership.
        request: String,
        /// Ordered queue operation.
        operation: crate::queue::Action,
    },
    /// Submit the next prompt using the current UI settings.
    Submit {
        /// Exact user message, bounded to PROMPT_LIMIT.
        text: String,
        /// File snapshots submitted with this turn.
        #[serde(default)]
        attachments: Vec<crate::attachment::Attachment>,
        /// Current model, reply and permission settings.
        config: Config,
    },
    /// Apply idle ACP selectors without a prompt.
    Configure {
        /// Idle controls to acknowledge before another prompt.
        config: Config,
    },
    /// Answer exactly the displayed card, rejecting stale answers.
    Answer {
        /// Exact request identity, rejecting stale cards.
        id: Value,
        /// Explicit user answer or permission choice.
        result: Value,
    },
    /// Interrupt the current turn, with host-side timeout escalation.
    Cancel,
    /// Stop the process; distinct from detaching a persistent view.
    End,
    /// Change last-view lifetime in place.
    Persistence {
        /// Requested last-view lifetime.
        persistent: bool,
    },
    /// Load the next saved-conversation page.
    HistoryMore,
    /// Restore one backend-owned saved conversation.
    HistoryResume {
        /// Backend-owned saved conversation ID.
        id: String,
    },
    /// Delete one listed conversation or all saved chats for this project/backend.
    HistoryDelete {
        /// A listed identity, or None for all pages (including archived Codex chats).
        id: Option<String>,
        /// Open frontend conversations to keep, in addition to host-owned controllers.
        protected: Vec<String>,
    },
}

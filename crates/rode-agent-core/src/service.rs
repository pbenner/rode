//! Bounded native-service messages. Context and permissions are captured on
//! creation; a later attachment cannot change the host or enclosing sandbox.
use crate::{
    Config, Plan, Provider, controls,
    history::{Entry, Owner},
    protocol::{Message, Prompt},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

/// Lifetime of an agent after its controlling view disconnects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Persistence {
    /// Stop when the controlling view disconnects.
    Off,
    /// Finish detached work, including queued prompts and pending questions, then stop.
    #[default]
    Auto,
    /// Retain the conversation until explicitly ended, including while idle.
    On,
}
impl Persistence {
    /// Choices presented for each conversation; new chats start in Auto.
    pub const ALL: [Self; 3] = [Self::Auto, Self::On, Self::Off];
    /// Compact user-facing lifetime choice.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Auto => "Auto",
            Self::On => "On",
        }
    }
    /// Whether closing a view may leave accepted work running on its host.
    pub fn enabled(self) -> bool {
        self != Self::Off
    }
}
impl From<bool> for Persistence {
    fn from(persistent: bool) -> Self {
        if persistent { Self::On } else { Self::Off }
    }
}
impl Serialize for Persistence {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Off | Self::On => serializer.serialize_bool(*self == Self::On),
        }
    }
}
impl<'de> Deserialize<'de> for Persistence {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Persistence;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("true, false, or \"auto\"")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(value.into())
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "auto" => Ok(Persistence::Auto),
                    _ => Err(E::unknown_variant(value, &["auto"])),
                }
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

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
    /// Full host policy for native file-only helpers.
    pub file_tool_plan: Option<serde_json::Value>,
    /// Fully resolved installed executable, including a verified optional runtime.
    pub executable: String,
    /// Complete caller environment; each agent is independent of the first service client.
    pub env: BTreeMap<String, String>,
    /// Project interpreter identity for history invalidation.
    pub python: String,
    /// Lifetime after the controlling view closes; omitted values use Auto.
    #[serde(default)]
    pub persistent: Persistence,
    /// First prompt, absent only for explicit history browsing.
    pub text: Option<String>,
    /// Immutable file contents for the first prompt.
    pub attachments: Vec<crate::attachment::Attachment>,
}
/// Discoverable live agent metadata, separate from backend-owned saved chats.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    /// A disposable history connection, retained for routing but hidden from Running.
    pub history_browser: bool,
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
    /// Lifetime after that view closes.
    pub persistent: Persistence,
    /// Creation time in Unix seconds.
    pub created: u64,
}
/// Current chat presentation; bounded exactly as the foreground controller.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChatView {
    /// A failed backend is retained until explicit recovery or termination.
    pub recoverable: bool,
    /// Backend conversation identity, used to protect open chats from history deletion.
    pub conversation: String,
    /// Recent messages, never saved as RODE preferences.
    pub messages: VecDeque<Message>,
    /// Recent accepted/replayed prompts, independent of transcript eviction.
    /// Omitted when unchanged in a delta update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_history: Option<Vec<String>>,
    /// Permission/questions waiting for explicit input.
    pub prompts: VecDeque<Prompt>,
    /// Current status.
    pub status: String,
    /// Whether a turn/start is in progress.
    pub busy: bool,
    /// Completed turns used for clean-buffer refresh.
    pub completed: u64,
    /// False for failures and interrupted turns.
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
    /// Explicit listing scope; older hosts retain project-only browsing.
    #[serde(default)]
    pub scope: crate::history::Scope,
    /// Whether the backend acknowledged history-list support.
    #[serde(default)]
    pub can_list: bool,
    /// Bounded saved-chat rows.
    pub entries: Vec<Entry>,
    /// Known owners on this host; other chats are checked when restored.
    pub owners: BTreeMap<String, Owner>,
    /// Additional process output, kept separate from the operation failure.
    pub backend_output: String,
    /// Current support/load/error status.
    pub status: String,
    /// Whether listing/restoring is in progress.
    pub busy: bool,
    /// Whether the backend supports restoring a row.
    pub can_resume: bool,
    /// Whether the backend provides saved-conversation deletion.
    pub can_delete: bool,
    /// Whether an explicit delete operation is pending.
    pub deleting: bool,
    /// Whether another page can be requested.
    pub more: bool,
}
/// Current authoritative state streamed by the process host to one controlling UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    /// Host-owned waiting prompts, excluding attachment data.
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
/// Streamed view update. Only changed messages and input history are repeated;
/// ordering also removes trimmed items, keeping SSH conversations bounded and efficient.
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
        if self.state.chat.input_history.is_none() {
            self.state.chat.input_history =
                previous.and_then(|state| state.chat.input_history.clone());
        }
        anyhow::ensure!(
            self.state.chat.input_history.is_some(),
            "Initial agent update is missing prompt history"
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
        attachments: Vec<crate::attachment::Attachment>,
        /// Current model, reply and permission settings.
        config: Box<Config>,
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
        persistent: Persistence,
    },
    /// Load the next saved-conversation page.
    HistoryMore,
    /// Change only the saved-conversation metadata scope.
    HistoryScope {
        /// Current project or all projects visible to the captured backend/store.
        scope: crate::history::Scope,
    },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifetime_modes_keep_explicit_boolean_choices_and_default_to_auto() {
        assert_eq!(Persistence::default(), Persistence::Auto);
        for (wire, mode) in [
            (serde_json::json!(false), Persistence::Off),
            (serde_json::json!(true), Persistence::On),
            (serde_json::json!("auto"), Persistence::Auto),
        ] {
            assert_eq!(
                serde_json::from_value::<Persistence>(wire.clone()).unwrap(),
                mode
            );
            assert_eq!(serde_json::to_value(mode).unwrap(), wire);
        }
        assert!(serde_json::from_value::<Persistence>(serde_json::json!("unknown")).is_err());
        let spec = serde_json::json!({
            "id": "fixture", "name": "Fixture", "policy": "a".repeat(64),
            "mode": "NORMAL MODE", "config": Config::default(),
            "plan": Plan { root: "/project".into(), ..Default::default() },
            "executable": "/bin/true", "file_tool_plan": null, "env": {},
            "python": "off", "text": "work", "attachments": []
        });
        assert_eq!(
            serde_json::from_value::<Spec>(spec).unwrap().persistent,
            Persistence::Auto
        );
    }
}

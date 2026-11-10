//! Tagged service-control requests and replies, independent of PTY ownership.
//!
//! These are the shared wire contracts for both installed and standalone helpers.
//! The crate root re-exports them to preserve the public client API. Changes to
//! wire compatibility or required service protections advance [`PROTOCOL`].
use crate::tunnels;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Required service protocol and runtime-directory generation.
/// Generation 24 also requires transport signalling to finish before reaping.
pub const PROTOCOL: u32 = 24;

/// Host-service metadata for one terminal; no live PTY handle is transferred to clients.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    /// Opaque native session identifier.
    pub id: String,
    /// User-visible session name.
    pub name: String,
    /// Project working directory on the service host.
    pub root: String,
    /// Fingerprint of the creating workspace's actual sandbox plan.
    pub policy: String,
    /// Display label for the actual launch mode.
    pub mode: String,
    /// Whether the shell is still running.
    pub alive: bool,
    /// Number of currently attached terminal views.
    pub clients: usize,
    /// Creation time in Unix seconds.
    pub created: u64,
    /// Exit status when known; completed records are normally reaped promptly.
    pub exit_code: Option<u32>,
    /// Keep running with no attached views.
    pub persistent: bool,
    /// Scheduler job name for a managed interactive allocation, retained across
    /// renames and shared attachments.
    pub allocation: Option<String>,
}
/// Owner-only Unix-socket control protocol, serialized as JSON with an `action` tag.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    /// Discover daemon-owned reverse SSH tunnels without starting a service.
    Tunnels,
    /// Start or explicitly replace one daemon-owned tunnel.
    StartTunnel {
        /// Stable request identity for idempotent retries.
        id: String,
        /// Relay, host identity and explicitly selected listener scope.
        spec: tunnels::Spec,
    },
    /// Stop a tunnel and all of its active forwarding channels.
    StopTunnel {
        /// Exact tunnel identity; unrelated sessions are retained.
        id: String,
    },
    /// Report the required protocol before accepting client operations.
    Protocol,
    /// Discover retained notebook and debugger runtimes.
    Runtimes,
    /// Create a captured host-owned runtime.
    CreateRuntime {
        /// Immutable launch and lifetime context.
        spec: Box<rode_runtime_core::service::Spec>,
    },
    /// Attach the exclusive controlling desktop.
    AttachRuntime {
        /// Native runtime identity.
        id: String,
        /// Original workspace policy fingerprint.
        policy: String,
    },
    /// End a runtime and discard retained output.
    EndRuntime {
        /// Native runtime identity.
        id: String,
        /// Original workspace policy fingerprint.
        policy: String,
    },
    /// List running sessions without keeping an otherwise idle service alive.
    List,
    /// List live agents without starting a service or inference.
    Agents,
    /// Create a host-owned conversation controller on explicit Send/History.
    CreateAgent {
        /// Captured and validated host launch context.
        spec: Box<rode_agent_core::service::Spec>,
    },
    /// Attach the sole controlling desktop to a live agent.
    AttachAgent {
        /// Native agent identity.
        id: String,
        /// Original policy fingerprint.
        policy: String,
    },
    /// Explicitly terminate one agent and its scoped process tree.
    EndAgent {
        /// Native agent identity.
        id: String,
        /// Original policy fingerprint.
        policy: String,
    },
    /// Create a supervised PTY shell with explicit lifetime and environment.
    Create {
        /// Unique identifier for the new session.
        id: String,
        /// Initial user-visible session name.
        name: String,
        /// Initial working directory on the service host.
        root: String,
        /// Workspace policy fingerprint checked by later mutations/attachments.
        policy: String,
        /// Actual launch mode label for discovery.
        mode: String,
        /// Executable and literal arguments, including any enforcing sandbox prefix.
        argv: Vec<String>,
        /// Startup shell input sent once, never replayed when reattaching.
        initial_input: String,
        /// Environment captured from the creating caller.
        env: BTreeMap<String, String>,
        /// Whether to retain the shell after its final view disconnects.
        persistent: bool,
        /// Unique scheduler job name, if this terminal owns an allocation.
        allocation: Option<String>,
    },
    /// Attach a streaming view after verifying the workspace policy fingerprint.
    Attach {
        /// Session to attach.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
    },
    /// Rename a terminal without restarting its shell.
    Rename {
        /// Session to rename.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
        /// Replacement display name.
        name: String,
    },
    /// Change last-view lifetime without restarting the shell.
    SetPersistence {
        /// Session to update.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
        /// Desired persistence after the final view disconnects.
        persistent: bool,
    },
    /// Explicitly terminate one session and its owned descendants.
    End {
        /// Session to terminate.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
    },
    /// Stop the service only when no running sessions prevent shutdown.
    Shutdown,
}
/// Tagged JSON reply to a session-service control request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Response {
    /// Discoverable daemon-owned reverse SSH tunnels.
    Tunnels(Vec<tunnels::Info>),
    /// Started/replaced tunnel, which may still be connecting.
    Tunnel(Box<tunnels::Info>),
    /// Exact protocol implemented by this service.
    Protocol(u32),
    /// Discoverable retained notebook/debugger controllers.
    Runtimes(Vec<rode_runtime_core::service::Info>),
    /// Created notebook/debugger identity.
    Runtime(Box<rode_runtime_core::service::Info>),
    /// Snapshot of discoverable running terminals.
    Sessions(Vec<Info>),
    /// Live native agent controllers.
    Agents(Vec<rode_agent_core::service::Info>),
    /// Created native agent metadata.
    Agent(rode_agent_core::service::Info),
    /// Metadata for the created or updated terminal.
    Session(Info),
    /// Successful operation without a result payload.
    Ok,
    /// User-visible service error; use [`Self::checked`] to propagate it.
    Error(String),
}
impl Response {
    /// Convert a service error reply into an `anyhow` error.
    pub fn checked(self) -> Result<Self> {
        if let Self::Error(error) = &self {
            bail!("{error}");
        }
        Ok(self)
    }
}

//! Captured runtime identities, commands and bounded reattachment state.
use crate::{debugger, notebook};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Type of host-owned computation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Stateful Jupyter kernel.
    Notebook,
    /// Installed DAP debugger.
    Debugger,
}
/// Immutable launch context, resolved on the selected project host.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spec {
    /// Saved source digests captured by the debugger's creating desktop.
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
    /// Random native identity.
    pub id: String,
    /// User-visible name.
    pub name: String,
    /// Original canonical project root.
    pub root: String,
    /// Original sandbox fingerprint.
    pub policy: String,
    /// Actual sandbox label.
    pub mode: String,
    /// Document path for notebook discovery; no source file is modified.
    pub document: String,
    /// Installed executable and literal arguments, including the compute launcher.
    pub argv: Vec<String>,
    /// Enforcing project-host sandbox prefix.
    pub prefix: Vec<String>,
    /// Caller environment, independent of the first service client.
    pub env: BTreeMap<String, String>,
    /// Selected Python interpreter identity.
    pub python: String,
    /// Replace `argv[0]` with the selected interpreter on the host.
    pub python_program: bool,
    /// Keep running without a controlling desktop.
    pub persistent: bool,
    /// Captured Slurm target, if any, for discovery and reattachment.
    pub target: Option<Value>,
    /// Notebook kernelspec, or debugger initialization payload.
    pub start: Start,
}
/// Initial controller configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Start {
    /// Start an installed Jupyter kernel.
    Notebook {
        /// Installed kernelspec.
        name: String,
    },
    /// Start one debugger coordinator.
    Debugger {
        /// Adapter/program configuration.
        launch: Box<debugger::Start>,
        /// Initial breakpoints and watches.
        settings: Box<debugger::Settings>,
    },
}
/// Discoverable runtime identity and acknowledged lifetime.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    /// Original saved source digests; unknown/changed sources remain visibly stale.
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
    /// Native runtime identity.
    pub id: String,
    /// Human-readable title.
    pub name: String,
    /// Computation type.
    pub kind: Kind,
    /// Original project root.
    pub root: String,
    /// Policy required for later attachments.
    pub policy: String,
    /// Original sandbox label.
    pub mode: String,
    /// Notebook path or debugged source path.
    pub document: String,
    /// Slurm target captured at creation.
    pub target: Option<Value>,
    /// Selected Python identity at creation.
    pub python: String,
    /// Kernel name or debugger backend label.
    pub configuration: String,
    /// Whether the runtime is still executing.
    pub alive: bool,
    /// Whether initialization or execution is underway.
    pub busy: bool,
    /// Last acknowledged status.
    pub status: String,
    /// Whether a controlling desktop owns the lease.
    pub attached: bool,
    /// Whether detach preserves execution.
    pub persistent: bool,
    /// Creation time in Unix seconds.
    pub created: u64,
}
/// High-level debugger actions; stopped epochs prevent stale handle use.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DebugAction {
    /// Continue, pause or step.
    Control {
        /// DAP control name.
        command: String,
    },
    /// Select one paused thread.
    Thread {
        /// Adapter thread ID.
        id: i64,
    },
    /// Select one paused frame.
    Frame {
        /// Adapter frame ID.
        id: i64,
    },
    /// Expand one paused variable.
    Variables {
        /// DAP reference.
        reference: i64,
    },
    /// Explicit expression evaluation.
    Evaluate {
        /// Expression in selected frame.
        expression: String,
    },
    /// Synchronize validated settings, breakpoints and watches.
    Configure {
        /// Current settings.
        settings: debugger::Settings,
        /// Sources requiring breakpoint refresh.
        paths: Vec<String>,
        /// Whether to refresh watches.
        watches: bool,
    },
    /// Coordinator terminal input/resize; never changes captured launch policy.
    Terminal {
        /// Validated terminal operation.
        command: debugger::Command,
    },
}
/// Commands accepted only from the current controlling lease.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Command {
    /// Update discovery after an explicit notebook Save As.
    Document {
        /// Current document path on the same project host.
        path: String,
    },
    /// Clear retained output while preserving kernel variables and execution.
    ClearOutputs {
        /// Stable cell ID, or all retained cells.
        cell: Option<String>,
    },
    /// Execute one atomic submitted queue. Controller continues while detached.
    Run {
        /// Cell snapshots.
        runs: Vec<notebook::Run>,
    },
    /// Drop queued cells whose stable ID/source changed.
    Retain {
        /// Current cell IDs and source versions.
        cells: Vec<(String, String)>,
    },
    /// Interrupt the active notebook execution.
    Interrupt,
    /// Control a current debugger stop.
    Debug {
        /// Epoch drawn in the controlling UI.
        epoch: u64,
        /// Requested operation.
        operation: Box<DebugAction>,
    },
    /// Update explicit last-view policy.
    Persistence {
        /// Desired persistence.
        persistent: bool,
    },
    /// Stop this computation and retain its final outputs for discovery.
    Stop,
}
/// Idempotent attachment mutation, acknowledged by the host after application.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    /// Random request token, never reused for another operation.
    pub id: String,
    /// Requested operation.
    pub command: Command,
}
/// One retained output and its Jupyter display identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Output {
    /// nbformat output.
    pub value: Value,
    /// Cross-cell display update identity.
    pub display: Option<String>,
}
/// Latest bounded execution result for one stable notebook cell.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cell {
    /// Source snapshot that owns the result.
    pub run: notebook::Run,
    /// Latest ordered outputs.
    pub outputs: Vec<Output>,
    /// Execution count when known.
    pub count: Option<u64>,
    /// Completion status, absent while running.
    pub done: Option<String>,
    /// Monotonic cell revision for replay/deduplication.
    pub revision: u64,
}
/// Small authoritative snapshot; output cells stream as separate bounded frames.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    /// Last applied mutation token, including a rejected operation.
    pub acknowledgement: String,
    /// Recent applied tokens so interleaved terminal writes cannot hide an acknowledgement.
    pub acknowledgements: Vec<String>,
    /// Discoverable identity and lifetime.
    pub info: Info,
    /// Present for a notebook, including currently executing/queued cells.
    pub runs: Vec<RunRef>,
    /// Jupyter language information from initialization.
    pub language: Value,
    /// Actual runtime interpreter.
    pub python: String,
    /// Actual hostname/allocation.
    pub execution: String,
    /// Authoritative debugger state, present for debugger runtimes.
    pub debugger: Option<debugger::protocol::View>,
    /// Last rejected command or runtime failure.
    pub error: String,
}
/// Small queue identity; source text is retained on the host and not repeated in snapshots.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRef {
    /// Creating view's cell identity.
    pub uid: u64,
    /// Stable nbformat cell identity.
    pub cell: String,
    /// Creating view's source version.
    pub version: u64,
    /// SHA-256 of the submitted source text.
    pub source: String,
}
impl From<&notebook::Run> for RunRef {
    fn from(run: &notebook::Run) -> Self {
        use sha2::{Digest, Sha256};
        Self {
            uid: run.uid,
            cell: run.cell.clone(),
            version: run.version,
            source: format!("{:x}", Sha256::digest(run.code.as_bytes())),
        }
    }
}
/// Streamed attachment update. Cell replay always includes its source identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Update {
    /// Current controller state.
    State(Box<State>),
    /// Latest result for one cell.
    Cell(Cell),
    /// Debug terminal history or incremental output.
    Terminal(debugger::Event),
    /// Attachment protocol failure.
    Error(String),
}

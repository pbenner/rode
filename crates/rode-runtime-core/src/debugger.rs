//! Project debugging through installed stdio Debug Adapter Protocol backends.
//! Adapters and debuggees share a single project-host sandbox coordinator.
//!
//! # Design decisions
//!
//! DAP keeps the native inspection UI independent of a particular language
//! debugger and lets users supply installed debugpy/LLDB/custom adapters. RODE
//! does not embed those debuggers or automatically install their toolchains;
//! adapter availability and capabilities therefore remain explicit.
//!
//! One coordinator owns the adapter and reverse `runInTerminal` targets inside
//! the same host/sandbox context. Starting them through unrelated sandbox calls
//! would give them separate private namespaces and resource lifetimes. Attach
//! stays normal-mode only because an already-running external process cannot
//! retroactively acquire this project's enforcing sandbox. Stopped epochs keep
//! late variable/watch replies from reviving inspection state after resume.
pub mod protocol;
mod shutdown;
const REQUEST_LIMIT: usize = 256;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Installed stdio DAP backend preset; custom adapters use the same outer policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Launch debugpy through the selected project interpreter.
    #[default]
    Python,
    /// Launch an installed LLDB DAP adapter for native code.
    Lldb,
    /// Launch an explicitly configured DAP executable.
    Custom,
}
impl Backend {
    /// Human-readable backend label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Python => "Python · debugpy",
            Self::Lldb => "Native · LLDB",
            Self::Custom => "Custom DAP",
        }
    }
}
/// Persisted debugger launch/attach configuration, with literal arguments and bounded JSON
/// extensions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Adapter preset or explicit custom backend.
    pub backend: Backend,
    /// Optional installed adapter executable override.
    pub adapter: String,
    /// Literal adapter-process arguments.
    pub adapter_arguments: Vec<String>,
    /// Debuggee program template, normally `{file}`.
    pub program: String,
    /// Literal debuggee arguments with supported placeholder expansion.
    pub arguments: Vec<String>,
    /// Debuggee working-directory template, normally `{root}`.
    pub cwd: String,
    /// Request a pause at the program's entry point.
    pub stop_on_entry: bool,
    /// Attach to an existing process rather than launching; only supported in normal mode.
    pub attach: bool,
    /// Additional bounded DAP launch/attach arguments as a JSON object.
    pub extra: Value,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            backend: Backend::Python,
            adapter: String::new(),
            adapter_arguments: vec![],
            program: "{file}".into(),
            arguments: vec![],
            cwd: "{root}".into(),
            stop_on_entry: false,
            attach: false,
            extra: json!({}),
        }
    }
}
impl Config {
    /// Validate launch settings without running an adapter or debuggee.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.extra.is_object(),
            "Advanced debugger arguments must be a JSON object."
        );
        ensure!(
            self.arguments.len() <= 128 && self.adapter_arguments.len() <= 128,
            "Too many debugger arguments."
        );
        for s in [&self.adapter, &self.program, &self.cwd]
            .into_iter()
            .chain(self.arguments.iter())
            .chain(self.adapter_arguments.iter())
        {
            ensure!(
                s.len() <= 4096 && !s.contains('\0'),
                "Invalid debugger path or argument."
            );
        }
        ensure!(
            self.backend != Backend::Custom || !self.adapter.trim().is_empty(),
            "Choose an installed DAP adapter executable."
        );
        Ok(())
    }
}
/// One persisted source breakpoint/logpoint; line numbers are one-based.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Breakpoint {
    /// One-based source line.
    pub line: usize,
    /// Whether to send this breakpoint to the adapter.
    pub enabled: bool,
    /// Optional adapter-evaluated condition expression.
    pub condition: String,
    /// Optional adapter-specific hit-count condition.
    pub hit_condition: String,
    /// Optional logpoint message in place of stopping.
    pub log_message: String,
}
impl Default for Breakpoint {
    fn default() -> Self {
        Self {
            line: 1,
            enabled: true,
            condition: String::new(),
            hit_condition: String::new(),
            log_message: String::new(),
        }
    }
}
/// Host/project debugger preferences; excludes runtime variables and transcripts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Current launch/attach configuration.
    pub config: Config,
    /// Project-host source paths mapped to their breakpoints.
    pub breakpoints: BTreeMap<String, Vec<Breakpoint>>,
    /// Expressions evaluated when paused in the selected frame.
    pub watches: Vec<String>,
    /// Requested adapter exception-filter identifiers.
    pub exceptions: Vec<String>,
}
impl Settings {
    /// Validate configuration, breakpoints, watches and total persisted-size limits.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            serde_json::to_vec(self)?.len() <= 256 * 1024,
            "Debugger settings exceed 256 KiB."
        );
        ensure!(
            self.breakpoints.len() <= 128
                && self.breakpoints.values().map(Vec::len).sum::<usize>() <= 2048,
            "At most 2,048 breakpoints in 128 sources."
        );
        ensure!(
            self.watches.len() <= 32
                && self
                    .watches
                    .iter()
                    .all(|s| !s.trim().is_empty() && s.len() <= 4096),
            "At most 32 watch expressions, each up to 4 KiB."
        );
        ensure!(self.exceptions.len() <= 32, "Too many exception filters.");
        for (path, items) in &self.breakpoints {
            ensure!(
                path.len() <= 4096 && !path.contains('\0') && items.len() <= 256,
                "Invalid breakpoint source or too many breakpoints."
            );
            for bp in items {
                ensure!(
                    (1..=10_000_000).contains(&bp.line)
                        && [&bp.condition, &bp.hit_condition, &bp.log_message]
                            .into_iter()
                            .all(|s| s.len() <= 1024 && !s.contains('\0')),
                    "Invalid breakpoint location/condition."
                );
            }
        }
        self.config.validate()?;
        Ok(())
    }
}
/// Initial coordinator launch payload captured from the prepared workspace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Start {
    /// Project-host working directory.
    pub root: String,
    /// Selected source file used by `{file}` expansion.
    pub file: String,
    /// Selected project-host Python interpreter for the Python preset.
    pub python: String,
    /// Validated launch/attach configuration.
    pub config: Config,
}
/// Editor-to-coordinator JSON-line protocol; payloads stay within the project-host sandbox.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// Launch the adapter and initialize its debuggee context.
    Start {
        /// Captured initial launch configuration.
        launch: Start,
    },
    /// Forward one DAP request/response to the adapter.
    Dap {
        /// Complete DAP JSON message.
        message: Value,
    },
    /// Write raw bytes to a coordinator-owned debug terminal.
    Input {
        /// Coordinator-local terminal identifier.
        id: u64,
        /// Standard base64-encoded terminal input bytes.
        data: String,
    },
    /// Resize a debug terminal's PTY.
    Resize {
        /// Coordinator-local terminal identifier.
        id: u64,
        /// PTY height in character rows.
        rows: u16,
        /// PTY width in character columns.
        columns: u16,
    },
    /// End one owned debug terminal.
    EndTerminal {
        /// Coordinator-local terminal identifier.
        id: u64,
    },
    /// Stop the coordinator and clean up its owned process descendants.
    Shutdown,
}
/// Coordinator-to-editor JSON-line notifications, including native terminal output.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Forward one adapter DAP message.
    Dap {
        /// Complete DAP JSON message.
        message: Value,
    },
    /// Announce an interactive debug terminal.
    Terminal {
        /// Coordinator-local terminal identifier.
        id: u64,
        /// Display title for the new terminal.
        title: String,
    },
    /// Supply output bytes from a debug PTY.
    Data {
        /// Coordinator-local terminal identifier.
        id: u64,
        /// Standard base64-encoded terminal output bytes.
        data: String,
    },
    /// Mark an owned debug terminal exited while preserving its displayed output.
    TerminalExited {
        /// Coordinator-local terminal identifier.
        id: u64,
    },
    /// Report a coordinator/adapter failure for display.
    Error {
        /// Bounded failure explanation.
        message: String,
    },
}
/// Expand supported `{name}` placeholders once, leaving unknown placeholders literal.
pub fn replace(value: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('}') else {
            break;
        };
        match values.iter().find(|(key, _)| *key == &rest[1..end]) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[..=end]),
        };
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}
/// Recursively expand string values in bounded JSON arrays/objects; object keys stay unchanged.
pub fn expand(value: &Value, values: &[(&str, &str)], depth: usize) -> Result<Value> {
    ensure!(depth <= 24, "Debugger argument nesting exceeds its limit.");
    Ok(match value {
        Value::String(s) => Value::String(replace(s, values)),
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|v| expand(v, values, depth + 1))
                .collect::<Result<_>>()?,
        ),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| Ok((k.clone(), expand(v, values, depth + 1)?)))
                .collect::<Result<_>>()?,
        ),
        v => v.clone(),
    })
}

//! One-shot project-tool contracts, independent of the host implementation.
//! The host supplies execution policy and validates buffer ownership on application.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
/// Project-tool wire protocol version.
pub const API_VERSION: u32 = 1;
/// Maximum UTF-8 source/replacement size.
pub const TEXT_LIMIT: usize = 8 * 1024 * 1024;
/// Normalized source diagnostic. Rows and character columns are one-based; end positions are
/// tool-derived and may equal the start for point diagnostics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Tool/rule identifier.
    pub code: String,
    /// Human-readable issue description.
    pub message: String,
    /// One-based start line.
    pub row: usize,
    /// One-based start character column after tool-specific normalization.
    pub column: usize,
    /// One-based end line.
    pub end_row: usize,
    /// One-based end character column.
    pub end_column: usize,
    /// Severity label, such as `error` or `warning`.
    pub severity: String,
}
/// This is a one-shot request over existing project transport, not a plugin daemon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invocation {
    /// Required adapter protocol version.
    pub api: u32,
    /// Owning extension identifier.
    pub extension: String,
    /// Project-host executable captured from its validated manifest.
    pub program: String,
    /// Literal executable arguments.
    pub args: Vec<String>,
    /// Requested format/check/command operation.
    pub operation: String,
    /// Optional project-host source path used only as tool context.
    pub path: Option<String>,
    /// Unsaved source snapshot for buffer operations.
    pub text: String,
    /// Namespaced extension settings captured with this invocation.
    pub settings: BTreeMap<String, serde_json::Value>,
}
/// Bounded API v1 tool result, validated before UI application.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    /// Reply protocol version, which must match the supported API.
    pub api: u32,
    /// Optional formatted replacement snapshot; the UI must check the original buffer version.
    #[serde(default)]
    pub text: Option<String>,
    /// Bounded normalized diagnostics for the submitted source.
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    /// Bounded human-readable command output.
    #[serde(default)]
    pub output: String,
}
impl Reply {
    /// Validate reply shape, size and operation-specific fields before accepting adapter
    /// output.
    pub fn validate(&self, operation: &str) -> Result<()> {
        ensure!(self.api == API_VERSION, "Unsupported adapter response API.");
        ensure!(
            self.text.as_ref().is_none_or(|s| s.len() <= TEXT_LIMIT)
                && self.output.len() <= 1024 * 1024
                && self.diagnostics.len() <= 4096,
            "Adapter response exceeds limits."
        );
        ensure!(
            operation == "format" || self.text.is_none(),
            "Only format may return replacement text."
        );
        ensure!(
            self.diagnostics.iter().all(|d| d.row > 0
                && d.column > 0
                && d.end_row >= d.row
                && d.end_column > 0
                && [d.row, d.column, d.end_row, d.end_column]
                    .iter()
                    .all(|&n| n <= TEXT_LIMIT + 1)
                && (d.end_row != d.row || d.end_column >= d.column)
                && d.message.len() <= 8192
                && d.code.len() <= 128
                && ["error", "warning", "info"].contains(&d.severity.as_str())),
            "Invalid adapter diagnostic (positions are 1-based Unicode scalar columns)."
        );
        Ok(())
    }
}

/// Payload delivered to an adapter's stdin. Executable and policy selection stay
/// in the host; providers receive context and return edits instead of writing files.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub api: u32,
    pub extension: String,
    pub operation: String,
    pub root: String,
    pub path: Option<String>,
    pub text: String,
    pub settings: BTreeMap<String, serde_json::Value>,
}

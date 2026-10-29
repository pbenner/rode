//! Native nbformat 4 model. Unknown metadata, MIME bundles and cell fields survive edits.
//!
//! # Design decisions
//!
//! Retain raw JSON alongside the native cell model so editing supported fields
//! does not discard metadata or output types produced by other notebook tools.
//! This costs some duplicate in-memory state, bounded by document/output limits,
//! and requires serialization to take cells from their current edited models.
//! Stable cell IDs and source versions let outputs follow moved cells while
//! rejecting results for source that changed during execution. A separately
//! started kernel keeps execution dependencies/lifetimes out of ordinary file
//! opening and saving; unknown rich output can be preserved without executing it.
pub mod kernel;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

/// Maximum serialized notebook size in bytes.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Maximum retained cells in one notebook.
pub const MAX_CELLS: usize = 1024;
/// Maximum source size per cell in bytes.
pub const MAX_SOURCE: usize = 256 * 1024;
/// Maximum code bytes in one execution request.
pub const MAX_CODE: usize = 128 * 1024;
/// Maximum retained outputs per code cell.
pub const MAX_OUTPUTS: usize = 256;
/// Maximum serialized outputs per cell in bytes.
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Editable cell preserving unknown nbformat fields alongside runtime-only identity/version
/// data.
#[derive(Clone, Debug)]
pub struct Cell {
    /// Stable in-memory identity, independent of cell position and never serialized.
    pub uid: u64,
    /// Original cell JSON updated through the cell's editing methods.
    pub data: Value,
    /// Joined source text used by the native editor.
    pub source: String,
    /// Source revision used to reject obsolete execution results.
    pub version: u64,
    /// Output revision used to invalidate rendered output caches.
    pub outputs_version: u64,
}
impl Cell {
    /// Create an empty code, Markdown or raw cell with a fresh runtime identity.
    pub fn new(kind: &str) -> Self {
        let uid = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut data = json!({"id":format!("rode-{}-{uid}",std::process::id()),"cell_type":kind,"metadata":{},"source":[]});
        if kind == "code" {
            data["outputs"] = json!([]);
            data["execution_count"] = Value::Null;
        }
        Self {
            uid,
            data,
            source: String::new(),
            version: 0,
            outputs_version: 0,
        }
    }
    /// Read the nbformat cell type without discarding unknown fields.
    pub fn kind(&self) -> &str {
        self.data["cell_type"].as_str().unwrap_or("raw")
    }
    /// Borrow saved/executed outputs, or an empty slice for non-code cells.
    pub fn outputs(&self) -> &[Value] {
        self.data["outputs"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    /// Update source JSON and advance its version when source changes.
    pub fn set_source(&mut self, source: String) {
        self.source = source;
        self.data["source"] = json!(self.source);
        self.version = self.version.wrapping_add(1);
    }
    /// Switch the cell type, maintaining the code-only execution/output fields.
    pub fn change_kind(&mut self, kind: &str) {
        self.data["cell_type"] = json!(kind);
        self.data.as_object_mut().unwrap().remove("outputs");
        self.data.as_object_mut().unwrap().remove("execution_count");
        if kind == "code" {
            self.data["outputs"] = json!([]);
            self.data["execution_count"] = Value::Null;
        }
        self.version = self.version.wrapping_add(1);
        self.outputs_version = self.outputs_version.wrapping_add(1);
    }
    /// Remove code outputs/execution count and advance output-cache state.
    pub fn clear_outputs(&mut self) {
        if self.kind() == "code" {
            self.data["outputs"] = json!([]);
            self.data["execution_count"] = Value::Null;
            self.outputs_version = self.outputs_version.wrapping_add(1);
        }
    }
}
/// Native nbformat 4 document preserving unknown top-level fields and cell metadata.
#[derive(Clone, Debug)]
pub struct Notebook {
    /// Top-level JSON retained across editing; serialization replaces its cells from
    /// [`Self::cells`].
    pub data: Value,
    /// Ordered native cell models with stable runtime identities.
    pub cells: Vec<Cell>,
}
impl Default for Notebook {
    fn default() -> Self {
        Self {
            data: json!({"nbformat":4,"nbformat_minor":5,"metadata":{"kernelspec":{"name":"python3","display_name":"Python 3","language":"python"},"language_info":{"name":"python"}}}),
            cells: vec![Cell::new("code")],
        }
    }
}
/// Recognize notebook paths by the `.ipynb` extension.
pub fn is_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("ipynb"))
}
/// Decode nbformat's string-or-string-array text representation, rejecting other shapes.
pub fn multiline(value: &Value) -> Result<String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Array(lines) => {
            let mut result = String::new();
            for line in lines {
                result.push_str(line.as_str().context("Notebook lines must be strings")?);
            }
            Ok(result)
        }
        _ => anyhow::bail!("Notebook text must be a string or list of strings"),
    }
}
impl Notebook {
    /// Parse bounded nbformat 4 JSON, validating cell/source/output limits without launching a
    /// kernel.
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(text.len() <= MAX_BYTES, "Notebook exceeds 8 MiB.");
        let mut data: Value = serde_json::from_str(text).context("Invalid notebook JSON")?;
        ensure!(
            data["nbformat"].as_u64() == Some(4),
            "Only nbformat 4 notebooks are supported; open the JSON to convert other versions."
        );
        ensure!(
            data["metadata"].is_object(),
            "Notebook metadata must be an object."
        );
        let raw = data
            .as_object_mut()
            .context("Notebook must be an object")?
            .remove("cells")
            .context("Notebook cells are missing")?;
        let raw = raw.as_array().context("Notebook cells must be an array")?;
        ensure!(raw.len() <= MAX_CELLS, "Notebook exceeds 1024 cells.");
        let mut cells = Vec::new();
        for value in raw {
            ensure!(
                value.is_object()
                    && value["cell_type"].is_string()
                    && value["metadata"].is_object(),
                "Invalid notebook cell."
            );
            let source = multiline(&value["source"])?;
            ensure!(
                source.len() <= MAX_SOURCE,
                "A cell exceeds 256 KiB; use JSON source view to edit it."
            );
            if value["cell_type"] == "code" {
                ensure!(
                    value["outputs"].is_array(),
                    "Code cell outputs must be an array."
                );
            }
            cells.push(Cell {
                uid: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                data: value.clone(),
                source,
                version: 0,
                outputs_version: 0,
            });
        }
        Ok(Self { data, cells })
    }
    /// Serialize current cell JSON while preserving unknown fields; reject results above the
    /// document limit.
    pub fn serialize(&self) -> Result<String> {
        let mut data = self.data.clone();
        data["cells"] = Value::Array(self.cells.iter().map(|c| c.data.clone()).collect());
        let mut text = serde_json::to_string_pretty(&data)?;
        text.push('\n');
        ensure!(
            text.len() <= MAX_BYTES,
            "Notebook exceeds 8 MiB. The change was not applied; clear outputs or reduce the notebook first."
        );
        Ok(text)
    }
    /// Return the saved kernelspec name, defaulting to `python3` when absent.
    pub fn kernel(&self) -> String {
        self.data["metadata"]["kernelspec"]["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("python3")
            .into()
    }
    /// Return the declared notebook language, defaulting to Python when absent.
    pub fn language(&self) -> &str {
        self.data["metadata"]["language_info"]["name"]
            .as_str()
            .or_else(|| self.data["metadata"]["kernelspec"]["language"].as_str())
            .unwrap_or("python")
    }
    /// Validate and update kernelspec metadata; does not install or start a kernel.
    pub fn set_kernel(&mut self, name: &str) -> Result<()> {
        ensure!(
            !name.is_empty()
                && name.len() <= 128
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
            "Invalid kernelspec name."
        );
        if !self.data["metadata"]["kernelspec"].is_object() {
            self.data["metadata"]["kernelspec"] = json!({});
        }
        self.data["metadata"]["kernelspec"]["name"] = json!(name);
        self.data["metadata"]["kernelspec"]["display_name"] = json!(name);
        Ok(())
    }
    /// Apply output/count/clear only to the matching cell identity and source version. Returns
    /// `false` for stale/deleted cells and rolls back changes that exceed output/document
    /// limits.
    pub fn apply_output(
        &mut self,
        uid: u64,
        version: u64,
        output: Option<Value>,
        count: Option<u64>,
        clear: bool,
    ) -> Result<bool> {
        let Some(index) = self
            .cells
            .iter()
            .position(|c| c.uid == uid && c.version == version && c.kind() == "code")
        else {
            return Ok(false);
        };
        let cell = &mut self.cells[index];
        let old = cell.data.clone();
        if clear {
            cell.data["outputs"] = json!([]);
        }
        if let Some(count) = count {
            cell.data["execution_count"] = json!(count);
        }
        if let Some(output) = output {
            let outputs = cell.data["outputs"]
                .as_array_mut()
                .context("Cell outputs are invalid")?;
            if outputs.len() >= MAX_OUTPUTS {
                cell.data = old;
                anyhow::bail!("Cell output limit reached.");
            }
            outputs.push(output);
        }
        if cell.data["outputs"].to_string().len() > MAX_OUTPUT_BYTES {
            cell.data = old;
            anyhow::bail!("Cell output exceeds 1 MiB; interrupt or clear outputs.");
        }
        if let Err(error) = self.serialize() {
            self.cells[index].data = old;
            return Err(error);
        }
        self.cells[index].outputs_version = self.cells[index].outputs_version.wrapping_add(1);
        Ok(true)
    }
}

//! Native nbformat 4 model. Unknown metadata, MIME bundles and cell fields survive edits.
//!
//! # Design decisions
//!
//! Retain unknown notebook/cell metadata while storing MIME payloads separately
//! behind shared references. Structured serialization borrows those values, so
//! snapshots and ordinary saves do not deep-copy the retained output tree.
//! Stable cell IDs and source versions let outputs follow moved cells while
//! rejecting results for source that changed during execution. A separately
//! started kernel keeps execution dependencies/lifetimes out of ordinary file
//! opening and saving; unknown rich output can be preserved without executing it.
pub mod kernel;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Maximum retained cells in one notebook.
pub const MAX_CELLS: usize = 1024;
/// Maximum source size per cell in bytes.
pub const MAX_SOURCE: usize = 256 * 1024;
use rode_runtime_core::notebook::outputs;
pub use rode_runtime_core::notebook::{MAX_CODE, outputs::MemoryBudget};
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Editable cell preserving unknown nbformat fields alongside runtime-only identity/version
/// data.
#[derive(Clone, Debug)]
pub struct Cell {
    /// Stable in-memory identity, independent of cell position and never serialized.
    pub uid: u64,
    /// Cell fields updated through editing methods; code outputs live in shared storage.
    pub data: Value,
    /// Joined source text used by the native editor.
    pub source: String,
    /// Source revision used to reject obsolete execution results.
    pub version: u64,
    /// Output revision used to invalidate rendered output caches.
    pub outputs_version: u64,
    outputs: Vec<Arc<Value>>,
    output_sizes: Vec<usize>,
    output_bytes: usize,
}
impl Cell {
    /// Create an empty code, Markdown or raw cell with a fresh runtime identity.
    pub fn new(kind: &str) -> Self {
        let uid = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut data = json!({"id":format!("rode-{}-{uid}",std::process::id()),"cell_type":kind,"metadata":{},"source":[]});
        if kind == "code" {
            data["execution_count"] = Value::Null;
        }
        Self {
            uid,
            data,
            source: String::new(),
            version: 0,
            outputs_version: 0,
            outputs: vec![],
            output_sizes: vec![],
            output_bytes: 0,
        }
    }
    /// Read the nbformat cell type without discarding unknown fields.
    pub fn kind(&self) -> &str {
        self.data["cell_type"].as_str().unwrap_or("raw")
    }
    /// Borrow saved/executed outputs, or an empty slice for non-code cells.
    pub fn outputs(&self) -> &[Arc<Value>] {
        &self.outputs
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
        self.outputs.clear();
        self.output_sizes.clear();
        self.output_bytes = 0;
        self.data.as_object_mut().unwrap().remove("execution_count");
        if kind == "code" {
            self.data["execution_count"] = Value::Null;
        }
        self.version = self.version.wrapping_add(1);
        self.outputs_version = self.outputs_version.wrapping_add(1);
    }
    /// Remove code outputs/execution count and advance output-cache state.
    pub fn clear_outputs(&mut self) {
        if self.kind() == "code" {
            self.outputs.clear();
            self.output_sizes.clear();
            self.output_bytes = 0;
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
    output_budget: MemoryBudget,
}
impl Default for Notebook {
    fn default() -> Self {
        Self {
            data: json!({"nbformat":4,"nbformat_minor":5,"metadata":{"kernelspec":{"name":"python3","display_name":"Python 3","language":"python"},"language_info":{"name":"python"}}}),
            cells: vec![Cell::new("code")],
            output_budget: Default::default(),
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
    /// Parse in-memory nbformat 4 JSON, validating cell/source structure without launching a
    /// kernel.
    pub fn parse(text: &str) -> Result<Self> {
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
        let Value::Array(raw) = raw else {
            anyhow::bail!("Notebook cells must be an array");
        };
        ensure!(raw.len() <= MAX_CELLS, "Notebook exceeds 1024 cells.");
        let mut cells = Vec::new();
        for mut value in raw {
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
            let retained = if value["cell_type"] == "code" {
                value
                    .as_object_mut()
                    .unwrap()
                    .remove("outputs")
                    .unwrap()
                    .as_array_mut()
                    .map(std::mem::take)
                    .unwrap()
            } else {
                vec![]
            };
            let output_sizes = retained
                .iter()
                .map(outputs::retained_bytes)
                .collect::<Result<Vec<_>>>()?;
            let output_bytes = output_sizes.iter().sum();
            cells.push(Cell {
                uid: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                data: value,
                source,
                version: 0,
                outputs_version: 0,
                outputs: retained.into_iter().map(Arc::new).collect(),
                output_sizes,
                output_bytes,
            });
        }
        Ok(Self {
            data,
            cells,
            output_budget: Default::default(),
        })
    }
    /// Serialize current cell JSON while preserving unknown fields, without a file-byte ceiling.
    pub fn serialize(&self) -> Result<String> {
        let mut text = serde_json::to_string_pretty(&NotebookFile(self))?;
        text.push('\n');
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
    /// Set the RAM allowance used by new live output. Loaded files are preserved.
    pub fn set_output_budget(&mut self, budget: MemoryBudget) {
        self.output_budget = budget;
    }

    /// Account all cells without walking their shared MIME payloads again.
    fn retained_output_bytes(&self) -> usize {
        self.cells.iter().map(|cell| cell.output_bytes).sum()
    }

    /// Apply an ordered delta atomically. Validation measures only changed payloads;
    /// successful edits move shared MIME references and append stream text in place.
    pub fn apply_edits(
        &mut self,
        uid: u64,
        version: u64,
        reset: bool,
        edits: &[rode_runtime_core::service::OutputEdit],
        count: Option<u64>,
    ) -> Result<bool> {
        use rode_runtime_core::service::OutputEdit;
        let Some(at) = self
            .cells
            .iter()
            .position(|cell| cell.uid == uid && cell.version == version && cell.kind() == "code")
        else {
            return Ok(false);
        };
        let cell = &self.cells[at];
        let mut bytes = if reset { 0 } else { cell.output_bytes };
        let mut appended_sizes = Vec::new();
        let mut changed_sizes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for edit in edits {
            match edit {
                OutputEdit::Append { output } => {
                    let size = outputs::retained_bytes(&output.value)?;
                    bytes = bytes.saturating_add(size);
                    appended_sizes.push(size);
                }
                OutputEdit::Replace { index, output } => {
                    ensure!(
                        !reset && *index < cell.outputs.len() && seen.insert(*index),
                        "Invalid notebook output replacement"
                    );
                    let size = outputs::retained_bytes(&output.value)?;
                    bytes = bytes
                        .saturating_sub(cell.output_sizes[*index])
                        .saturating_add(size);
                    changed_sizes.push((*index, size));
                }
                OutputEdit::Text { index, text } => {
                    ensure!(
                        !reset && *index < cell.outputs.len() && seen.insert(*index),
                        "Invalid notebook stream position"
                    );
                    ensure!(
                        cell.outputs[*index]["output_type"] == "stream"
                            && cell.outputs[*index]["text"].is_string(),
                        "Notebook output is not a stream"
                    );
                    let extra = rode_runtime_core::json_size(text)?.saturating_sub(2);
                    bytes = bytes.saturating_add(extra);
                    changed_sizes.push((*index, cell.output_sizes[*index].saturating_add(extra)));
                }
            }
        }
        let old_total = self.retained_output_bytes();
        let total = old_total
            .saturating_sub(cell.output_bytes)
            .saturating_add(bytes);
        if total > old_total {
            self.output_budget.check(total)?;
        }
        let cell = &mut self.cells[at];
        if reset {
            cell.outputs.clear();
            cell.output_sizes.clear();
        }
        for edit in edits {
            match edit {
                OutputEdit::Append { output } => cell.outputs.push(output.value.clone()),
                OutputEdit::Replace { index, output } => {
                    cell.outputs[*index] = output.value.clone()
                }
                OutputEdit::Text { index, text } => {
                    if let Value::String(previous) =
                        &mut Arc::make_mut(&mut cell.outputs[*index])["text"]
                    {
                        previous.reserve_exact(text.len());
                        previous.push_str(text);
                    }
                }
            }
        }
        if let Some(count) = count {
            cell.data["execution_count"] = json!(count);
        }
        for (index, size) in changed_sizes {
            cell.output_sizes[index] = size;
        }
        cell.output_sizes.extend(appended_sizes);
        cell.output_bytes = bytes;
        cell.outputs_version = cell.outputs_version.wrapping_add(1);
        Ok(true)
    }

    /// Validate an entire cross-cell display update before replacing any references.
    pub fn update_display_outputs(
        &mut self,
        targets: &[(u64, u64, usize)],
        output: &Value,
    ) -> Result<Vec<u64>> {
        let size = outputs::retained_bytes(output)?;
        let mut locations = std::collections::BTreeSet::new();
        let mut total = self.retained_output_bytes();
        for &(uid, version, index) in targets {
            if let Some(at) = self
                .cells
                .iter()
                .position(|cell| cell.uid == uid && cell.version == version)
                && index < self.cells[at].outputs.len()
                && locations.insert((at, index))
            {
                total = total
                    .saturating_sub(self.cells[at].output_sizes[index])
                    .saturating_add(size);
            }
        }
        if total > self.retained_output_bytes() {
            self.output_budget.check(total)?;
        }
        let shared = Arc::new(output.clone());
        let mut changed = std::collections::BTreeSet::new();
        for (at, index) in locations {
            let cell = &mut self.cells[at];
            cell.output_bytes = cell
                .output_bytes
                .saturating_sub(cell.output_sizes[index])
                .saturating_add(size);
            cell.output_sizes[index] = size;
            cell.outputs[index] = shared.clone();
            cell.outputs_version = cell.outputs_version.wrapping_add(1);
            changed.insert(cell.uid);
        }
        Ok(changed.into_iter().collect())
    }

    /// Apply foreground output while preserving source identity and normal stream coalescing.
    pub fn apply_output(
        &mut self,
        uid: u64,
        version: u64,
        output: Option<Value>,
        count: Option<u64>,
        clear: bool,
    ) -> Result<bool> {
        use rode_runtime_core::service::{Output, OutputEdit};
        let mut edits = vec![];
        if let Some(output) = output {
            let stream = (!clear)
                .then(|| {
                    self.cells
                        .iter()
                        .find(|cell| cell.uid == uid && cell.version == version)
                        .and_then(|cell| {
                            cell.outputs.last().and_then(|old| {
                                outputs::stream_fragment(old, &output)
                                    .map(|text| (cell.outputs.len() - 1, text.to_owned()))
                            })
                        })
                })
                .flatten();
            if let Some((index, text)) = stream {
                edits.push(OutputEdit::Text { index, text });
            } else {
                edits.push(OutputEdit::Append {
                    output: Output::new(output, None)?,
                });
            }
        }
        self.apply_edits(uid, version, clear, &edits, count)
    }
}

/// Borrow structured cells during serialization instead of cloning all MIME payloads.
struct NotebookFile<'a>(&'a Notebook);
impl serde::Serialize for NotebookFile<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in self.0.data.as_object().unwrap() {
            if key != "cells" {
                map.serialize_entry(key, value)?;
            }
        }
        struct Cells<'a>(&'a [Cell]);
        impl serde::Serialize for Cells<'_> {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                use serde::ser::SerializeSeq;
                let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
                for cell in self.0 {
                    seq.serialize_element(&CellFile(cell))?;
                }
                seq.end()
            }
        }
        map.serialize_entry("cells", &Cells(&self.0.cells))?;
        map.end()
    }
}
struct CellFile<'a>(&'a Cell);
impl serde::Serialize for CellFile<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in self.0.data.as_object().unwrap() {
            if key != "outputs" || self.0.kind() != "code" {
                map.serialize_entry(key, value)?;
            }
        }
        if self.0.kind() == "code" {
            map.serialize_entry("outputs", &self.0.outputs)?;
        }
        map.end()
    }
}

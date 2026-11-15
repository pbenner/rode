//! Retained nbformat output policy shared by host publication and desktop edits.
//! Check complete candidate arrays before applying a display update's fan-out.
use anyhow::{Result, ensure};
use serde_json::Value;

/// Maximum retained outputs per code cell.
pub const MAX_OUTPUTS: usize = 256;
/// Maximum compact JSON bytes in one cell's output array, including delimiters.
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Validate a proposed cell output array without cloning values or serializing
/// it into another buffer. Callers can substitute display replacements in the
/// iterator and validate every affected cell before publishing any changes.
pub fn validate<'a>(outputs: impl IntoIterator<Item = &'a Value>) -> Result<()> {
    let mut size = 2usize; // Array delimiters.
    for (index, output) in outputs.into_iter().enumerate() {
        ensure!(index < MAX_OUTPUTS, "Cell output limit reached.");
        let bytes = crate::json_size(output)?;
        ensure!(
            bytes <= MAX_OUTPUT_BYTES.saturating_sub(size + usize::from(index > 0)),
            "Cell output exceeds 1 MiB; interrupt or clear outputs."
        );
        size += bytes + usize::from(index > 0);
    }
    Ok(())
}

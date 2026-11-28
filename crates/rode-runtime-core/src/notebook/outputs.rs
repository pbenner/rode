//! Retained nbformat output policy shared by host publication and desktop edits.
//! Check complete candidate arrays before applying a display update's fan-out.
use anyhow::{Result, ensure};
use serde_json::Value;

/// Maximum retained outputs per code cell.
pub const MAX_OUTPUTS: usize = 256;
/// Maximum compact JSON bytes in one cell's output array, including delimiters.
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// Maximum non-image JSON bytes retained by a cell, including text MIME alternatives.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
/// Maximum encoded host-retained notebook state, shared by all its cells.
pub const MAX_NOTEBOOK_BYTES: usize = 64 * 1024 * 1024;
/// Characters per JSON fragment; even four-byte UTF-8 stays below the process line limit.
pub(super) const CHUNK_CHARS: usize = 64 * 1024;
/// A bounded output event includes its cell payload plus small protocol/display metadata.
pub(super) const MAX_MESSAGE_BYTES: usize = MAX_OUTPUT_BYTES + 16 * 1024;

/// Send one policy to the Python bridge instead of maintaining independent Python limits.
pub(super) fn configuration() -> Value {
    serde_json::json!({
        "bytes": MAX_OUTPUT_BYTES, "text": MAX_TEXT_BYTES, "count": MAX_OUTPUTS,
        "chunk_chars": CHUNK_CHARS, "message": MAX_NOTEBOOK_BYTES,
    })
}

/// Count text and metadata independently of PNG/JPEG payloads. All MIME alternatives
/// remain in the total budget, while only supported raster payloads get the larger allowance.
fn sizes(output: &Value) -> Result<(usize, usize)> {
    let bytes = crate::json_size(output)?;
    let mut text = bytes;
    for mime in ["image/png", "image/jpeg"] {
        if let Some(image) = output.get("data").and_then(|data| data.get(mime)) {
            text = text.saturating_sub(crate::json_size(image)?);
        }
    }
    Ok((bytes, text))
}

/// Validate a proposed cell output array without cloning values or serializing
/// it into another buffer. Callers can substitute display replacements in the
/// iterator and validate every affected cell before publishing any changes.
pub fn validate<'a>(outputs: impl IntoIterator<Item = &'a Value>) -> Result<()> {
    let mut size = 2usize; // Array delimiters.
    let mut text_size = 2usize;
    for (index, output) in outputs.into_iter().enumerate() {
        ensure!(index < MAX_OUTPUTS, "Cell output limit reached.");
        let (bytes, text) = sizes(output)?;
        ensure!(
            bytes <= MAX_OUTPUT_BYTES.saturating_sub(size + usize::from(index > 0)),
            "Cell output exceeds 8 MiB; earlier output is preserved. Clear outputs to make room."
        );
        ensure!(
            text <= MAX_TEXT_BYTES.saturating_sub(text_size + usize::from(index > 0)),
            "Cell text output exceeds 1 MiB; earlier output is preserved. Clear outputs to make room."
        );
        size += bytes + usize::from(index > 0);
        text_size += text + usize::from(index > 0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn raster_payloads_have_room_without_loosening_text_or_total_limits() {
        let image = json!({"output_type":"display_data", "data":{"image/png":"a".repeat(3*1024*1024), "text/plain":"plot"}});
        validate([&image]).unwrap();
        for output in [
            json!({"output_type":"stream", "text":"x".repeat(MAX_TEXT_BYTES)}),
            json!({"data":{"image/png":"small", "text/html":"x".repeat(MAX_TEXT_BYTES)}}),
            json!({"data":{"image/png":"x".repeat(MAX_OUTPUT_BYTES)}}),
        ] {
            assert!(validate([&output]).is_err());
        }
        assert!(validate(std::iter::repeat_n(&image, 3)).is_err());
        let small = json!({"output_type":"stream", "text":"ok"});
        validate(std::iter::repeat_n(&small, MAX_OUTPUTS)).unwrap();
        assert!(validate(std::iter::repeat_n(&small, MAX_OUTPUTS + 1)).is_err());
    }
}

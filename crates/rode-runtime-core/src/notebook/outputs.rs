//! RAM-only notebook output policy. Retention is shared across a notebook's cells;
//! wire chunks and rendering have separate, small working buffers.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Largest configurable retained-output budget; also bounds one incoming payload.
pub const MAX_RETAINED_BYTES: usize = 512 * 1024 * 1024;
/// Account for output records and their index in addition to their encoded MIME payloads.
pub const RECORD_OVERHEAD: usize = 128;
/// Bytes per runtime attachment fragment, independent of total output size.
pub const CHUNK_BYTES: usize = 64 * 1024;
pub(super) const CHUNK_CHARS: usize = CHUNK_BYTES;
// A full replay also carries source and envelope metadata. JSON may escape
// each source byte as six bytes; output capacity must not consume that space.
pub(crate) const MAX_MESSAGE_BYTES: usize = MAX_RETAINED_BYTES + 6 * super::MAX_CODE + 256 * 1024;

/// One notebook's live output allowance, in MiB. No separate text/image/count cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "usize", into = "usize")]
pub struct MemoryBudget(usize);
impl MemoryBudget {
    /// Configurable RAM allowances, shared by all cells in a kernel.
    pub const PRESETS: [Self; 4] = [Self(64), Self(128), Self(256), Self(512)];
    /// Allowance in MiB for display and project preference storage.
    pub fn mib(self) -> usize {
        self.0
    }
    /// Allowance in accounted output bytes.
    pub fn bytes(self) -> usize {
        self.0 * 1024 * 1024
    }
    /// Check an atomic candidate without evicting accepted output.
    pub fn check(self, bytes: usize) -> Result<()> {
        ensure!(
            bytes <= self.bytes(),
            "Notebook output RAM budget reached ({} MiB). Earlier output is preserved and computation continues. Clear outputs or increase Output RAM budget for the next run.",
            self.mib()
        );
        Ok(())
    }
}
impl Default for MemoryBudget {
    fn default() -> Self {
        Self(256)
    }
}
impl TryFrom<usize> for MemoryBudget {
    type Error = &'static str;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if Self::PRESETS.iter().any(|preset| preset.0 == value) {
            Ok(Self(value))
        } else {
            Err("Notebook output RAM budget must be 64, 128, 256 or 512 MiB")
        }
    }
}
impl From<MemoryBudget> for usize {
    fn from(value: MemoryBudget) -> Self {
        value.0
    }
}

/// Encoded payload and a fixed allowance for its record/index. Cached by owners.
pub fn retained_bytes(output: &Value) -> Result<usize> {
    // JSON bytes conservatively cover string contents. Account tree/collection
    // allocations too, so millions of tiny JSON values cannot bypass the budget.
    fn index_bytes(value: &Value) -> usize {
        match value {
            Value::Array(values) => values
                .capacity()
                .saturating_mul(std::mem::size_of::<Value>())
                .saturating_add(values.iter().map(index_bytes).sum::<usize>()),
            Value::Object(values) => values
                .len()
                .saturating_mul(
                    std::mem::size_of::<(String, Value)>() + 6 * std::mem::size_of::<usize>(),
                )
                .saturating_add(values.values().map(index_bytes).sum::<usize>()),
            Value::String(text) => text.capacity().saturating_sub(text.len()),
            _ => 0,
        }
    }
    Ok(crate::json_size(output)?
        .saturating_add(index_bytes(output))
        .saturating_add(RECORD_OVERHEAD))
}

pub(super) fn configuration() -> Value {
    serde_json::json!({"bytes": MemoryBudget::default().bytes(), "overhead": RECORD_OVERHEAD,
        "chunk_chars": CHUNK_CHARS, "message": MAX_MESSAGE_BYTES})
}

/// Borrow compatible stream text without crossing a rich output or stdout/stderr
/// boundary. Callers validate its cost before publishing it.
/// Unknown fields must match so coalescing never discards notebook metadata.
pub fn stream_fragment<'a>(previous: &Value, next: &'a Value) -> Option<&'a str> {
    let (Some(old), Some(new)) = (previous.as_object(), next.as_object()) else {
        return None;
    };
    if old.get("output_type").and_then(Value::as_str) != Some("stream")
        || old.get("name").and_then(Value::as_str).is_none()
        || old.len() != new.len()
        || old
            .iter()
            .any(|(key, value)| key != "text" && new.get(key) != Some(value))
    {
        return None;
    }
    let (Some(_), Some(fragment)) = (
        old.get("text").and_then(Value::as_str),
        new.get("text").and_then(Value::as_str),
    ) else {
        return None;
    };
    Some(fragment)
}

/// Append a compatible stream fragment in place after its owner has admitted its cost.
pub fn merge_stream(previous: &mut Value, next: &Value) -> bool {
    let Some(fragment) = stream_fragment(previous, next) else {
        return false;
    };
    if let Value::String(text) = &mut previous["text"] {
        text.reserve_exact(fragment.len());
        text.push_str(fragment);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn budgets_roundtrip_and_many_outputs_share_one_allowance() {
        assert_eq!(MemoryBudget::default().mib(), 256);
        for preset in MemoryBudget::PRESETS {
            assert_eq!(
                serde_json::from_value::<MemoryBudget>(serde_json::to_value(preset).unwrap())
                    .unwrap(),
                preset
            );
        }
        for invalid in [0, 1, 8, usize::MAX] {
            assert!(serde_json::from_value::<MemoryBudget>(json!(invalid)).is_err());
        }
        let small = json!({"output_type":"display_data", "data":{"text/plain":"ok"}});
        MemoryBudget::default()
            .check(retained_bytes(&small).unwrap() * 10000)
            .unwrap();
        MemoryBudget::default()
            .check(retained_bytes(&json!({"data":{"text/html":"x".repeat(10*1024*1024)}})).unwrap())
            .unwrap();
        let indexed = json!({"metadata": vec![Value::Null; 10000]});
        assert!(
            retained_bytes(&indexed).unwrap()
                > crate::json_size(&indexed).unwrap() + 10000 * std::mem::size_of::<Value>()
        );
        assert!(
            MemoryBudget::default()
                .check(MemoryBudget::default().bytes() + 1)
                .is_err()
        );
    }
    #[test]
    fn stream_chunks_preserve_unicode_and_metadata() {
        let mut previous =
            json!({"output_type":"stream", "name":"stdout", "text":"日本語\n", "extra":7});
        let next = json!({"output_type":"stream", "name":"stdout", "text":"🦀\rnext", "extra":7});
        assert!(merge_stream(&mut previous, &next));
        assert_eq!(previous["text"], "日本語\n🦀\rnext");
        assert!(!merge_stream(
            &mut previous,
            &json!({"output_type":"stream", "name":"stderr", "text":"error", "extra":7})
        ));
    }
}

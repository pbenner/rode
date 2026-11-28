//! Reassemble one bounded rich-output event without widening the process transport.
use super::outputs::{CHUNK_CHARS, MAX_MESSAGE_BYTES};
use anyhow::{Context, Result, ensure};
use serde_json::Value;

#[derive(Default)]
pub(super) struct Incoming {
    id: Option<u64>,
    data: String,
}

impl Incoming {
    pub(super) fn message(&mut self, message: Value) -> Result<Option<Value>> {
        if message["kind"] != "output_chunk" {
            ensure!(self.id.is_none(), "Incomplete notebook output message");
            return Ok(Some(message));
        }
        let id = message["id"]
            .as_u64()
            .context("Missing notebook chunk execution")?;
        let chunk = message["data"]
            .as_str()
            .context("Missing notebook output fragment")?;
        let last = message["last"]
            .as_bool()
            .context("Missing notebook output fragment boundary")?;
        ensure!(
            self.id.is_none_or(|previous| previous == id),
            "Notebook chunk execution changed"
        );
        ensure!(
            chunk.len() <= CHUNK_CHARS * 4 && self.data.len() + chunk.len() <= MAX_MESSAGE_BYTES,
            "Notebook output fragments exceed their size limit"
        );
        self.id = Some(id);
        let needed = self.data.len() + chunk.len();
        if needed > self.data.capacity() {
            let capacity = needed
                .max(self.data.capacity().saturating_mul(2))
                .min(MAX_MESSAGE_BYTES);
            self.data.reserve_exact(capacity - self.data.len());
        }
        self.data.push_str(chunk);
        if !last {
            return Ok(None);
        }
        let value: Value = serde_json::from_str(&std::mem::take(&mut self.data))
            .context("Invalid chunked notebook output")?;
        self.id = None;
        ensure!(
            value["kind"] == "output" && value["id"].as_u64() == Some(id),
            "Invalid chunked notebook output identity"
        );
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fragments_preserve_unicode_and_reject_interleaving_and_excess() {
        let expected =
            json!({"kind":"output", "id":7, "output":{"text":"日本語 🦀".repeat(20000)}});
        let characters = expected.to_string().chars().collect::<Vec<_>>();
        let chunks = characters.chunks(CHUNK_CHARS).collect::<Vec<_>>();
        let mut incoming = Incoming::default();
        for (index, chunk) in chunks.iter().enumerate() {
            let result = incoming
                .message(json!({"kind":"output_chunk", "id":7,
                "data":chunk.iter().collect::<String>(), "last":index+1==chunks.len()}))
                .unwrap();
            if index + 1 == chunks.len() {
                assert_eq!(result.unwrap(), expected);
            } else {
                assert!(result.is_none());
            }
        }
        let first = json!({"kind":"output_chunk", "id":7,"data":"{", "last":false});
        incoming.message(first.clone()).unwrap();
        assert!(incoming.message(json!({"kind":"done", "id":7})).is_err());
        assert!(
            incoming
                .message(json!({"kind":"output_chunk", "id":8,"data":"}","last":true}))
                .is_err()
        );
        let mut incoming = Incoming::default();
        let chunk =
            json!({"kind":"output_chunk", "id":7, "data":"x".repeat(CHUNK_CHARS), "last":false});
        for _ in 0..MAX_MESSAGE_BYTES / CHUNK_CHARS {
            incoming.message(chunk.clone()).unwrap();
        }
        assert!(incoming.message(chunk).is_err());
        let mut incoming = Incoming::default();
        assert!(
            incoming
                .message(json!({"kind":"output_chunk", "id":7,"data":"[]", "last":true}))
                .is_err()
        );
    }
}

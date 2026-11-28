//! Bounded JSON packets for runtime attachments. Total output size is independent
//! of frame/line size; the writer never builds a second complete serialized cell.
use crate::notebook::outputs::{CHUNK_BYTES, MAX_MESSAGE_BYTES};
use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::io::{self, Write};

/// Serialize one value into ordinary small messages or UTF-8 fragment packets.
/// The emitter must preserve packet order on its existing authenticated channel.
pub fn send(value: &impl Serialize, mut emit: impl FnMut(&Value) -> Result<()>) -> Result<()> {
    struct Packets<'a, F> {
        buffer: Vec<u8>,
        chunked: bool,
        written: usize,
        emit: &'a mut F,
    }
    impl<F: FnMut(&Value) -> Result<()>> Write for Packets<'_, F> {
        fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
            let length = bytes.len();
            self.written = self.written.saturating_add(length);
            if self.written > MAX_MESSAGE_BYTES {
                return Err(io::Error::other("Runtime output exceeds its RAM allowance"));
            }
            while !bytes.is_empty() {
                let take = bytes.len().min(CHUNK_BYTES + 4 - self.buffer.len());
                self.buffer.extend_from_slice(&bytes[..take]);
                bytes = &bytes[take..];
                if self.buffer.len() > CHUNK_BYTES {
                    let end = match std::str::from_utf8(&self.buffer[..CHUNK_BYTES]) {
                        Ok(_) => CHUNK_BYTES,
                        Err(error) => error.valid_up_to(),
                    };
                    let text =
                        std::str::from_utf8(&self.buffer[..end]).map_err(io::Error::other)?;
                    (self.emit)(&json!({"kind":"runtime_chunk","data":text,"last":false}))
                        .map_err(io::Error::other)?;
                    self.buffer.drain(..end);
                    self.chunked = true;
                }
            }
            Ok(length)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Packets {
        buffer: Vec::with_capacity(CHUNK_BYTES + 4),
        chunked: false,
        written: 0,
        emit: &mut emit,
    };
    serde_json::to_writer(&mut writer, value)?;
    if writer.chunked {
        (writer.emit)(
            &json!({"kind":"runtime_chunk","data":std::str::from_utf8(&writer.buffer)?,"last":true}),
        )?;
    } else {
        (writer.emit)(&serde_json::from_slice(&writer.buffer)?)?;
    }
    Ok(())
}

/// Reassemble one runtime value under the largest supported RAM allowance.
/// Partial messages never escape to a view or report a completed cell.
#[derive(Default)]
pub struct Incoming {
    data: Vec<u8>,
    active: bool,
}
impl Incoming {
    /// Accept one ordered packet, returning a value only at its final boundary.
    pub fn message<T: DeserializeOwned>(&mut self, value: Value) -> Result<Option<T>> {
        if value["kind"] != "runtime_chunk" {
            ensure!(!self.active, "Incomplete runtime output message");
            return Ok(Some(serde_json::from_value(value)?));
        }
        let data = value["data"].as_str().context("Missing runtime fragment")?;
        let last = value["last"]
            .as_bool()
            .context("Missing runtime fragment boundary")?;
        ensure!(
            data.len() <= CHUNK_BYTES
                && self.data.len().saturating_add(data.len()) <= MAX_MESSAGE_BYTES,
            "Runtime fragments exceed their RAM allowance"
        );
        ensure!(
            last || !data.is_empty(),
            "Empty runtime continuation fragment"
        );
        self.active = true;
        let needed = self.data.len() + data.len();
        if needed > self.data.capacity() {
            let capacity = needed
                .max(self.data.capacity().saturating_mul(2))
                .min(MAX_MESSAGE_BYTES);
            self.data.reserve_exact(capacity - self.data.len());
        }
        self.data.extend_from_slice(data.as_bytes());
        if !last {
            return Ok(None);
        }
        self.active = false;
        let bytes = std::mem::take(&mut self.data);
        Ok(Some(
            serde_json::from_slice(&bytes).context("Invalid runtime output JSON")?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_unicode_cells_use_small_packets_and_small_states_stay_ordinary_json() {
        for value in [
            json!({"kind":"state"}),
            json!({"html":"日本語 🦀\\\"".repeat(2 * 1024 * 1024)}),
        ] {
            let mut incoming = Incoming::default();
            let mut result = None;
            let mut packets = 0;
            send(&value, |packet| {
                packets += 1;
                assert!(serde_json::to_vec(packet)?.len() < 1024 * 1024);
                if let Some(value) = incoming.message::<Value>(packet.clone())? {
                    assert!(result.is_none());
                    result = Some(value);
                }
                Ok(())
            })
            .unwrap();
            assert_eq!(result, Some(value.clone()));
            assert_eq!(packets == 1, value.get("html").is_none());
        }
        let mut incoming = Incoming::default();
        incoming
            .message::<Value>(json!({"kind":"runtime_chunk","data":"{","last":false}))
            .unwrap();
        assert!(incoming.message::<Value>(json!({"kind":"state"})).is_err());
    }
}

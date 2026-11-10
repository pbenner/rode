//! Bounded framing shared by service control, terminal streams and runtimes.
//!
//! A big-endian u32 counts the kind byte and payload; the length prefix itself
//! is excluded. Control/terminal frames allow 4 MiB, runtime frames 16 MiB.
//! Incoming lengths are checked before allocating their payload; outgoing sizes
//! are checked before writing a prefix. Raw terminal frames may have an empty
//! payload, while JSON messages must use kind `J` and valid JSON.
//! This codec does not authenticate peers or own attachment lifetimes.
use anyhow::{Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{Read, Write};

/// Maximum control/terminal frame size including its kind byte.
pub(super) const MAX_FRAME: usize = 4 * 1024 * 1024;
const CONTROL: Codec = Codec {
    limit: MAX_FRAME,
    name: "session",
};
/// Runtime envelopes carry notebook/debugger state with a separate byte budget.
pub(super) const RUNTIME: Codec = Codec {
    limit: 16 * 1024 * 1024,
    name: "runtime",
};

/// One wire format with a domain-specific inclusive size bound and error label.
pub(super) struct Codec {
    limit: usize,
    name: &'static str,
}

impl Codec {
    /// Inclusive frame size, counting the kind byte but excluding the prefix.
    pub(super) const fn limit(&self) -> usize {
        self.limit
    }

    /// Write a complete bounded frame and flush its destination.
    fn frame(&self, out: &mut impl Write, kind: u8, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() < self.limit,
            "{} message exceeds its {}-byte frame limit",
            self.name,
            self.limit
        );
        out.write_all(&((bytes.len() + 1) as u32).to_be_bytes())?;
        out.write_all(&[kind])?;
        out.write_all(bytes)?;
        out.flush()?;
        Ok(())
    }

    /// Validate the prefix before allocating; leave subsequent frames unread.
    fn read_frame(&self, input: &mut impl Read) -> Result<(u8, Vec<u8>)> {
        let mut size = [0; 4];
        input.read_exact(&mut size)?;
        let size = u32::from_be_bytes(size) as usize;
        ensure!(
            size > 0 && size <= self.limit,
            "Invalid {} frame size (maximum {} bytes)",
            self.name,
            self.limit
        );
        let mut kind = [0];
        input.read_exact(&mut kind)?;
        let mut bytes = vec![0; size - 1];
        input.read_exact(&mut bytes)?;
        Ok((kind[0], bytes))
    }

    /// Serialize one typed JSON message within this connection's frame budget.
    pub(super) fn send_json(&self, out: &mut impl Write, value: &impl Serialize) -> Result<()> {
        self.frame(out, b'J', &serde_json::to_vec(value)?)
    }

    /// Require JSON framing and deserialize a single message.
    pub(super) fn recv_json<T: DeserializeOwned>(&self, input: &mut impl Read) -> Result<T> {
        let (kind, bytes) = self.read_frame(input)?;
        ensure!(kind == b'J', "Expected a {} JSON message", self.name);
        Ok(serde_json::from_slice(&bytes)?)
    }
}

/// Write a raw control/terminal frame under the control connection's limit.
pub(super) fn frame(out: &mut impl Write, kind: u8, bytes: &[u8]) -> Result<()> {
    CONTROL.frame(out, kind, bytes)
}

/// Read a raw control/terminal frame under the control connection's limit.
pub(super) fn read_frame(input: &mut impl Read) -> Result<(u8, Vec<u8>)> {
    CONTROL.read_frame(input)
}

/// Encode a control or agent message, which shares the terminal frame budget.
pub(super) fn send_json(out: &mut impl Write, value: &impl Serialize) -> Result<()> {
    CONTROL.send_json(out, value)
}

/// Decode a control or agent message, rejecting terminal-only frame kinds.
pub(super) fn recv_json<R: Read, T: DeserializeOwned>(input: &mut R) -> Result<T> {
    CONTROL.recv_json(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const SMALL: Codec = Codec {
        limit: 4,
        name: "test",
    };

    #[test]
    fn exact_limit_and_empty_raw_frames_preserve_wire_bytes_and_message_boundaries() {
        let mut bytes = Vec::new();
        SMALL.frame(&mut bytes, b'O', b"abc").unwrap();
        SMALL.frame(&mut bytes, b'X', b"").unwrap();
        assert_eq!(bytes, b"\0\0\0\x04Oabc\0\0\0\x01X");
        let mut input = bytes.as_slice();
        assert_eq!(
            SMALL.read_frame(&mut input).unwrap(),
            (b'O', b"abc".to_vec())
        );
        assert_eq!(SMALL.read_frame(&mut input).unwrap(), (b'X', Vec::new()));
        assert!(input.is_empty());
    }

    #[test]
    fn oversized_output_is_rejected_before_writing_a_prefix() {
        let mut output = Vec::new();
        assert!(SMALL.frame(&mut output, b'O', b"abcd").is_err());
        assert!(SMALL.send_json(&mut output, &"ab").is_err());
        assert!(output.is_empty());
    }

    #[test]
    fn invalid_lengths_are_rejected_without_reading_a_body() {
        for size in [0_u32, 5, u32::MAX] {
            let mut bytes = size.to_be_bytes().to_vec();
            bytes.extend_from_slice(b"unread body");
            let mut input = Cursor::new(bytes);
            assert!(SMALL.read_frame(&mut input).is_err());
            assert_eq!(input.position(), 4);
        }
    }

    #[test]
    fn incomplete_frames_and_invalid_json_are_rejected() {
        for bytes in [
            b"\0\0".as_slice(),
            b"\0\0\0\x02",
            b"\0\0\0\x02J",
            b"\0\0\0\x02O1",
            b"\0\0\0\x01J",
            b"\0\0\0\x02Jx",
        ] {
            assert!(SMALL.recv_json::<u32>(&mut &bytes[..]).is_err());
        }
    }

    #[test]
    fn runtime_json_keeps_its_larger_limit_and_control_rejects_the_same_frame() {
        // A quoted string makes the complete frame one byte larger than control
        // allows while staying within the runtime's separately owned budget.
        let value = "x".repeat(MAX_FRAME - 2);
        let mut bytes = Vec::new();
        RUNTIME.send_json(&mut bytes, &value).unwrap();
        assert_eq!(
            u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize,
            MAX_FRAME + 1
        );
        assert!(recv_json::<_, String>(&mut bytes.as_slice()).is_err());
        assert_eq!(
            RUNTIME.recv_json::<String>(&mut bytes.as_slice()).unwrap(),
            value
        );
        let header = ((RUNTIME.limit() + 1) as u32).to_be_bytes();
        assert!(RUNTIME.recv_json::<String>(&mut header.as_slice()).is_err());
    }
}

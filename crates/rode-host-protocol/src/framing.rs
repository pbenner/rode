//! Bounded, chunked JSON messages over an authenticated byte stream.
use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};

mod stream;
pub use stream::{read_document, read_json_line, write_document};

/// Independent of the persistent-session protocol.
pub const VERSION: u32 = 1;
/// Reject shell banners and incompatible peers before interpreting any request.
pub const MAGIC: &[u8; 12] = b"RODE-HOST\0\0\x01";
/// Physical frames are small enough to bound individual transport allocations.
pub const CHUNK: usize = 64 * 1024;
/// Limit for control messages; document transfers use the streaming entry points.
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// Idle desktops renew each remote gateway and stream independently of GUI repaints.
pub const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
/// Abandon transient SSH work when its desktop stops sending input or heartbeats.
pub const CONNECTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_MESSAGE.saturating_sub(self.0.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Host message exceeds 64 MiB",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Encode with an allocation cap, then write bounded frames and an end marker.
pub fn write<T: Serialize>(output: &mut impl Write, value: &T) -> io::Result<()> {
    let mut bytes = Bounded(Vec::new());
    serde_json::to_writer(&mut bytes, value).map_err(io::Error::other)?;
    for chunk in bytes.0.chunks(CHUNK) {
        output.write_all(&(chunk.len() as u32).to_be_bytes())?;
        output.write_all(chunk)?;
    }
    output.write_all(&0u32.to_be_bytes())?;
    output.flush()
}

/// EOF is valid only between complete messages. Truncation never becomes an empty request.
pub fn read<T: DeserializeOwned>(input: &mut impl Read) -> io::Result<Option<T>> {
    let mut bytes = Vec::new();
    loop {
        let mut header = [0; 4];
        let n = loop {
            match input.read(&mut header[..1]) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => break result?,
            }
        };
        if n == 0 && bytes.is_empty() {
            return Ok(None);
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Truncated host message",
            ));
        }
        input.read_exact(&mut header[1..])?;
        let size = u32::from_be_bytes(header) as usize;
        if size == 0 {
            return serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(io::Error::other);
        }
        if size > CHUNK || size > MAX_MESSAGE.saturating_sub(bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Oversized host frame/message",
            ));
        }
        let start = bytes.len();
        bytes.resize(start + size, 0);
        input.read_exact(&mut bytes[start..])?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_messages_preserve_unicode_and_boundaries() {
        let value = "é".repeat(CHUNK);
        let mut bytes = vec![];
        write(&mut bytes, &value).unwrap();
        write(&mut bytes, &42).unwrap();
        let mut input = bytes.as_slice();
        assert_eq!(read::<String>(&mut input).unwrap(), Some(value));
        assert_eq!(read::<u32>(&mut input).unwrap(), Some(42));
        assert!(read::<u32>(&mut input).unwrap().is_none());
    }
    #[test]
    fn rejects_truncated_and_oversized_frames_before_allocation() {
        assert!(read::<String>(&mut [0u8, 1].as_slice()).is_err());
        assert!(read::<String>(&mut ((CHUNK + 1) as u32).to_be_bytes().as_slice()).is_err());
        let mut bytes = vec![];
        write(&mut bytes, &"hello").unwrap();
        bytes.truncate(bytes.len() - 4);
        assert!(read::<String>(&mut bytes.as_slice()).is_err());
    }
}

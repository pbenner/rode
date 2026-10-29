//! DAP framing is byte-counted, including non-ASCII JSON. Never read an
//! unbounded header or allocate an adapter-controlled content length.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::io::{BufRead, Write};
/// Maximum accepted DAP message body size in bytes.
pub const LIMIT: usize = 1024 * 1024;
/// Read one bounded Content-Length-framed JSON message; clean EOF before a header returns
/// `None`. Lengths count UTF-8 bytes, and malformed/truncated frames fail.
pub fn read(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut header = Vec::new();
    let mut length = None;
    let mut total = 0;
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            ensure!(total == 0, "Truncated debugger header.");
            return Ok(None);
        }
        let n = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| i + 1);
        total += n;
        ensure!(total <= 8192, "Debugger header exceeds 8 KiB.");
        header.extend_from_slice(&bytes[..n]);
        reader.consume(n);
        if !header.ends_with(b"\n") {
            continue;
        }
        let line = std::str::from_utf8(&header).context("Debugger header is not ASCII")?;
        ensure!(line.is_ascii(), "Debugger header is not ASCII.");
        if line == "\r\n" || line == "\n" {
            break;
        }
        let (name, value) = line
            .trim_end()
            .split_once(':')
            .context("Invalid debugger header")?;
        if name.eq_ignore_ascii_case("Content-Length") {
            ensure!(length.is_none(), "Duplicate debugger Content-Length.");
            let value = value.trim();
            ensure!(
                !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()),
                "Invalid debugger Content-Length."
            );
            let n = value.parse::<usize>()?;
            ensure!((1..=LIMIT).contains(&n), "Debugger message exceeds 1 MiB.");
            length = Some(n);
        }
        // Account for all header fields, while retaining only the current line.
        ensure!(header.len() < 8192, "Debugger header exceeds 8 KiB.");
        header.clear();
    }
    let mut body = vec![0; length.context("Missing debugger Content-Length")?];
    reader
        .read_exact(&mut body)
        .context("Truncated debugger message")?;
    Ok(Some(
        serde_json::from_slice(&body).context("Invalid debugger JSON")?,
    ))
}
/// Write one DAP JSON message with a byte-counted Content-Length header and flush it.
pub fn write(writer: &mut impl Write, message: &Value) -> Result<()> {
    let body = serde_json::to_vec(message)?;
    ensure!(body.len() <= LIMIT, "Debugger message exceeds 1 MiB.");
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf8_fragmented_frames_and_limits() {
        let value = serde_json::json!({"type":"event","body":"🦀\nλ"});
        let mut wire = vec![];
        write(&mut wire, &value).unwrap();
        let mut reader = std::io::BufReader::with_capacity(1, wire.as_slice());
        assert_eq!(read(&mut reader).unwrap(), Some(value));
        assert!(read(&mut reader).unwrap().is_none());
        for wire in [
            "Content-Length: 1048577\r\n\r\n",
            "Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "Content-Length: 20\r\n\r\n{}",
            "X-Unknown: foo\r\n",
        ] {
            assert!(read(&mut std::io::Cursor::new(wire)).is_err());
        }
        let many = "X-Unknown: hello\r\n".repeat(600) + "Content-Length: 2\r\n\r\n{}";
        assert!(read(&mut std::io::Cursor::new(many)).is_err());
    }
}

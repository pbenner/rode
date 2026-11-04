//! Bounded line framing shared by foreground and host-owned process transports.
use anyhow::{Result, bail};
use std::io::BufRead;

/// Read one bounded line. The stream name identifies
/// which budget failed; structured messages are never silently truncated.
pub fn read_line(reader: &mut impl BufRead, limit: usize, stream: &str) -> Result<Option<Vec<u8>>> {
    let mut result = Vec::new();
    loop {
        let data = reader.fill_buf()?;
        if data.is_empty() {
            return Ok((!result.is_empty()).then_some(result));
        }
        let len = data
            .iter()
            .position(|b| *b == b'\n')
            .map_or(data.len(), |p| p + 1);
        if len > limit.saturating_sub(result.len()) {
            bail!("{stream} line exceeds its size limit of {limit} bytes.");
        }
        result.extend_from_slice(&data[..len]);
        reader.consume(len);
        if result.last() == Some(&b'\n') {
            return Ok(Some(result));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn framing_keeps_boundaries_and_reports_the_overflowing_stream() {
        let mut reader = BufReader::with_capacity(2, Cursor::new(b"123\nnext"));
        assert_eq!(
            read_line(&mut reader, 4, "stdout").unwrap().unwrap(),
            b"123\n"
        );
        assert_eq!(
            read_line(&mut reader, 4, "stdout").unwrap().unwrap(),
            b"next"
        );
        assert!(read_line(&mut reader, 4, "stdout").unwrap().is_none());
        let mut reader = BufReader::with_capacity(2, Cursor::new(b"1234\n"));
        let error = read_line(&mut reader, 4, "stderr diagnostics")
            .unwrap_err()
            .to_string();
        assert!(error.contains("stderr diagnostics"));
        assert!(error.contains("4 bytes"));
    }
}

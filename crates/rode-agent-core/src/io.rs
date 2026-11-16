//! Bounded protocol framing and streaming diagnostics shared by foreground and
//! host-owned process transports. Diagnostics are not protocol frames: even a
//! large unterminated log entry must not disconnect a working conversation.
use crate::Event;
use anyhow::{Context, Result, bail};
use std::io::{BufRead, Read};
mod events;
pub use events::{EventReceiver, EventSender, event_channel};

/// Byte budget for retained diagnostics and individual decoded stderr chunks.
pub const DIAGNOSTIC_LIMIT: usize = 8192;

/// Deliver complete JSON messages under the caller's wire budget. Never truncate
/// a protocol message: its identifier, permissions or tool arguments may matter.
pub fn read_messages(
    reader: &mut impl BufRead,
    limit: usize,
    stream: &str,
    mut emit: impl FnMut(Event) -> bool,
) -> Result<()> {
    while let Some(bytes) = read_line(reader, limit, stream)? {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let value = serde_json::from_slice(&bytes).with_context(|| {
            format!("{stream} is not JSON-RPC. Check the selected protocol and arguments.")
        })?;
        drop(bytes);
        if !emit(Event::Message(value)) {
            break;
        }
    }
    Ok(())
}

/// Stream diagnostics without waiting for a newline or retaining an unbounded
/// line. Keep split UTF-8 characters between reads; replace malformed bytes only.
/// Returning false from the consumer stops reading when its connection closes.
pub fn read_diagnostics(reader: &mut impl Read, mut emit: impl FnMut(Event) -> bool) -> Result<()> {
    // Every invalid byte can expand into a three-byte replacement character.
    // Bound decoded chunks as well as the raw read buffer.
    let mut buffer = [0; DIAGNOSTIC_LIMIT / 3];
    let mut pending = 0;
    loop {
        let size = match reader.read(&mut buffer[pending..]) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if size == 0 {
            if pending > 0 {
                emit(Event::Stderr(
                    String::from_utf8_lossy(&buffer[..pending]).into(),
                ));
            }
            return Ok(());
        }
        let size = pending + size;
        let end = complete_utf8_len(&buffer[..size]);
        if end > 0
            && !emit(Event::Stderr(
                String::from_utf8_lossy(&buffer[..end]).into(),
            ))
        {
            return Ok(());
        }
        buffer.copy_within(end..size, 0);
        pending = size - end;
    }
}

fn complete_utf8_len(bytes: &[u8]) -> usize {
    let mut start = 0;
    loop {
        match std::str::from_utf8(&bytes[start..]) {
            Ok(_) => return bytes.len(),
            Err(error) => {
                start += error.valid_up_to();
                if let Some(size) = error.error_len() {
                    start += size;
                } else {
                    return start;
                }
            }
        }
    }
}

/// Keep the most recent diagnostic bytes, preserving original newlines and UTF-8.
/// Unlike a prefix clip, this retains the actual failure after verbose startup.
pub fn append_diagnostic(target: &mut String, text: &str) {
    let limit = DIAGNOSTIC_LIMIT;
    if text.len() >= limit {
        target.clear();
        let mut start = text.len() - limit;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        target.push_str(&text[start..]);
    } else {
        let mut remove = (target.len() + text.len()).saturating_sub(limit);
        while !target.is_char_boundary(remove) {
            remove += 1;
        }
        target.drain(..remove);
        target.push_str(text);
    }
}

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

    #[test]
    fn large_replies_preserve_fields_and_message_boundaries_without_widening_input() {
        use serde_json::json;
        let large = json!({"id":7,"result":{"text":"界".repeat(2 * 1024 * 1024)}});
        let mut bytes = serde_json::to_vec(&large).unwrap();
        assert!(bytes.len() > crate::LINE_LIMIT);
        bytes.extend_from_slice(b"\n{\"id\":8,\"result\":true}\n");
        let mut events = Vec::new();
        read_messages(
            &mut BufReader::with_capacity(13, Cursor::new(bytes)),
            crate::OUTPUT_LINE_LIMIT,
            "stdout",
            |event| {
                events.push(event);
                true
            },
        )
        .unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], Event::Message(value) if value == &large));
        assert!(
            matches!(&events[1], Event::Message(value) if value["id"] == 8 && value["result"] == true)
        );
    }

    #[test]
    fn malformed_and_oversized_protocol_frames_fail_without_delivering_a_partial_message() {
        for (bytes, limit, expected) in [
            (b"{\"id\":1,\"result\":true}\n".as_slice(), 8, "size limit"),
            (b"not JSON\n".as_slice(), 128, "not JSON-RPC"),
        ] {
            let mut events = Vec::new();
            let error = read_messages(&mut Cursor::new(bytes), limit, "stdout", |event| {
                events.push(event);
                true
            })
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
            assert!(events.is_empty());
        }
    }

    #[test]
    fn diagnostics_preserve_split_unicode_newlines_and_the_final_unterminated_error() {
        // Exercise every byte boundary, including an invalid byte before an
        // incomplete UTF-8 suffix. Partial code points flush only at EOF.
        for capacity in 1..12 {
            let bytes = b"startup\n\xff\xe7\x95\x8c\xf0\x9f\xa6\x80\npatch failed\nlast error\xe7";
            struct Short<'a> {
                bytes: &'a [u8],
                capacity: usize,
            }
            impl Read for Short<'_> {
                fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                    let size = self.capacity.min(output.len());
                    self.bytes.read(&mut output[..size])
                }
            }
            let mut output = String::new();
            read_diagnostics(&mut Short { bytes, capacity }, |event| {
                let Event::Stderr(text) = event else { panic!() };
                assert!(text.len() <= DIAGNOSTIC_LIMIT);
                output.push_str(&text);
                true
            })
            .unwrap();
            assert_eq!(output, String::from_utf8_lossy(bytes));
        }
    }

    #[test]
    fn diagnostics_stream_before_eof_and_keep_recent_errors_under_the_byte_budget() {
        let mut bytes = vec![b'x'; 2 * 1024 * 1024];
        bytes
            .extend_from_slice("\napply_patch verification failed: source changed 界🦀".as_bytes());
        let mut retained = String::new();
        let mut count = 0;
        read_diagnostics(&mut Cursor::new(bytes), |event| {
            let Event::Stderr(text) = event else { panic!() };
            append_diagnostic(&mut retained, &text);
            assert!(text.len() <= DIAGNOSTIC_LIMIT);
            assert!(retained.len() <= DIAGNOSTIC_LIMIT);
            count += 1;
            true
        })
        .unwrap();
        assert!(count > 1);
        assert!(retained.ends_with("apply_patch verification failed: source changed 界🦀"));

        // A backend can wait for more input without ending its stderr line.
        // The first chunk must reach the consumer without another read.
        struct Once(bool);
        impl Read for Once {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                assert!(!self.0, "Waited for a newline or EOF");
                self.0 = true;
                output[..5].copy_from_slice(b"error");
                Ok(5)
            }
        }
        read_diagnostics(&mut Once(false), |event| {
            assert!(matches!(event, Event::Stderr(text) if text == "error"));
            false
        })
        .unwrap();

        let mut invalid = String::new();
        read_diagnostics(&mut Cursor::new(vec![0xff; 20000]), |event| {
            let Event::Stderr(text) = event else { panic!() };
            assert!(text.len() <= DIAGNOSTIC_LIMIT);
            invalid.push_str(&text);
            true
        })
        .unwrap();
        assert_eq!(invalid, "�".repeat(20000));
        for text in ["🦀".repeat(DIAGNOSTIC_LIMIT), "é".repeat(DIAGNOSTIC_LIMIT)] {
            append_diagnostic(&mut retained, &text);
            append_diagnostic(&mut retained, "\nlatest error");
            assert!(retained.len() <= DIAGNOSTIC_LIMIT);
            assert!(retained.ends_with("\nlatest error"));
        }
    }
}

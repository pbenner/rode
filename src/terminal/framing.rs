//! SSH startup files can print before the attachment starts its framed stream.
//! Accept a bounded diagnostic prelude once, then keep strict framing for the
//! rest of the connection. This preserves older helpers without changing wire data.
use anyhow::{Context, Result, bail};
use std::io::{Cursor, Read};

const MAX_PRELUDE: usize = 64 * 1024;

/// Find the first terminal frame, publishing diagnostic lines while waiting.
/// Total startup output stays bounded even when already published in chunks.
pub(super) fn begin(
    input: &mut impl Read,
    mut diagnostic: impl FnMut(&[u8]),
) -> Result<(u8, Vec<u8>)> {
    let mut header = [0; 5];
    let mut count = 0;
    let mut prelude = Vec::new();
    let mut consumed = 0;
    loop {
        if let Err(error) = input.read_exact(&mut header[count..count + 1]) {
            let keep = count.min(MAX_PRELUDE - consumed);
            prelude.extend_from_slice(&header[..keep]);
            diagnostic(&prelude);
            if keep < count {
                bail!("Native terminal startup output exceeds 64 KiB");
            }
            return Err(error).context("Native terminal ended before its first frame");
        }
        count += 1;
        if count < header.len() {
            continue;
        }
        let size = u32::from_be_bytes(header[..4].try_into().unwrap());
        let valid = match header[4] {
            b'S' => size == 5,
            b'P' => size == 2,
            b'O' | b'X' => (1..=4 * 1024 * 1024).contains(&size),
            _ => false,
        };
        if valid {
            diagnostic(&prelude);
            return rode_session::read_stream(&mut Cursor::new(header).chain(input));
        }
        if consumed == MAX_PRELUDE {
            diagnostic(&prelude);
            bail!("Native terminal startup output exceeds 64 KiB");
        }
        prelude.push(header[0]);
        consumed += 1;
        header.copy_within(1.., 0);
        count = 4;
        // Every supported frame length starts with NUL. A text-only window
        // ending in newline cannot overlap a frame, so publish the whole line
        // now instead of waiting for the remote command to produce more data.
        if header[count - 1] == b'\n' && !header[..count].contains(&0) {
            let keep = count.min(MAX_PRELUDE - consumed);
            prelude.extend_from_slice(&header[..keep]);
            consumed += keep;
            diagnostic(&prelude);
            prelude.clear();
            if keep < count {
                bail!("Native terminal startup output exceeds 64 KiB");
            }
            count = 0;
        } else if prelude.last() == Some(&b'\n') || prelude.len() >= 4096 {
            diagnostic(&prelude);
            prelude.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_diagnostics_do_not_consume_frames_or_binary_output() {
        use std::{cell::Cell, rc::Rc};
        struct Progress {
            input: Cursor<Vec<u8>>,
            boundary: u64,
            visible: Rc<Cell<bool>>,
        }
        impl Read for Progress {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                if self.input.position() == self.boundary {
                    assert!(
                        self.visible.get(),
                        "Startup diagnostics must be visible before waiting for a frame"
                    );
                }
                self.input.read(bytes)
            }
        }
        let mut bytes = b"/home/user/.bashrc: missing command\r\n".to_vec();
        let boundary = bytes.len() as u64;
        bytes.extend_from_slice(b"\0\0\0\x05S\0\x18\0\x64");
        bytes.extend_from_slice(b"\0\0\0\x04Ox\0y");
        let visible = Rc::new(Cell::new(false));
        let mut input = Progress {
            input: Cursor::new(bytes),
            boundary,
            visible: visible.clone(),
        };
        let mut warning = Vec::new();
        let first = begin(&mut input, |bytes| {
            warning.extend_from_slice(bytes);
            visible.set(true);
        })
        .unwrap();
        assert_eq!(warning, b"/home/user/.bashrc: missing command\r\n");
        assert_eq!(first, (b'S', vec![0, 24, 0, 100]));
        assert_eq!(
            rode_session::read_stream(&mut input).unwrap(),
            (b'O', b"x\0y".to_vec())
        );
    }

    #[test]
    fn startup_output_is_bounded_and_survives_early_eof() {
        let mut warning = Vec::new();
        assert!(
            begin(&mut Cursor::new(b"login failed\r\n"), |bytes| warning
                .extend_from_slice(bytes))
            .is_err()
        );
        assert_eq!(warning, b"login failed\r\n");
        warning.clear();
        let error = begin(&mut std::io::repeat(b'w'), |bytes| {
            warning.extend_from_slice(bytes)
        })
        .unwrap_err();
        assert!(error.to_string().contains("64 KiB"));
        assert_eq!(warning.len(), MAX_PRELUDE);
        warning.clear();
        let lines = "warning\n".repeat(MAX_PRELUDE / 8 + 1);
        let error = begin(&mut Cursor::new(lines), |bytes| {
            warning.extend_from_slice(bytes)
        })
        .unwrap_err();
        assert!(error.to_string().contains("64 KiB"));
        assert_eq!(
            warning.len(),
            MAX_PRELUDE,
            "Published lines still consume the total budget"
        );
    }

    #[test]
    fn newline_in_a_binary_frame_length_is_not_a_diagnostic_line() {
        let mut input = b"x\0\0\0\x0aO123456789".as_slice();
        let mut warning = Vec::new();
        let frame = begin(&mut input, |bytes| warning.extend_from_slice(bytes)).unwrap();
        assert_eq!(warning, b"x");
        assert_eq!(frame, (b'O', b"123456789".to_vec()));
    }
}

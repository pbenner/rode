//! Stdio command forwarding shared by agent and runtime attachments.
//!
//! The caller authenticates the socket and chooses its typed command, inclusive
//! input-line limit and wire encoder. This module owns only the stdin worker:
//! EOF, malformed/oversized input and write errors close both socket directions
//! so the output reader wakes and the service releases the controlling lease.
use anyhow::{Result, ensure};
use serde::de::DeserializeOwned;
use std::{
    io::{BufRead, BufReader, Read},
    net::Shutdown,
    os::unix::net::UnixStream,
};

/// Spawn a command-only stdin worker for an already authenticated attachment.
/// Limits count encoded bytes including the optional trailing newline. Agent
/// and runtime output decoding and lifetime rules remain with their owners.
pub(super) fn forward_commands<T: DeserializeOwned + 'static>(
    mut stream: UnixStream,
    line_limit: usize,
    send: fn(&mut UnixStream, &T) -> Result<()>,
) {
    std::thread::spawn(move || {
        if let Ok(mut input) = rode_host_core::connection::Input::stdin() {
            input.unbounded();
            let _ = forward(&mut BufReader::new(input), line_limit, |command| {
                send(&mut stream, &command)
            });
        }
        let _ = stream.shutdown(Shutdown::Both);
    });
}

/// Deserialize one bounded line at a time; stop before forwarding invalid input.
fn forward<T: DeserializeOwned>(
    input: &mut impl BufRead,
    line_limit: usize,
    mut send: impl FnMut(T) -> Result<()>,
) -> Result<()> {
    loop {
        let mut bytes = Vec::new();
        let count = input
            .by_ref()
            .take(line_limit as u64 + 1)
            .read_until(b'\n', &mut bytes)?;
        if count == 0 {
            return Ok(());
        }
        ensure!(
            count <= line_limit,
            "Attachment command exceeds its line limit"
        );
        send(serde_json::from_slice(&bytes)?)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_typed_commands_in_order_including_a_final_line_without_newline() {
        let mut values = Vec::new();
        forward(&mut &b"1\n22\n333"[..], 3, |value: u32| {
            values.push(value);
            Ok(())
        })
        .unwrap();
        assert_eq!(values, [1, 22, 333]);
    }

    #[test]
    fn rejects_oversized_or_invalid_commands_before_forwarding_them() {
        for bytes in [b"1234\n".as_slice(), b"{}\n", b"no\n"] {
            let mut values = Vec::new();
            assert!(
                forward(&mut &bytes[..], 3, |value: u32| {
                    values.push(value);
                    Ok(())
                })
                .is_err()
            );
            assert!(values.is_empty());
        }
    }

    #[test]
    fn stops_after_a_write_failure_without_forwarding_later_commands() {
        let mut values = Vec::new();
        assert!(
            forward(&mut &b"1\n2\n"[..], 3, |value: u32| {
                values.push(value);
                anyhow::bail!("Attachment closed")
            })
            .is_err()
        );
        assert_eq!(values, [1]);
    }
}

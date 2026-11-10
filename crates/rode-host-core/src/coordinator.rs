//! One-request stdio coordinators for cancellable project-host tools.
//!
//! Each dedicated helper reads a bounded JSON request, then treats the remaining
//! stdin lifetime as cancellation. The tool retains its own host, sandbox,
//! timeout and output bounds. Stdout contains only its serialized reply.
use anyhow::{Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    io::{BufRead, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Run in a dedicated helper process, never on the GUI thread. A byte, EOF or
/// read error after the request asks the tool to cancel; its runner owns cleanup.
pub(crate) fn serve<I: DeserializeOwned, O: Serialize>(
    request_limit: u64,
    run: impl FnOnce(&I, &AtomicBool) -> Result<O>,
) -> Result<()> {
    let request = read_request(&mut std::io::stdin().lock(), request_limit)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let control = cancel.clone();
    std::thread::spawn(move || {
        let _ = std::io::stdin().lock().read(&mut [0]);
        control.store(true, Ordering::Release);
    });
    serde_json::to_writer(std::io::stdout().lock(), &run(&request, &cancel)?)?;
    Ok(())
}

/// Count the optional request newline in the byte limit without consuming the
/// following lifetime channel or accepting an oversized request's valid prefix.
fn read_request<T: DeserializeOwned>(input: &mut impl BufRead, limit: u64) -> Result<T> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(limit.saturating_add(1))
        .read_until(b'\n', &mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Tool request exceeds its byte limit"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_reader_preserves_the_following_lifetime_channel() {
        let mut input = &b"\"hello\\nworld\"\nX"[..];
        let value: String = read_request(&mut input, 16).unwrap();
        assert_eq!(value, "hello\nworld");
        assert_eq!(input, b"X");
    }

    #[test]
    fn request_limits_include_newlines_and_reject_valid_but_truncated_prefixes() {
        assert_eq!(read_request::<u32>(&mut &b"12\n"[..], 3).unwrap(), 12);
        assert_eq!(read_request::<u32>(&mut &b"123"[..], 3).unwrap(), 123);
        for bytes in [b"1234\n".as_slice(), b"12\n", b"12 3", b"{}\n", b""] {
            assert!(read_request::<u32>(&mut &bytes[..], 2).is_err());
        }
    }
}

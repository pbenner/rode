//! Allocation-free size accounting for retained runtime wire values.
use serde::Serialize;
use std::io::Write;

/// Count compact JSON bytes without allocating a second serialized copy.
/// Uses the same encoder as runtime messages, including UTF-8 and escaping.
pub fn json_size(value: &impl Serialize) -> anyhow::Result<usize> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("Encoded JSON size overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

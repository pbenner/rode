//! Host identity shared by display labels and host-local coordination paths.

/// Read this machine's hostname without starting a process or doing DNS or
/// filesystem work. An unavailable name cannot identify shared-home state.
pub fn hostname() -> Option<String> {
    let mut bytes = [0_u8; 256];
    // SAFETY: gethostname receives a writable buffer of the stated length. Read
    // only that buffer, even on platforms that omit the trailing NUL on truncation.
    if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return None;
    }
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let hostname = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
    (!hostname.is_empty() && !hostname.chars().any(char::is_control)).then_some(hostname)
}

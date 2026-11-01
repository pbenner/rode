//! Identify fixture children in the test runner's PID namespace before cleanup.

use std::{fs, os::unix::ffi::OsStrExt, path::Path};

pub fn host_pid(marker: &Path) -> u32 {
    let marker = marker.as_os_str().as_bytes();
    let matches: Vec<_> = fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let command = fs::read(entry.path().join("cmdline")).ok()?;
            command
                .split(|byte| *byte == 0)
                .any(|argument| argument == marker)
                .then_some(pid)
        })
        .collect();
    assert_eq!(matches.len(), 1, "Expected exactly one live fixture child");
    matches[0]
}

pub fn running(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, fields)| !fields.starts_with('Z'))
    })
}

//! Report optional coverage independently of libtest's successful-output capture.
#![allow(dead_code)]
use std::{
    fs::OpenOptions,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};

pub fn skip(capability: &str, reason: &str) {
    assert!(
        !std::env::var("RODE_TEST_REQUIRED_CAPABILITIES")
            .unwrap_or_default()
            .split(',')
            .any(|name| name == capability),
        "Required capability {capability}: {reason}"
    );
    let thread = std::thread::current();
    let test = thread.name().unwrap_or("fixture");
    eprintln!("SKIP {test} [{capability}]: {reason}");
    if let Some(directory) = std::env::var_os("RODE_TEST_SKIP_DIR") {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        std::fs::create_dir_all(&directory).unwrap();
        let path = std::path::PathBuf::from(directory).join(format!(
            "{}-{}.json",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"capability": capability, "reason": reason, "test": test})
        )
        .unwrap();
    }
}

pub fn tool(capability: &str, variables: &[&str]) -> Option<String> {
    if let Some(value) = variables
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
    {
        return Some(value);
    }
    skip(
        capability,
        &format!("Set {} to enable this case", variables.join(" or ")),
    );
    None
}

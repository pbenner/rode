//! Desktop discovery of the matching native session helper.
use crate::project;
use anyhow::{Context, Result, ensure};
pub use rode_session::workspace::{Helper, Operation, Reply, fingerprint, initial_input};
use std::path::PathBuf;
/// Use the matching service executable without replacing existing jobs.
pub fn local(operation: &Operation) -> Result<Reply> {
    let exe = rode_session::current_executable()?;
    // The editor's embedded service always matches its client. A sibling
    // binary can be stale after cargo build or an unpackaged update.
    let standalone = std::env::var_os("RODE_SESSION_HELPER").map(PathBuf::from);
    // Library-test executables have Rust's test harness as their main, so they
    // cannot implement the editor's --session-service entry point themselves.
    #[cfg(test)]
    let standalone = standalone.or_else(|| {
        exe.parent()
            .and_then(std::path::Path::parent)
            .map(|p| p.join("rode-sessiond"))
            .filter(|p| p.is_file())
    });
    if let Some(path) = &standalone {
        let mut command = std::process::Command::new(path);
        command.arg("protocol");
        let output = project::run_controlled_limited(
            command,
            vec![],
            std::time::Duration::from_secs(5),
            &std::sync::atomic::AtomicBool::new(false),
            false,
            4096,
            std::time::Duration::ZERO,
        )
        .context("Cannot check RODE_SESSION_HELPER; rebuild it with cargo xtask build-helper")?;
        ensure!(
            output.success() && output.stdout.trim() == rode_session::PROTOCOL.to_string(),
            "RODE_SESSION_HELPER uses an older terminal protocol. Rebuild it with cargo xtask build-helper or unset RODE_SESSION_HELPER to use the editor's matching service. Existing jobs were not touched."
        );
    }
    let launcher = match standalone {
        Some(executable) => rode_session::Launcher {
            executable,
            prefix: vec![],
        },
        None => rode_session::Launcher {
            executable: exe,
            prefix: vec!["--session-service".into()],
        },
    };
    rode_session::workspace::local(operation, launcher)
}

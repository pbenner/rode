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
        .context("Cannot check RODE_SESSION_HELPER; rebuild it with scripts/build-sessiond.sh")?;
        ensure!(
            output.success() && output.stdout.trim() == rode_session::PROTOCOL.to_string(),
            "RODE_SESSION_HELPER uses an older terminal protocol. Rebuild it with scripts/build-sessiond.sh or unset RODE_SESSION_HELPER to use the editor's matching service. Existing jobs were not touched."
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
/// Prefer the static helper for remote hosts; it needs no desktop libraries.
pub fn helper(with_data: bool) -> Result<Helper> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let exe = std::env::current_exe()?;
    let bin = exe.parent().context("Missing binary directory")?;
    let mut candidates = vec![];
    if let Some(path) = std::env::var_os("RODE_SESSION_HELPER") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(bin.join("rode-sessiond-static"));
    if let Some(target) = bin.parent() {
        candidates.push(
            target
                .join(format!("{}-unknown-linux-musl", std::env::consts::ARCH))
                .join(bin.file_name().unwrap())
                .join("rode-sessiond-standalone"),
        );
        // Preserve discovery of static helpers built before the Cargo install target.
        candidates.push(
            target
                .join(format!("{}-unknown-linux-musl", std::env::consts::ARCH))
                .join(bin.file_name().unwrap())
                .join("rode-sessiond"),
        );
    }
    candidates.push(bin.join("rode-sessiond"));
    let path=candidates.into_iter().find(|p|p.is_file()).context("Remote persistent terminals need rode-sessiond beside the editor. Reinstall with cargo install --locked --path . from the RODE source checkout, use scripts/build-sessiond.sh for a static helper, or set RODE_SESSION_HELPER to a compatible helper binary.")?;
    let bytes = std::fs::read(&path)?;
    ensure!(
        bytes.len() <= 24 * 1024 * 1024,
        "Session helper exceeds 24 MiB; use a release build"
    );
    ensure!(
        bytes.starts_with(b"\x7fELF") && bytes.len() > 20,
        "Remote session helper must be a Linux ELF binary"
    );
    let architecture = match u16::from_le_bytes([bytes[18], bytes[19]]) {
        62 => "x86_64",
        183 => "aarch64",
        _ => anyhow::bail!("Unsupported remote helper architecture"),
    };
    Ok(Helper {
        digest: crate::files::digest(&bytes),
        architecture: architecture.into(),
        data: with_data.then(|| STANDARD.encode(bytes)),
    })
}

/// The connection handshake owns verified remote helper deployment.
pub fn remote_helper(_remote: &crate::ssh::Remote, _with_data: bool) -> Result<Option<Helper>> {
    Ok(None)
}

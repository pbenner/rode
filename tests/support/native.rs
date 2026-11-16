//! Exercise the shared host services through a separate headless Rust process.
#![allow(dead_code)]
#[path = "capabilities.rs"]
pub mod capabilities;
use std::{path::Path, process::Command};
pub fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_rode-sessiond")
}
pub fn command() -> Command {
    let mut command = Command::new(binary());
    command.arg("--file-helper");
    command
}

/// Publish a private, deployable snapshot of the selected helper without changing
/// Cargo's image or a caller-supplied artifact. Unstripped builds can exceed the
/// deployment size bound; strip only the snapshot when needed, never relax it.
pub fn copy_deployable(source: &Path, destination: &Path) -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

    let parent = destination
        .parent()
        .context("Missing test helper directory")?;
    let pending = tempfile::tempdir_in(parent)?;
    let snapshot = pending.path().join("rode-sessiond");
    fs::copy(source, &snapshot).context("Cannot copy the selected native test helper")?;
    let limit = rode_host_protocol::helper::MAX_EXECUTABLE;
    if snapshot.metadata()?.len() > limit {
        let mut strip = Command::new("strip");
        strip.arg("--strip-unneeded").arg(&snapshot);
        rode::project::run(strip, vec![], Duration::from_secs(20))
            .context("Cannot strip the oversized native test helper; install binutils")?
            .checked()?;
    }
    ensure!(
        snapshot.metadata()?.len() <= limit,
        "Native test helper still exceeds the deployment size limit after stripping"
    );
    fs::set_permissions(&snapshot, fs::Permissions::from_mode(0o700))?;
    // A hard link publishes complete bytes atomically, refuses an existing
    // destination and retains no alias to Cargo's original executable.
    fs::hard_link(&snapshot, destination).context("Cannot publish the native test helper")?;
    Ok(())
}

pub fn prepare(
    root: &str,
    options: &rode::sandbox::Options,
) -> anyhow::Result<rode::sandbox::Plan> {
    rode::sandbox::prepare_with_helper(root, options, Path::new(binary()))
}

pub fn environment_prefix(
    settings: &rode::environment::Settings,
    root: &str,
    python: &str,
) -> anyhow::Result<Vec<String>> {
    rode::sandbox::Plan {
        root: root.into(),
        native_helper: Some(binary().into()),
        environment: settings.clone(),
        ..Default::default()
    }
    .environment_prefix(python)
}

/// Helper paths differ between retained desktop images and immutable headless artifacts.
/// All other grants and launch arguments must still match exactly.
pub fn policy_arguments(plan: &rode::sandbox::Plan) -> Vec<String> {
    plan.prefix
        .iter()
        .map(|argument| {
            if plan.native_helper.as_ref() == Some(argument) {
                "<native-helper>".into()
            } else {
                argument.clone()
            }
        })
        .collect()
}

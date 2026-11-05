//! Bounded client-owned command execution under an already enforced offline
//! project plan. This entry point accepts no policy or executable from the
//! model; controllers supply the original captured plan and environment.
use crate::{project, sandbox::Plan};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, process::Command, sync::atomic::AtomicBool, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    command: String,
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
}

fn default_timeout() -> u64 {
    30
}

/// Run one noninteractive shell command and return its exit status and bounded
/// output. Cancellation, timeout and overflow stop the owned process group.
/// Missing enforcement is an error and never falls back to normal execution.
pub fn run(
    plan: &Plan,
    arguments: &Value,
    environment: &BTreeMap<String, String>,
    cancel: &AtomicBool,
) -> Result<Value> {
    ensure!(
        plan.active && !plan.prefix.is_empty() && !plan.prefix.iter().any(|v| v == "--share-net"),
        "Native agent commands require an active offline sandbox"
    );
    ensure!(
        !plan.project_read_only,
        "Commands are unavailable in a read-only project"
    );
    let args: Arguments = serde_json::from_value(arguments.clone())?;
    ensure!(
        !args.command.trim().is_empty()
            && args.command.len() <= 32 * 1024
            && !args.command.contains('\0'),
        "Command must contain 1–32768 bytes without NUL characters"
    );
    ensure!(
        (1..=120).contains(&args.timeout_seconds),
        "Command timeout must be between 1 and 120 seconds"
    );
    let argv = plan.execution_prefix("")?;
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .args(["/bin/sh", "-c", &args.command])
        .current_dir(&plan.root)
        .env_clear()
        .envs(environment);
    let output = project::run_controlled_limited(
        command,
        vec![],
        Duration::from_secs(args.timeout_seconds),
        cancel,
        false,
        256 * 1024,
        Duration::ZERO,
    )?;
    Ok(json!({"exit_code":output.code,"stdout":output.stdout,"stderr":output.stderr}))
}

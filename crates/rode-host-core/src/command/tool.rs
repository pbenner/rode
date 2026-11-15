//! UTF-8 tool results and adapters over the shared bounded process-capture engine.
//!
//! These adapters preserve tool exit status and reject invalid UTF-8. Binary
//! transfers retain the byte-oriented command API and its independent budgets.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{io::Read, process::Command, sync::atomic::AtomicBool, time::Duration};

/// Default tool stream budget, also bounding Git patch input at the service boundary.
pub(crate) const OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;

/// Bounded UTF-8 command output and an exit status.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Output {
    /// Exit code; `-1` represents termination without a conventional exit code.
    pub code: i32,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}
impl Output {
    /// Return whether the command exited with code zero.
    pub fn success(&self) -> bool {
        self.code == 0
    }
    /// Return stdout on success, or diagnostics with an exit-status fallback on failure.
    pub fn checked(self) -> Result<String> {
        if self.success() {
            Ok(self.stdout)
        } else {
            let diagnostics = [self.stderr.trim(), self.stdout.trim()]
                .into_iter()
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if !diagnostics.is_empty() {
                bail!("{diagnostics}");
            }
            if self.code < 0 {
                bail!("Command terminated without an exit code");
            }
            bail!("Command exited with status {}", self.code)
        }
    }
}

/// Drain both pipes concurrently, cap allocations, and stop the owned command
/// group on completion, timeout or overflow (including hooks and children).
pub fn run(command: Command, input: Vec<u8>, timeout: Duration) -> Result<Output> {
    run_controlled(command, input, timeout, &AtomicBool::new(false), false)
}

/// Run an owned command with concurrent bounded I/O, cancellation and a deadline.
/// `lifeline` retains stdin after the initial payload so helper cleanup follows channel loss.
pub fn run_controlled(
    command: Command,
    input: Vec<u8>,
    timeout: Duration,
    cancel: &AtomicBool,
    lifeline: bool,
) -> Result<Output> {
    run_controlled_limited(
        command,
        input,
        timeout,
        cancel,
        lifeline,
        OUTPUT_LIMIT,
        Duration::ZERO,
    )
}
/// Use explicit per-stream output-byte bounds and termination grace before scoped
/// process-group escalation; return captured exit/output only after pipe cleanup.
pub fn run_controlled_limited(
    command: Command,
    input: Vec<u8>,
    timeout: Duration,
    cancel: &AtomicBool,
    lifeline: bool,
    output_limit: u64,
    terminate_grace: Duration,
) -> Result<Output> {
    run_controlled_stream(
        command,
        std::io::Cursor::new(input),
        timeout,
        cancel,
        lifeline,
        output_limit,
        terminate_grace,
    )
}

/// Stream an owned input without retaining large executable transfers in memory.
/// The same cancellation, output bounds and process-group cleanup apply as for tools.
pub fn run_controlled_stream(
    command: Command,
    input: impl Read + Send + 'static,
    timeout: Duration,
    cancel: &AtomicBool,
    lifeline: bool,
    output_limit: u64,
    terminate_grace: Duration,
) -> Result<Output> {
    let output = super::capture::run(
        command,
        input,
        timeout,
        cancel,
        lifeline,
        super::capture::Limits {
            stdout: output_limit,
            stderr: output_limit,
        },
        terminate_grace,
    )?;
    Ok(Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).context("Tool output is not UTF-8")?,
        stderr: String::from_utf8(output.stderr).context("Tool output is not UTF-8")?,
    })
}

//! Scheduler allocation state, kept distinct from physical GPU utilization.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Command, time::Duration};
mod interactive;
pub use interactive::{Allocation, Launch};
/// One user-owned Slurm allocation; scheduler resource counts are distinct from live GPU
/// utilization.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    /// Exact allocation id, including a single array/heterogeneous component when present.
    pub id: String,
    /// Scheduler state label.
    pub state: String,
    /// Scheduler-formatted elapsed time.
    pub elapsed: String,
    /// Scheduler-formatted remaining time.
    pub remaining: String,
    /// Assigned or pending node-list text.
    pub nodes: String,
    /// Allocated trackable resources string from `squeue`.
    pub tres: String,
    /// Allocation's display name.
    pub name: String,
    /// Allocated GPU count when known; `None` means unavailable, not zero.
    pub gpus: Option<u64>,
}
impl Job {
    /// Return whether the scheduler state still holds or is configuring an allocation.
    pub fn allocated(&self) -> bool {
        matches!(
            self.state.as_str(),
            "RUNNING" | "COMPLETING" | "SUSPENDED" | "RESIZING" | "STAGE_OUT" | "CONFIGURING"
        )
    }
}
/// Bounded snapshot of the current user's allocations on the project host.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// Discovered allocations, capped by the protocol limit.
    pub jobs: Vec<Job>,
    /// Nonfatal scheduler availability/format explanation.
    pub warning: String,
}
/// Accept only one exact numeric allocation id, optionally with array/heterogeneous suffixes;
/// reject ranges, steps and command arguments.
pub fn valid_id(id: &str) -> bool {
    let mut parts = id.split('+');
    let first = parts.next().unwrap_or("");
    let mut array = first.split('_');
    let head = array.next().unwrap_or("");
    let numeric = |value: &str| {
        !value.is_empty() && value.len() <= 20 && value.bytes().all(|c| c.is_ascii_digit())
    };
    numeric(head)
        && array.next().is_none_or(numeric)
        && array.next().is_none()
        && parts.next().is_none_or(numeric)
        && parts.next().is_none()
}
/// Read allocated GPU counts from TRES, preferring the generic total over typed subtotals to
/// avoid double counting.
pub fn gpu_count(tres: &str) -> Option<u64> {
    if tres.trim().is_empty() || matches!(tres.trim(), "N/A" | "(null)" | "None") {
        return None;
    }
    let mut typed = 0u64;
    let mut generic = None;
    for token in tres.split(',') {
        if let Some((key, value)) = token.trim().split_once('=') {
            if key == "gres/gpu" {
                generic = Some(value.parse().ok()?);
            } else if key.starts_with("gres/gpu:") {
                typed = typed.checked_add(value.parse().ok()?)?;
            }
        }
    }
    generic.or(Some(typed))
}
/// Parse the controlled pipe-delimited `squeue` output format with bounded records.
pub fn parse(text: &str) -> Result<Snapshot> {
    let mut jobs = vec![];
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.splitn(7, '|').map(str::trim).collect();
        ensure!(
            fields.len() == 7 && valid_id(fields[0]),
            "Slurm returned an unsupported job record"
        );
        jobs.push(Job {
            id: fields[0].into(),
            state: fields[1].into(),
            elapsed: fields[2].into(),
            remaining: fields[3].into(),
            nodes: fields[4].into(),
            tres: fields[5].into(),
            name: fields[6].into(),
            gpus: gpu_count(fields[5]),
        });
        ensure!(
            jobs.len() <= 512,
            "More than 512 Slurm jobs; use squeue in the terminal to inspect them all"
        );
    }
    Ok(Snapshot {
        jobs,
        warning: String::new(),
    })
}
fn command(program: &str, root: &Path) -> Command {
    let program = if program.contains('/') || program.starts_with('~') {
        let p = crate::files::expand_home(program);
        if p.is_absolute() { p } else { root.join(p) }
    } else {
        program.into()
    };
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .env("LC_ALL", "C")
        .env_remove("SLURM_CLUSTERS");
    for (key, _) in std::env::vars_os() {
        if ["SCANCEL_", "SQUEUE_"]
            .iter()
            .any(|prefix| key.to_string_lossy().starts_with(prefix))
        {
            command.env_remove(key);
        }
    }
    command
}
/// Query local-host jobs for the effective UID using an installed queue executable; inherited
/// Slurm target/filter overrides are removed.
pub fn snapshot(root: &Path, queue: &str) -> Result<Snapshot> {
    let mut cmd = command(queue, root);
    cmd.args([
        "--noheader",
        "--array",
        "--local",
        "--user",
        &unsafe { libc::geteuid() }.to_string(),
        "--Format",
        "JobID:0|,State:0|,TimeUsed:0|,TimeLeft:0|,NodeList:0|,tres-alloc:0|,Name:0",
    ]);
    let text=crate::project::run(cmd,vec![],Duration::from_secs(15)).context("Cannot query Slurm. Configure the squeue executable or a wrapper that loads your site's Slurm environment.")?.checked()?;
    parse(&text)
}
/// Recheck ownership in a fresh snapshot, then cancel one exact allocation with the installed
/// cancellation executable. Requires explicit user action and never cancels by wildcard.
pub fn cancel(root: &Path, queue: &str, program: &str, id: &str) -> Result<crate::project::Output> {
    ensure!(
        valid_id(id),
        "Only exact Slurm allocation IDs may be released (not steps, ranges, or arbitrary arguments)"
    );
    ensure!(
        snapshot(root, queue)?.jobs.iter().any(|job| job.id == id),
        "Allocation is no longer in your jobs on this host; refresh before releasing it"
    );
    let mut command = command(program, root);
    command.args(["--user", &unsafe { libc::geteuid() }.to_string(), "--", id]);
    crate::project::run(command, vec![], Duration::from_secs(20))
}

//! Identity-checked stash removal for Git's files reference backend.
//!
//! `git stash drop stash@{n}` has no expected-object argument. Checking with a
//! separate Git command would leave a race with other Git writers. Hold the
//! reference lock through a prepared Git transaction while checking and rewriting the
//! reflog, including its old-object chain, then update the stash tip. This is
//! the files-backend equivalent of `reflog delete --rewrite --updateref`:
//! <https://github.com/git/git/blob/v2.43.0/refs/files-backend.c>
//! Other storage backends fail before mutation; apply still uses ordinary Git.
use super::{Output, git_command, run};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

mod transaction;
use transaction::Transaction;

/// Explicit action on a captured stash identity.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Apply the immutable object without removing its saved entry.
    Apply,
    /// Apply the immutable object, then remove only its captured entry.
    Pop,
    /// Remove only the captured entry, without changing the working tree.
    Drop,
}

fn git(root: &Path, args: &[&str]) -> Result<Output> {
    let mut command = git_command(root);
    command.args(args);
    run(command, vec![], Duration::from_secs(120))
}
fn hook(root: &Path, phase: &str, update: &[u8]) -> Result<Output> {
    let mut command = git_command(root);
    command.args([
        "hook",
        "run",
        "--ignore-missing",
        "reference-transaction",
        "--",
        phase,
    ]);
    run(command, update.to_vec(), Duration::from_secs(120))
}
fn object(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
pub(super) fn execute(root: &Path, action: Action, reference: &str, oid: &str) -> Result<Output> {
    ensure!(object(oid), "Invalid stash object ID");
    let index: usize = reference
        .strip_prefix("stash@{")
        .and_then(|s| s.strip_suffix('}'))
        .filter(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()))
        .context("Invalid stash selector")?
        .parse()?;
    let mut output = Output::default();
    if !matches!(action, Action::Drop) {
        output = git(root, &["stash", "apply", oid])?;
        if !output.success() || matches!(action, Action::Apply) {
            return Ok(output);
        }
    }
    if let Err(error) = drop_entry(root, index, oid) {
        if matches!(action, Action::Pop) {
            output.code = 1;
            output.stderr.push_str(&format!(
                "Stash was applied, but removal did not complete; refresh the stash list before retrying: {error:#}\n"
            ));
            return Ok(output);
        }
        return Err(error);
    }
    output
        .stdout
        .push_str(&format!("Dropped {reference} ({oid})\n"));
    Ok(output)
}

fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    // A packed-only reference has no loose file yet. Git's prepared lock
    // supplies creation permissions instead of tempfile's private 0600.
    let metadata = fs::symlink_metadata(path).or_else(|error| {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error);
        }
        let name = path
            .file_name()
            .context("Missing Git filename")
            .map_err(std::io::Error::other)?;
        fs::symlink_metadata(path.with_file_name(format!("{}.lock", name.to_string_lossy())))
    })?;
    ensure!(metadata.is_file(), "Git metadata must be a regular file");
    let mut temporary = tempfile::Builder::new()
        .prefix(".rode-stash-")
        .tempfile_in(path.parent().context("Missing Git metadata directory")?)?;
    temporary
        .as_file()
        .set_permissions(metadata.permissions())?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}
fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(metadata.is_file(), "Git metadata must be a regular file");
    }
    crate::persistence::read(path, 8 * 1024 * 1024)
}
fn drop_entry(root: &Path, index: usize, oid: &str) -> Result<()> {
    ensure!(
        std::env::var_os("GIT_NAMESPACE").is_none_or(|v| v.is_empty()),
        "Namespaced stash removal is unsupported; use the Git terminal"
    );
    let storage = git(root, &["config", "--get", "extensions.refStorage"])?;
    ensure!(
        (storage.code == 1 && storage.stdout.is_empty())
            || (storage.success() && storage.stdout.trim() == "files"),
        "Safe stash removal requires Git's files reference backend; use the Git terminal for this repository"
    );
    let common = git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?
    .checked()?;
    let common = PathBuf::from(common.trim_end_matches('\n'));
    let reference = common.join("refs/stash");
    let log = common.join("logs/refs/stash");
    let before = read(&log)?.context("Stash history disappeared; refresh before removing it")?;
    let deleting = before.split_inclusive(|&b| b == b'\n').count() == 1;
    let tip = git(root, &["rev-parse", "--verify", "refs/stash"])?.checked()?;
    let tip = tip.trim();
    ensure!(object(tip), "Invalid stash reference");
    let transaction = Transaction::prepare(root, tip, deleting)
        .context("Stash list changed or Git metadata is busy; refresh before removing it")?;
    ensure!(
        !log.with_file_name("stash.lock").exists(),
        "Git metadata is busy; refresh and retry"
    );
    if let Some(loose) = read(&reference)? {
        ensure!(
            std::str::from_utf8(&loose)?.trim() == tip,
            "Symbolic or changed stash reference; refresh before removing it"
        );
    }
    let bytes = read(&log)?.context("Stash history disappeared; refresh before removing it")?;
    let mut lines: Vec<_> = bytes
        .split_inclusive(|&b| b == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    let width = oid.len();
    for line in &lines {
        ensure!(
            line.len() > 2 * width + 2
                && line[width] == b' '
                && line[2 * width + 1] == b' '
                && line.ends_with(b"\n")
                && line[..width].iter().all(u8::is_ascii_hexdigit)
                && line[width + 1..2 * width + 1]
                    .iter()
                    .all(u8::is_ascii_hexdigit),
            "Invalid stash reflog; no entry was removed"
        );
    }
    let selected = index
        .checked_add(1)
        .and_then(|offset| lines.len().checked_sub(offset))
        .context("Stash list changed; refresh before removing it")?;
    ensure!(
        &lines[selected][width + 1..2 * width + 1] == oid.as_bytes()
            && &lines.last().context("Stash list is empty")?[width + 1..2 * width + 1]
                == tip.as_bytes(),
        "Stash list changed; refresh before removing it"
    );
    lines.remove(selected);
    let mut previous = vec![b'0'; width];
    let mut rewritten = Vec::new();
    for line in &mut lines {
        line[..width].copy_from_slice(&previous);
        previous.copy_from_slice(&line[width + 1..2 * width + 1]);
        rewritten.extend_from_slice(line);
    }
    ensure!(
        lines.is_empty() == deleting,
        "Stash list changed; refresh before removing it"
    );
    if deleting {
        // Git owns reference/packed-reference deletion and lock cleanup. Hooks
        // run only after the selected reflog entry was checked under that lease.
        let update = format!("{tip} {} refs/stash\n", "0".repeat(width));
        let prepared = hook(root, "prepared", update.as_bytes()).and_then(Output::checked);
        if let Err(error) = prepared {
            let _ = hook(root, "aborted", update.as_bytes());
            return Err(error).context("Git reference-transaction hook rejected stash removal");
        }
        if let Err(error) = transaction.finish(true) {
            let _ = hook(root, "aborted", update.as_bytes());
            return Err(error);
        }
        let _ = hook(root, "committed", update.as_bytes());
    } else {
        replace(&log, &rewritten)?;
        previous.push(b'\n');
        replace(&reference, &previous)?;
        // The verification transaction is a lease; publication above already
        // replaced the reflog and loose tip without manufacturing a log entry.
        transaction.finish(false)?;
    }
    Ok(())
}

//! Explicit folder exports and SSHFS mounts. The SFTP server holds one directory
//! capability; transport authentication never grants it the rest of the host.
//! Mount helpers own foreground SSHFS children and clean up on stream EOF.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

mod mount;
mod sftp;
pub use mount::{Mount, unmount};

/// Which machine supplies the files; the opposite machine receives the mount.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Make desktop files accessible to programs on the SSH host.
    #[default]
    LocalToRemote,
    /// Make project-host files accessible to desktop programs.
    RemoteToLocal,
}

/// Portable configuration only: no live sockets, mount IDs or credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Host supplying the shared directory.
    pub direction: Direction,
    /// Absolute or home-relative source on the supplying host.
    pub source: String,
    /// Existing empty directory on the receiving host.
    pub destination: String,
    /// Enforced by the exporting server as well as by the mount.
    pub read_only: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            direction: Direction::default(),
            source: String::new(),
            destination: String::new(),
            read_only: true,
        }
    }
}
impl Config {
    /// Check saved/user input without reading either host's filesystem.
    pub fn validate(&self) -> Result<()> {
        for path in [&self.source, &self.destination] {
            ensure!(
                !path.is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control),
                "Choose source and mount folders without control characters"
            );
            ensure!(
                path.starts_with('/') || path.starts_with("~/"),
                "Shared folders require absolute or home-relative paths"
            );
        }
        ensure!(
            self.source != "/"
                && self.source != "~/"
                && self.destination != "/"
                && self.destination != "~/",
            "Select a specific subfolder, not a whole filesystem or home directory"
        );
        Ok(())
    }
}

/// Bounded header preceding the binary SFTP stream on an export channel.
#[derive(Serialize, Deserialize)]
pub struct Export {
    /// Selected source interpreted only on this host.
    pub root: String,
    /// Mutating SFTP operations are refused when true.
    pub read_only: bool,
    /// Captured project policy for remote exports; local exports use a directory capability.
    pub plan: Option<crate::sandbox::Plan>,
}

/// Read exactly one bounded header, without consuming any following SFTP bytes.
pub fn header<T: serde::de::DeserializeOwned>(input: &mut impl Read) -> Result<T> {
    let mut bytes = Vec::new();
    for _ in 0..128 * 1024 {
        let mut byte = [0];
        input
            .read_exact(&mut byte)
            .context("Shared-folder header ended early")?;
        if byte[0] == b'\n' {
            return Ok(serde_json::from_slice(&bytes)?);
        }
        bytes.push(byte[0]);
    }
    anyhow::bail!("Shared-folder header exceeds its size limit")
}

/// Write one configuration header before handing the pipe to SFTP.
pub fn write_header(output: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() < 128 * 1024,
        "Shared-folder header exceeds its size limit"
    );
    output.write_all(&bytes)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

// Stdin's standard-library wrapper is buffered. Export launch hands the raw
// descriptor to a policy worker, so a buffered reader could steal an INIT sent
// in the same write as the JSON header. Read a duplicate descriptor directly.
fn stdin_header<T: serde::de::DeserializeOwned>() -> Result<T> {
    use std::os::fd::FromRawFd;
    let fd = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
    ensure!(
        fd >= 0,
        "Cannot duplicate shared-folder input: {}",
        std::io::Error::last_os_error()
    );
    let mut input = unsafe { std::fs::File::from_raw_fd(fd) };
    header(&mut input)
}

/// Early helper dispatch shared by the desktop and installed headless binary.
pub fn cli(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("--shared-folder-export") if args.len() == 1 => {
            let mut export: Export = stdin_header()?;
            if let Some(mut plan) = export.plan.take() {
                ensure!(
                    std::path::Path::new(&plan.root).is_absolute(),
                    "Invalid shared-folder project policy"
                );
                ensure!(
                    !plan.active
                        || plan.prefix.len() >= 2
                            && plan.prefix.last().is_some_and(|arg| arg == "--"),
                    "Invalid shared-folder sandbox plan"
                );
                plan.environment.validate()?;
                plan.native_helper = Some(std::env::current_exe()?.to_string_lossy().into());
                plan.environment.mise = false;
                let (mut command, _lease) =
                    plan.coordinator_command("--shared-folder-export-worker")?;
                command.arg(serde_json::to_string(&export)?);
                let status = command.status()?;
                ensure!(
                    status.success(),
                    "Shared-folder server exited with {status}"
                );
                Ok(())
            } else {
                sftp::serve(
                    &export,
                    &mut std::io::stdin().lock(),
                    &mut std::io::stdout().lock(),
                )
            }
        }
        Some("--shared-folder-export-worker") if args.len() == 2 => {
            let export = serde_json::from_str(&args[1])?;
            sftp::serve(
                &export,
                &mut std::io::stdin().lock(),
                &mut std::io::stdout().lock(),
            )
        }
        Some("--shared-folder-mount") if args.len() == 1 => mount::run(stdin_header()?),
        Some("--shared-folder-unmount") if args.len() == 1 => {
            let mount = stdin_header()?;
            unmount(&mount, false)
        }
        _ => anyhow::bail!("Invalid shared-folder helper invocation"),
    }
}

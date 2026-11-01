//! OpenSSH configuration, one-window ControlMaster ownership and bounded channels.
//!
//! The interactive master alone handles authentication and host verification.
//! File/tool channels reuse its private socket and cannot initiate a replacement
//! login after it disappears. Remote paths and JSON payloads are data; quote only
//! the POSIX shell command boundary and keep literal SSH options separate.
//!
//! # Design decisions
//!
//! System OpenSSH remains the connection authority so host aliases, keys, jump
//! hosts, agents and interactive authentication use the user's existing setup.
//! A separate master lets file/tool channels share that authenticated identity
//! without tying it to the lifetime of a project shell or collecting credentials
//! in the editor. Small per-request Python peers trade duplicated service code
//! for avoiding a remote editor installation or a new network listener.
//!
//! Slave channels disable fresh-login fallback. Otherwise a master disappearing
//! between the liveness check and spawn could unexpectedly reauthenticate or
//! reinterpret connection configuration. Connection loss therefore becomes an
//! explicit workspace transition with recoverable, unbound buffers.

use crate::files::{self, Request, Response};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
pub(crate) mod python;

static CONNECTION_SERIAL: AtomicU64 = AtomicU64::new(0);

/// User-managed OpenSSH settings, without passwords, private-key contents or cached
/// credentials.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Profile {
    /// Optional friendly profile name.
    pub name: String,
    /// OpenSSH host alias or destination host.
    pub host: String,
    /// Optional username override; empty defers to OpenSSH configuration.
    pub user: String,
    /// Optional port override stored as text for the settings form.
    pub port: String,
    /// Optional identity-file path on the desktop.
    pub identity: String,
    /// Optional ProxyJump specification interpreted by OpenSSH.
    pub jump: String,
    /// Optional alternative SSH config file on the desktop.
    pub config: String,
    /// Initial project directory on the remote host.
    pub root: String,
    /// Requested sandbox defaults for this profile.
    pub sandbox: crate::sandbox::Options,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            user: String::new(),
            port: String::new(),
            identity: String::new(),
            jump: String::new(),
            config: String::new(),
            root: "~".into(),
            sandbox: Default::default(),
        }
    }
}

impl Profile {
    /// Validate input bounds and SSH option values before constructing a command.
    pub fn validate(&self) -> Result<()> {
        self.sandbox.validate()?;
        if self.host.trim().is_empty() {
            bail!("Enter an SSH alias or hostname.");
        }
        for (label, value) in [
            ("Host", &self.host),
            ("Username", &self.user),
            ("Jump host", &self.jump),
        ] {
            if value.starts_with('-') || value.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                bail!("{label} cannot start with '-' or contain whitespace.");
            }
        }
        if !self.port.is_empty() && self.port.parse::<u16>().ok().filter(|p| *p > 0).is_none() {
            bail!("Port must be between 1 and 65535.");
        }
        if self.root.is_empty() || self.root.contains('\0') {
            bail!("Enter a valid remote folder.");
        }
        Ok(())
    }

    /// Friendly name, falling back to the connection endpoint.
    pub fn label(&self) -> String {
        if !self.name.is_empty() {
            self.name.clone()
        } else if !self.user.is_empty() {
            format!("{}@{}", self.user, self.host)
        } else {
            self.host.clone()
        }
    }

    /// Display the host with any explicit username and port.
    pub fn endpoint(&self) -> String {
        if self.user.is_empty() || self.host.contains('@') {
            self.host.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        }
    }

    /// Build literal OpenSSH connection options; never interpolate them as shell code.
    pub fn options(&self) -> Vec<String> {
        let mut args = vec![
            "-o".into(),
            "ConnectTimeout=15".into(),
            "-o".into(),
            "ServerAliveInterval=15".into(),
            "-o".into(),
            "ServerAliveCountMax=3".into(),
        ];
        if !self.config.is_empty() {
            args.extend([
                "-F".into(),
                files::expand_home(&self.config).to_string_lossy().into(),
            ]);
        }
        for (flag, value) in [("-l", &self.user), ("-p", &self.port), ("-J", &self.jump)] {
            if !value.is_empty() {
                args.extend([flag.into(), value.clone()]);
            }
        }
        if !self.identity.is_empty() {
            args.extend([
                "-i".into(),
                files::expand_home(&self.identity).to_string_lossy().into(),
            ]);
        }
        args
    }
}

/// One window's authenticated connection identity and private ControlMaster socket. Clones
/// share that identity; the owning workspace controls cleanup.
#[derive(Clone)]
pub struct Remote {
    /// Connection settings used to launch the authentication master.
    pub profile: Profile,
    /// Private Unix socket path for all multiplexed channels.
    pub socket: PathBuf,
}

/// Quote one argument for a POSIX remote shell, including embedded single quotes.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Build a login-shell command rooted at the supplied remote directory.
pub fn remote_shell(root: &str) -> String {
    let root = if root == "~" {
        "\"$HOME\"".into()
    } else if let Some(rest) = root.strip_prefix("~/") {
        format!("\"$HOME\"/{}", quote(rest))
    } else {
        quote(root)
    };
    format!("cd -- {root} && exec \"${{SHELL:-/bin/sh}}\" -l")
}

impl Remote {
    /// Allocate a private connection runtime after validating the profile; authentication
    /// starts separately.
    pub fn new(profile: Profile) -> Result<Self> {
        profile.validate()?;
        #[cfg(not(unix))]
        bail!("Remote workspaces currently require Unix OpenSSH ControlMaster (Linux or macOS).");
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let directory = std::env::temp_dir().join(format!(
                "rode-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos(),
                CONNECTION_SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700).create(&directory)?;
            Ok(Self {
                profile,
                socket: directory.join("ssh"),
            })
        }
    }

    /// Arguments for the interactive authentication master, retaining normal host-key checks.
    pub fn master_args(&self) -> Vec<String> {
        let mut args = self.profile.options();
        args.extend([
            "-M".into(),
            "-S".into(),
            self.socket.to_string_lossy().into(),
            "-o".into(),
            "ControlPersist=no".into(),
            "-N".into(),
            "--".into(),
            self.profile.host.clone(),
        ]);
        args
    }

    /// Ask OpenSSH whether this private master is alive; this is a blocking control operation.
    pub fn connected(&self) -> bool {
        self.socket.exists()
            && Command::new("ssh")
                .args([
                    "-F",
                    "/dev/null",
                    "-S",
                    &self.socket.to_string_lossy(),
                    "-O",
                    "check",
                    "--",
                    &self.profile.host,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    }

    /// Execute a blocking unsandboxed file request over the existing master. Use
    /// [`Self::request_in`] for project-policy operations.
    pub fn request(&self, request: &Request) -> Result<Response> {
        self.request_in(request, None)
    }

    /// Build multiplex-only channel arguments that cannot fall back to a fresh authentication
    /// flow.
    pub fn channel_args(&self, tty: bool) -> Vec<String> {
        vec![
            "-F".into(),
            "/dev/null".into(),
            "-S".into(),
            self.socket.to_string_lossy().into(),
            "-o".into(),
            "ControlMaster=no".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ProxyCommand=false".into(),
            if tty { "-tt".into() } else { "-T".into() },
            "--".into(),
            self.profile.host.clone(),
        ]
    }

    /// Build a quoted project login-shell command, retaining the actual sandbox prefix.
    pub fn shell_command(&self, plan: &crate::sandbox::Plan) -> String {
        let prefix = if plan.active {
            plan.prefix
                .iter()
                .map(|arg| quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
                + " "
        } else {
            String::new()
        };
        format!(
            "sh -c {}",
            quote(&format!(
                "cd -- {} && exec {prefix}\"${{SHELL:-/bin/sh}}\" -l",
                quote(&plan.root)
            ))
        )
    }

    /// Execute bounded JSON file I/O through the authenticated master and optional project
    /// plan. A lost master fails visibly; this never reconnects or retries outside an active
    /// sandbox.
    pub fn request_in(
        &self,
        request: &Request,
        plan: Option<&crate::sandbox::Plan>,
    ) -> Result<Response> {
        // Refuse to start a second authentication flow after the master disconnects.
        if !self.connected() {
            bail!("SSH is disconnected. Reconnect to continue; your unsaved edits are preserved.");
        }
        let mut command = Command::new("ssh");
        // The master already applied all connection settings. Slave channels
        // must use only its socket: re-reading ProxyJump could otherwise start
        // a fresh login if the socket disappears between the check and spawn.
        command.args(self.channel_args(false));
        let prefix = plan
            .filter(|p| p.active)
            .map(|p| {
                p.prefix
                    .iter()
                    .map(|arg| quote(arg))
                    .collect::<Vec<_>>()
                    .join(" ")
                    + " "
            })
            .unwrap_or_default();
        command.arg(format!("{prefix}{}", python::HOST.command()));
        let output = run_bounded(
            command,
            python::HOST.input(serde_json::to_vec(request)?)?,
            Duration::from_secs(if matches!(request, Request::Tool { .. }) {
                150
            } else {
                45
            }),
        )?;
        serde_json::from_slice(&output).context("Invalid response from remote host. Check that python3 is available and non-interactive shell startup prints no banners.")
    }

    /// Send a cancellable adapter request over this existing SSH master, applying the
    /// captured active host plan and refusing fresh-login or desktop execution fallback.
    pub(crate) fn extension_request(
        &self,
        request: &crate::project::Request,
        plan: Option<&crate::sandbox::Plan>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<crate::extensions::Reply> {
        anyhow::ensure!(
            self.connected(),
            "SSH is disconnected; reconnect to run an extension."
        );
        let mut command = Command::new("ssh");
        command.args(self.channel_args(false));
        let prefix = plan
            .filter(|p| p.active)
            .map(|p| {
                p.prefix
                    .iter()
                    .map(|a| quote(a))
                    .collect::<Vec<_>>()
                    .join(" ")
                    + " "
            })
            .unwrap_or_default();
        command.arg(format!(
            "{prefix}{} --extension-helper",
            python::HOST.command()
        ));
        crate::extensions::execute_with_prelude(
            command,
            request,
            cancel,
            python::HOST.input(vec![])?,
        )
    }
    /// Close only this connection's master and remove its private socket directory.
    pub fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.socket);
        if let Some(parent) = self.socket.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
    /// Run a cancellable LaTeX coordinator over the existing master and actual project policy.
    pub fn latex_request(
        &self,
        invocation: &crate::latex::Invocation,
        plan: &crate::sandbox::Plan,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<crate::latex::Reply> {
        anyhow::ensure!(
            self.connected(),
            "SSH is disconnected; reconnect to build LaTeX."
        );
        let mut command = Command::new("ssh");
        command.args(self.channel_args(false));
        let prefix = if plan.active {
            plan.prefix
                .iter()
                .map(|arg| quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
                + " "
        } else {
            String::new()
        };
        command.arg(format!("{prefix}{} --latex-helper", python::HOST.command()));
        crate::latex::execute_with_prelude(command, invocation, cancel, python::HOST.input(vec![])?)
    }
    /// Run a cancellable Marp coordinator over the existing master and actual project policy.
    pub fn marp_request(
        &self,
        invocation: &crate::marp::Invocation,
        plan: Option<&crate::sandbox::Plan>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<crate::marp::Reply> {
        anyhow::ensure!(
            self.connected(),
            "SSH is disconnected; reconnect to render Marp slides."
        );
        let mut command = Command::new("ssh");
        command.args(self.channel_args(false));
        let prefix = plan
            .filter(|p| p.active)
            .map(|p| {
                p.prefix
                    .iter()
                    .map(|arg| quote(arg))
                    .collect::<Vec<_>>()
                    .join(" ")
                    + " "
            })
            .unwrap_or_default();
        command.arg(format!("{prefix}{} --marp-helper", python::HOST.command()));
        crate::marp::execute_with_prelude(command, invocation, cancel, python::HOST.input(vec![])?)
    }
}

/// Concurrently drain both pipes and stdin so large saves cannot deadlock SSH.
pub(crate) fn run_bounded(
    mut command: Command,
    input: Vec<u8>,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not run OpenSSH; install the ssh client")?;
    let mut stdin = child.stdin.take().context("Missing SSH stdin")?;
    let stdout = child.stdout.take().context("Missing SSH stdout")?;
    let stderr = child.stderr.take().context("Missing SSH stderr")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out_tx = tx.clone();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes);
        let _ = out_tx.send((true, bytes, result));
    });
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stderr.take(64 * 1024 + 1).read_to_end(&mut bytes);
        let _ = tx.send((false, bytes, result));
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "SSH operation timed out. The remote host may be unreachable; your edits are still open."
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut output = Vec::new();
    let mut error = Vec::new();
    for _ in 0..2 {
        let (is_out, bytes, result) = rx
            .recv_timeout(Duration::from_secs(2))
            .context("SSH output exceeded its limit or did not close")?;
        result?;
        if is_out {
            output = bytes;
        } else {
            error = bytes;
        }
    }
    if !status.success() {
        bail!("{}", String::from_utf8_lossy(&error).trim());
    }
    if output.len() > 64 * 1024 * 1024 {
        bail!("SSH response exceeded its size limit.");
    }
    Ok(output)
}

/// Discover bounded host aliases from the default OpenSSH configuration.
pub fn aliases() -> Vec<String> {
    let mut aliases = Vec::new();
    if let Ok(config) = std::fs::read_to_string(files::expand_home("~/.ssh/config")) {
        for line in config.lines() {
            let mut words = line.split('#').next().unwrap_or("").split_whitespace();
            if words
                .next()
                .is_some_and(|key| key.eq_ignore_ascii_case("host"))
            {
                aliases.extend(
                    words
                        .filter(|s| !s.contains(['*', '?', '!']))
                        .map(str::to_owned),
                );
            }
        }
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

/// Return the desktop path used for saved RODE connection profiles.
pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| files::expand_home("~/.config"));
    base.join("rode").join("connections.json")
}

/// Load saved profiles, returning an empty list on failure; use the checked form when reporting
/// errors.
pub fn load_profiles() -> Vec<Profile> {
    load_profiles_checked().unwrap_or_default()
}

/// Load and validate saved profiles, preserving malformed storage on error.
pub fn load_profiles_checked() -> Result<Vec<Profile>> {
    profiles_at(&config_path())
}

fn profiles_at(path: &Path) -> Result<Vec<Profile>> {
    let profiles: Vec<Profile> = crate::persistence::read(path, 1024 * 1024)?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()
        .context("Invalid connection profiles; existing file was preserved")?
        .unwrap_or_default();
    anyhow::ensure!(
        profiles.len() <= 128,
        "At most 128 connection profiles are supported."
    );
    Ok(profiles)
}

/// Merge an explicit profile edit into the latest shared profile file under a lock.
pub fn save_profile(profile: &Profile, previous: Option<&Profile>) -> Result<Vec<Profile>> {
    save_profile_at(&config_path(), profile, previous)
}

/// Merge a profile edit at an explicit path; `previous` identifies the original record for
/// renames/concurrent field merges.
pub fn save_profile_at(
    path: &Path,
    profile: &Profile,
    previous: Option<&Profile>,
) -> Result<Vec<Profile>> {
    profile.validate()?;
    let path = path.canonicalize().unwrap_or_else(|_| path.into());
    crate::persistence::transaction(&path, |existed| {
        let mut raw: serde_json::Value = crate::persistence::read(&path, 1024 * 1024)?
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()
            .context("Invalid connection profiles; existing file was preserved")?
            .unwrap_or(serde_json::json!([]));
        let profiles: Vec<Profile> = serde_json::from_value(raw.clone())
            .context("Invalid connection profiles; existing file was preserved")?;
        let label = profile.label();
        let before = previous
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or(serde_json::to_value(Profile::default())?);
        let changes = crate::persistence::changes(&before, &serde_json::to_value(profile)?);
        let entries = raw
            .as_array_mut()
            .context("Invalid connection profile list")?;
        if let Some(index) = profiles.iter().position(|p| p.label() == label) {
            crate::persistence::apply(&mut entries[index], &changes);
        } else {
            entries.push(serde_json::to_value(profile)?);
        }
        anyhow::ensure!(
            entries.len() <= 128,
            "At most 128 connection profiles are supported."
        );
        let profiles = serde_json::from_value(raw.clone())?;
        let bytes = serde_json::to_vec_pretty(&raw)?;
        anyhow::ensure!(
            bytes.len() <= 1024 * 1024,
            "Connection profiles exceed 1 MiB."
        );
        crate::persistence::replace(&path, &bytes, existed)?;
        Ok(profiles)
    })
}

/// Return the final path component for UI labels; no filesystem access or canonicalization.
pub fn display_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_connections_own_distinct_private_directories() {
        let barrier = std::sync::Barrier::new(32);
        let connections = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..32)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        Remote::new(Profile {
                            host: "rode-fixture".into(),
                            ..Default::default()
                        })
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap().unwrap())
                .collect::<Vec<_>>()
        });
        let names: std::collections::HashSet<_> = connections
            .iter()
            .map(|remote| remote.socket.clone())
            .collect();
        for remote in connections {
            remote.cleanup();
        }
        assert_eq!(names.len(), 32);
    }
}

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
//! in the editor. The native backend deploys a verified headless helper sharing
//! local Rust services. It installs no remote GUI and opens no application listener.
//!
//! Slave channels disable fresh-login fallback. Otherwise a master disappearing
//! between the liveness check and spawn could unexpectedly reauthenticate or
//! reinterpret connection configuration. Connection loss therefore becomes an
//! explicit workspace transition with recoverable, unbound buffers.

use crate::files::{self, Request, Response};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

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
    /// Optional preinstalled helper on the remote host; empty enables discovery/deployment.
    pub remote_helper: String,
    /// Request X11 forwarding when the desktop has a display. OpenSSH still owns
    /// authentication, trust/timeout policy and server capability negotiation.
    pub x11_forwarding: bool,
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
            remote_helper: String::new(),
            x11_forwarding: true,
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
        if !self.remote_helper.is_empty()
            && (!(self.remote_helper.starts_with('/') || self.remote_helper.starts_with("~/"))
                || self.remote_helper.len() > 4096
                || self.remote_helper.chars().any(char::is_control))
        {
            bail!(
                "Remote helper must be an absolute path or start with ~/, with no control characters (maximum 4096 bytes)."
            );
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
    pub(crate) native: std::sync::Arc<crate::host::native::State>,
}

/// Quote one argument for a POSIX remote shell, including embedded single quotes.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

impl Remote {
    /// Allocate a private connection runtime after validating the profile; authentication
    /// starts separately. Unknown fields from older saved profiles are ignored.
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
                native: Default::default(),
            })
        }
    }

    /// Arguments for the interactive authentication master, retaining normal host-key checks.
    pub fn master_args(&self) -> Vec<String> {
        let mut args = self.profile.options();
        args.push(self.x11_flag().into());
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
    /// flow. Short-lived work channels do not request a display: their X11
    /// listener would disappear as soon as the command returns.
    pub fn channel_args(&self, tty: bool) -> Vec<String> {
        self.channel_args_with_x11(tty, false)
    }

    /// Request a display on a long-lived shell or native gateway channel. The
    /// gateway's environment is captured for new native terminals, and its SSH
    /// channel keeps their display listener alive across view detach/reattach.
    /// No display or a server refusing X11 must not prevent normal shell use.
    pub fn display_channel_args(&self, tty: bool) -> Vec<String> {
        self.channel_args_with_x11(tty, self.profile.x11_forwarding)
    }

    fn x11_flag(&self) -> &'static str {
        if self.profile.x11_forwarding {
            "-X"
        } else {
            "-x"
        }
    }

    fn channel_args_with_x11(&self, tty: bool, x11: bool) -> Vec<String> {
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
            // Each multiplexed session must request forwarding separately. The
            // master grants it only when its own ForwardAgent policy allows it.
            "-A".into(),
            if x11 { "-X".into() } else { "-x".into() },
            if tty { "-tt".into() } else { "-T".into() },
            "--".into(),
            self.profile.host.clone(),
        ]
    }

    /// Build a quoted project login-shell command, retaining the actual sandbox prefix.
    pub fn shell_command(&self, plan: &crate::sandbox::Plan) -> String {
        let arguments = match plan.execution_prefix("") {
            Ok(arguments) => arguments,
            Err(error) => {
                return format!(
                    "printf '%s\\n' {} >&2; exit 1",
                    quote(&format!("Cannot prepare project environment: {error:#}"))
                );
            }
        };
        let prefix = arguments
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
            + " ";
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
        self.native.client(self)?.request(request, plan)
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
            .map(|p| {
                Ok::<_, anyhow::Error>(
                    p.execution_prefix("")?
                        .iter()
                        .map(|a| quote(a))
                        .collect::<Vec<_>>()
                        .join(" ")
                        + " ",
                )
            })
            .transpose()?
            .unwrap_or_default();
        let helper = quote(&self.native.client(self)?.executable);
        command.arg(format!("{prefix}{helper} --extension-helper"));
        crate::extensions::execute_with_prelude(command, request, cancel, vec![])
    }
    /// Close only this connection's master and remove its private socket directory.
    pub fn cleanup(&self) {
        self.native.close();
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
        let prefix = plan
            .execution_prefix("")?
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
            + " ";
        let helper = quote(&self.native.client(self)?.executable);
        command.arg(format!("{prefix}{helper} --latex-helper"));
        crate::latex::execute_with_prelude(command, invocation, cancel, vec![])
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
            .map(|p| p.execution_prefix(""))
            .transpose()?
            .unwrap_or_default()
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
            + " ";
        let helper = quote(&self.native.client(self)?.executable);
        command.arg(format!("{prefix}{helper} --marp-helper"));
        crate::marp::execute_with_prelude(command, invocation, cancel, vec![])
    }
}

pub(crate) use rode_host_core::command::run_bounded;

/// Discover bounded host aliases from the default OpenSSH configuration.
pub fn aliases() -> Vec<String> {
    let mut aliases = Vec::new();
    if let Ok(Some(bytes)) = crate::persistence::read(
        &files::expand_home("~/.ssh/config"),
        crate::persistence::LIMIT,
    ) && let Ok(config) = std::str::from_utf8(&bytes)
    {
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
    fn saved_compatibility_profiles_keep_connection_settings() {
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "host": "cluster", "jump": "login", "root": "/work/project",
            "backend": "legacy_python"
        }))
        .unwrap();
        assert_eq!(profile.host, "cluster");
        assert_eq!(profile.jump, "login");
        assert_eq!(profile.root, "/work/project");
        assert!(profile.remote_helper.is_empty());
        assert!(profile.x11_forwarding);
        assert!(
            serde_json::to_value(profile)
                .unwrap()
                .get("backend")
                .is_none()
        );
    }
    #[test]
    fn display_channels_keep_multiplex_only_policy_and_profile_preference() {
        for enabled in [false, true] {
            let profile = Profile {
                host: "cluster".into(),
                x11_forwarding: enabled,
                ..Default::default()
            };
            let saved: Profile =
                serde_json::from_value(serde_json::to_value(&profile).unwrap()).unwrap();
            assert_eq!(saved.x11_forwarding, enabled);
            let remote = Remote::new(saved).unwrap();
            let flag = if enabled { "-X" } else { "-x" };
            assert!(remote.master_args().iter().any(|arg| arg == flag));
            for tty in [false, true] {
                let args = remote.display_channel_args(tty);
                assert!(args.iter().any(|arg| arg == flag));
                assert!(args.iter().any(|arg| arg == "ProxyCommand=false"));
                assert!(args.iter().any(|arg| arg == "BatchMode=yes"));
                assert!(!args.iter().any(|arg| arg == "-Y"));
                let work = remote.channel_args(tty);
                assert!(work.iter().any(|arg| arg == "-x"));
                assert!(!work.iter().any(|arg| arg == "-X"));
            }
            remote.cleanup();
        }
    }
    #[test]
    fn remote_helper_paths_are_literal_bounded_and_saved() {
        let root = tempfile::tempdir().unwrap();
        let mut profile = Profile {
            host: "cluster".into(),
            ..Default::default()
        };
        for path in [
            "",
            "/opt/rode ' build/rode-sessiond",
            "~/.cargo/bin/rode-sessiond",
        ] {
            profile.remote_helper = path.into();
            assert!(profile.validate().is_ok());
            let saved =
                save_profile_at(&root.path().join("profiles.json"), &profile, None).unwrap();
            assert_eq!(saved[0].remote_helper, path);
        }
        for path in [
            "rode-sessiond",
            "../helper",
            "~someone/helper",
            "/tmp/helper\nother",
            "/tmp/\0helper",
        ] {
            profile.remote_helper = path.into();
            assert!(profile.validate().is_err(), "Accepted {path:?}");
        }
        profile.remote_helper = format!("/{}", "x".repeat(4096));
        assert!(profile.validate().is_err());
    }
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

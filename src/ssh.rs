//! Configurable Rust/OpenSSH clients, connection ownership and bounded channels.
//!
//! The interactive master alone handles authentication and host verification.
//! File/tool channels reuse its private socket and cannot initiate a replacement
//! login after it disappears. Remote paths and JSON payloads are data; quote only
//! the POSIX shell command boundary and keep literal SSH options separate.
//!
//! # Design decisions
//!
//! The embedded russh client is the default; OpenSSH remains an explicit option
//! for configurations requiring its complete compatibility surface.
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

#[cfg(unix)]
mod native;

/// SSH client implementation. Native connections do not execute a system SSH client.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Embedded Rust SSH client, sharing one authenticated connection.
    #[default]
    Russh,
    /// Explicit compatibility mode using the installed OpenSSH client.
    #[serde(rename = "openssh", alias = "open_ssh")]
    OpenSsh,
}

/// Dispatch internal Rust SSH subprocesses before desktop initialization.
pub fn client_cli(args: &[String]) -> Result<i32> {
    #[cfg(unix)]
    {
        native::cli(args)
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        bail!("RODE SSH clients currently require Unix")
    }
}

static CONNECTION_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Alternate network route to a compute host, retaining its original SSH identity.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelRoute {
    /// Loopback port on the last ProxyJump host.
    pub port: u16,
    /// Original known-hosts token, including a nonstandard SSH port if applicable.
    pub host_key_alias: String,
}

/// User-managed OpenSSH settings, without passwords, private-key contents or cached
/// credentials.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Profile {
    /// Client transport; omitted fields in older profiles select Rust.
    pub backend: Backend,
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
    /// Optional comma-separated ProxyJump chain.
    pub jump: String,
    /// Optional daemon-owned reverse tunnel reached through the configured jump host.
    pub tunnel: Option<Box<TunnelRoute>>,
    /// Optional alternative SSH config file on the desktop.
    pub config: String,
    /// Optional preinstalled helper on the remote host; empty enables discovery/deployment.
    pub remote_helper: String,
    /// Request X11 forwarding when the desktop has a display, using configured
    /// trust/timeout policy and server capability negotiation.
    pub x11_forwarding: bool,
    /// Initial project directory on the remote host.
    pub root: String,
    /// Requested sandbox defaults for this profile.
    pub sandbox: crate::sandbox::Options,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            backend: Backend::default(),
            name: String::new(),
            host: String::new(),
            user: String::new(),
            port: String::new(),
            identity: String::new(),
            jump: String::new(),
            tunnel: None,
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
        if let Some(route) = &self.tunnel {
            anyhow::ensure!(
                route.port > 0 && !self.jump.is_empty() && self.jump != "none",
                "A tunnel route requires a relay jump host and a nonzero port"
            );
            anyhow::ensure!(
                !route.host_key_alias.is_empty()
                    && route.host_key_alias.len() <= 1024
                    && !route
                        .host_key_alias
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control()),
                "Invalid compute host key alias"
            );
        }
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
        if let Some(route) = &self.tunnel {
            args.extend([
                "-o".into(),
                "HostName=127.0.0.1".into(),
                "-o".into(),
                format!("HostKeyAlias={}", route.host_key_alias),
                "-p".into(),
                route.port.to_string(),
            ]);
        }
        for (flag, value) in [("-l", &self.user), ("-p", &self.port), ("-J", &self.jump)] {
            if flag == "-p" && self.tunnel.is_some() {
                continue;
            }
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

    /// Construct a return route without changing the compute host's authentication settings.
    /// Reads SSH configuration; call on a worker, not the UI thread.
    #[cfg(unix)]
    pub fn through_tunnel(&self, jump: String, port: u16) -> Result<Self> {
        let alias = match &self.tunnel {
            Some(route) => route.host_key_alias.clone(),
            None => native::host_token(self)?,
        };
        let mut profile = self.clone();
        profile.jump = jump;
        profile.tunnel = Some(Box::new(TunnelRoute {
            port,
            host_key_alias: alias,
        }));
        profile.name = format!("{} via tunnel", self.label());
        profile.validate()?;
        Ok(profile)
    }
}

/// One window's authenticated connection identity and private control socket. Clones
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
    /// Executable for connection and channel subprocesses, without PATH lookup in Rust mode.
    pub fn client_program(&self) -> PathBuf {
        if self.profile.backend == Backend::OpenSsh {
            return "ssh".into();
        }
        if let Some(executable) = std::env::var_os("RODE_TEST_SSH_CLIENT") {
            return executable.into();
        }
        let executable =
            rode_session::current_executable().unwrap_or_else(|_| PathBuf::from("/proc/self/exe"));
        // Cargo test harnesses cannot dispatch internal CLI flags. Cargo builds the
        // actual desktop alongside integration tests; never search PATH for it.
        if let Some(directory) = executable.parent()
            && directory.file_name().is_some_and(|name| name == "deps")
            && executable
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.rsplit_once('-'))
                .is_some_and(|(_, hash)| {
                    hash.len() == 16 && hash.bytes().all(|c| c.is_ascii_hexdigit())
                })
            && let Some(parent) = directory.parent()
        {
            return parent.join("rode");
        }
        executable
    }

    /// Start a noninteractive channel on this connection, never a new login.
    pub fn command(&self) -> Command {
        let mut command = Command::new(self.client_program());
        command.args(self.channel_args(false));
        command
    }
    /// Allocate a private connection runtime after validating the profile; authentication
    /// starts separately.
    pub fn new(profile: Profile) -> Result<Self> {
        profile.validate()?;
        #[cfg(not(unix))]
        bail!("Remote workspaces currently require Unix; Linux is the validated platform.");
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

    /// Prepare private launch data and arguments for the interactive authentication master.
    pub fn master_args(&self) -> Result<Vec<String>> {
        if self.profile.backend == Backend::Russh {
            use sha2::{Digest, Sha256};
            use std::io::Write;
            let directory = self
                .socket
                .parent()
                .context("Missing SSH runtime directory")?;
            let bytes = serde_json::to_vec(&self.profile)?;
            anyhow::ensure!(bytes.len() <= 128 * 1024, "SSH profile exceeds limit");
            // The login PTY must remain available for authentication. Hand over
            // its configuration in the existing private runtime directory instead.
            let mut file = tempfile::NamedTempFile::new_in(directory)?;
            file.write_all(&bytes)?;
            file.persist(directory.join("profile.json"))?;
            return Ok(vec![
                "--ssh-client-master".into(),
                self.socket.to_string_lossy().into(),
                // Bind the handoff to this snapshot even if a same-UID sandbox
                // was explicitly granted write access to the runtime directory.
                format!("{:x}", Sha256::digest(&bytes)),
            ]);
        }
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
        Ok(args)
    }

    /// Ask the selected client whether its master is alive; this is a blocking control operation.
    pub fn connected(&self) -> bool {
        #[cfg(unix)]
        if self.profile.backend == Backend::Russh {
            return native::connected(&self.socket);
        }
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
        if self.profile.backend == Backend::Russh {
            return vec![
                "--ssh-client-channel".into(),
                self.socket.to_string_lossy().into(),
                if tty { "pty" } else { "pipe" }.into(),
                if x11 { "x11" } else { "none" }.into(),
            ];
        }
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
        let mut command = self.command();
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
        crate::extensions::execute(command, request, cancel)
    }
    /// Close only this connection's master and remove its private socket directory.
    pub fn cleanup(&self) {
        self.native.close();
        #[cfg(unix)]
        if self.profile.backend == Backend::Russh {
            native::stop(&self.socket);
        }
        let _ = std::fs::remove_file(&self.socket);
        if let Some(parent) = self.socket.parent() {
            let _ = std::fs::remove_file(parent.join("profile.json"));
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
        let mut command = self.command();
        let prefix = plan
            .execution_prefix("")?
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
            + " ";
        let helper = quote(&self.native.client(self)?.executable);
        command.arg(format!("{prefix}{helper} --latex-helper"));
        crate::latex::execute(command, invocation, cancel)
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
        let mut command = self.command();
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
        crate::marp::execute(command, invocation, cancel)
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
    update_profiles_at(path, |entries, profiles| {
        let label = profile.label();
        let original = previous.map_or_else(|| label.clone(), Profile::label);
        anyhow::ensure!(
            profiles.iter().filter(|p| p.label() == original).count() <= 1,
            "Several saved connections have the label {original:?}; give them distinct names in connections.json before editing them."
        );
        let index = profiles.iter().position(|p| p.label() == original);
        anyhow::ensure!(
            previous.is_none() || index.is_some() || label != original,
            "Saved connection {original:?} was removed or renamed in another window. Choose a new name to save a new connection, or reopen Connect SSH to reload existing profiles."
        );
        anyhow::ensure!(
            label == original || !profiles.iter().any(|p| p.label() == label),
            "A saved connection named {label:?} already exists. Choose a different name."
        );
        let before = previous
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or(serde_json::to_value(Profile::default())?);
        let changes = crate::persistence::changes(&before, &serde_json::to_value(profile)?);
        if let Some(index) = index {
            crate::persistence::apply(&mut entries[index], &changes);
        } else {
            entries.push(serde_json::to_value(profile)?);
        }
        Ok(true)
    })
}

/// A targeted saved-list edit, applied to the latest file rather than a stale list snapshot.
#[derive(Clone)]
pub enum ProfileListEdit {
    /// Remove this profile only if its known settings still match the displayed version.
    Remove(Box<Profile>),
    /// Move one existing profile relative to another, preserving all profile contents.
    Move {
        /// Label of the profile to move.
        label: String,
        /// Label of the row used as the insertion anchor.
        relative_to: String,
        /// Insert after the anchor when true, before it otherwise.
        after: bool,
    },
}

/// Remove or reorder a saved connection under the shared profile lock.
pub fn edit_profiles(edit: &ProfileListEdit) -> Result<Vec<Profile>> {
    edit_profiles_at(&config_path(), edit)
}

/// Apply a list edit at an explicit path, preserving concurrent edits and unknown fields.
pub fn edit_profiles_at(path: &Path, edit: &ProfileListEdit) -> Result<Vec<Profile>> {
    update_profiles_at(path, |entries, profiles| {
        let index = |label: &str| -> Result<usize> {
            let mut matches = profiles
                .iter()
                .enumerate()
                .filter(|(_, p)| p.label() == label);
            let (index, _) = matches.next().with_context(|| format!(
                "Saved connection {label:?} was removed or renamed in another window. Reopen Connect SSH to refresh the list."
            ))?;
            anyhow::ensure!(
                matches.next().is_none(),
                "Several saved connections have the label {label:?}; give them distinct names before managing them."
            );
            Ok(index)
        };
        match edit {
            ProfileListEdit::Remove(profile) => {
                let label = profile.label();
                let index = index(&label)?;
                anyhow::ensure!(
                    serde_json::to_value(&profiles[index])? == serde_json::to_value(profile)?,
                    "Saved connection {label:?} changed in another window. Reopen Connect SSH to review it before removing it."
                );
                entries.remove(index);
            }
            ProfileListEdit::Move {
                label,
                relative_to,
                after,
            } => {
                let from = index(label)?;
                let anchor = index(relative_to)?;
                if from == anchor {
                    return Ok(false);
                }
                let to = anchor - usize::from(from < anchor) + usize::from(*after);
                if from == to {
                    return Ok(false);
                }
                let entry = entries.remove(from);
                entries.insert(to, entry);
            }
        }
        Ok(true)
    })
}

fn update_profiles_at(
    path: &Path,
    update: impl FnOnce(&mut Vec<serde_json::Value>, &[Profile]) -> Result<bool>,
) -> Result<Vec<Profile>> {
    let path = path.canonicalize().unwrap_or_else(|_| path.into());
    crate::persistence::transaction(&path, |existed| {
        let mut raw: serde_json::Value = crate::persistence::read(&path, 1024 * 1024)?
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()
            .context("Invalid connection profiles; existing file was preserved")?
            .unwrap_or(serde_json::json!([]));
        let profiles: Vec<Profile> = serde_json::from_value(raw.clone())
            .context("Invalid connection profiles; existing file was preserved")?;
        anyhow::ensure!(
            profiles.len() <= 128,
            "At most 128 connection profiles are supported."
        );
        let entries = raw
            .as_array_mut()
            .context("Invalid connection profile list")?;
        if !update(entries, &profiles)? {
            return Ok(profiles);
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
mod profile_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tunnel_profiles_roundtrip_and_keep_openssh_identity_separate_from_destination() {
        let profile = Profile {
            host: "compute".into(),
            port: "2222".into(),
            user: "worker".into(),
            identity: "/keys/compute".into(),
            jump: "relay".into(),
            tunnel: Some(Box::new(TunnelRoute {
                port: 22022,
                host_key_alias: "[compute.example]:2222".into(),
            })),
            ..Default::default()
        };
        profile.validate().unwrap();
        let saved: Profile =
            serde_json::from_value(serde_json::to_value(&profile).unwrap()).unwrap();
        assert_eq!(saved.tunnel, profile.tunnel);
        let args = saved.options();
        assert!(args.iter().any(|s| s == "HostName=127.0.0.1"));
        assert!(
            args.iter()
                .any(|s| s == "HostKeyAlias=[compute.example]:2222")
        );
        assert_eq!(args.iter().filter(|s| *s == "-p").count(), 1);
        assert!(args.windows(2).any(|s| s == ["-p", "22022"]));
        assert!(args.windows(2).any(|s| s == ["-i", "/keys/compute"]));
        assert_eq!(saved.endpoint(), "worker@compute");
        assert!(
            Profile {
                jump: String::new(),
                ..profile
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn old_profiles_default_to_rust_and_explicit_openssh_survives_serialization() {
        let profile: Profile = serde_json::from_str(r#"{"host":"cluster"}"#).unwrap();
        assert_eq!(profile.backend, Backend::Russh);
        let profile = Profile {
            backend: Backend::OpenSsh,
            ..profile
        };
        let value = serde_json::to_value(&profile).unwrap();
        assert_eq!(value["backend"], "openssh");
        assert_eq!(
            serde_json::from_value::<Profile>(value).unwrap().backend,
            Backend::OpenSsh
        );
    }
    #[test]
    fn display_channels_keep_multiplex_only_policy_and_profile_preference() {
        for enabled in [false, true] {
            let profile = Profile {
                host: "cluster".into(),
                backend: Backend::OpenSsh,
                x11_forwarding: enabled,
                ..Default::default()
            };
            let saved: Profile =
                serde_json::from_value(serde_json::to_value(&profile).unwrap()).unwrap();
            assert_eq!(saved.x11_forwarding, enabled);
            let remote = Remote::new(saved).unwrap();
            let flag = if enabled { "-X" } else { "-x" };
            assert!(remote.master_args().unwrap().iter().any(|arg| arg == flag));
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

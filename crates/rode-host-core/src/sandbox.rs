//! Optional project isolation. Plans are prepared/probed on the project host.
//!
//! # Design decisions
//!
//! Isolation wraps host-side file helpers and project process trees rather than
//! the desktop GUI. This makes the same grants apply to editor reads, terminals
//! and tools on local or SSH hosts while leaving display and authentication
//! resources outside project execution. The shared JSON mount policy reduces
//! drift between local and remote enforcement.
//!
//! Availability is probed before committing a plan because some hosts lack
//! bubblewrap or usable namespaces. Requested and active isolation are separate
//! states so fallback remains visible. Once a sandbox is active, an operation
//! failure must not trigger an unsandboxed retry: that would change the access
//! policy precisely when a grant had denied access.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};
static HELPER_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Explicit project-host sandbox implementation. Older preferences retain bubblewrap.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Mount/PID/network namespace isolation through system or bundled bubblewrap.
    #[default]
    Bwrap,
    /// Unprivileged Linux access control through a native Rust launcher.
    Landlock,
}

impl Backend {
    /// Short backend name used in controls and availability diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Bwrap => "bwrap",
            Self::Landlock => "Landlock",
        }
    }
}

/// Requested project access policy. This is user intent; consult [`Plan::active`] to determine
/// actual isolation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Options {
    /// Request the selected sandbox when supported by the project host.
    pub enabled: bool,
    /// Implementation to probe; no automatic switching to another backend.
    pub backend: Backend,
    /// Keep the host network namespace instead of isolating networking.
    pub share_network: bool,
    /// Additional host paths mounted read-only.
    pub read_only: Vec<String>,
    /// Additional host paths mounted read-write.
    pub read_write: Vec<String>,
    /// Per-built-in-path access overrides, including explicit hiding.
    pub overrides: BTreeMap<String, Access>,
    /// Mount the project root read-only unless a more specific grant permits writing.
    pub project_read_only: bool,
}

/// Effective access for one sandbox mount.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Leave the path absent from the sandbox.
    Hidden,
    /// Expose the path without write access.
    ReadOnly,
    /// Expose the path with write access.
    ReadWrite,
}
impl Access {
    /// Human-readable access label for the policy editor.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hidden => "Hidden",
            Self::ReadOnly => "Read only",
            Self::ReadWrite => "Read + write",
        }
    }
    fn flag(self) -> Option<&'static str> {
        match self {
            Self::Hidden => None,
            Self::ReadOnly => Some("--ro-bind"),
            Self::ReadWrite => Some("--bind"),
        }
    }
}

/// Load the built-in mount policy shared by local and SSH hosts.
pub fn default_mounts() -> Vec<(String, Access)> {
    let policy: Policy = serde_json::from_str(include_str!("sandbox_policy.json"))
        .expect("valid built-in sandbox policy");
    policy
        .system_read_only
        .into_iter()
        .map(|p| (p, Access::ReadOnly))
        .chain(
            policy
                .home_read_only
                .into_iter()
                .map(|p| (format!("~/{p}"), Access::ReadOnly)),
        )
        .chain(
            policy
                .home_read_write
                .into_iter()
                .map(|p| (format!("~/{p}"), Access::ReadWrite)),
        )
        .collect()
}

impl Default for Options {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: Backend::default(),
            share_network: true,
            read_only: vec![],
            read_write: vec![],
            overrides: BTreeMap::new(),
            project_read_only: false,
        }
    }
}

/// Probed project launch policy. `requested` records intent; `active` alone authorizes isolated
/// launch. A failed probe records visible normal-mode fallback rather than a silently weakened
/// active sandbox.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Plan {
    /// Selected implementation, retained even when its probe is unavailable.
    #[serde(default)]
    pub backend: Backend,
    /// Opaque native gateway context; absent for local execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_context: Option<String>,
    /// Verified native helper on this host; used for policy and environment launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_helper: Option<String>,
    /// Captured project environment provider; resolved inside the execution boundary at launch.
    #[serde(default)]
    pub environment: crate::environment::Settings,
    /// Canonical project root on the project host.
    pub root: String,
    /// Whether sandboxing was requested.
    pub requested: bool,
    /// Whether the complete selected plan passed its capability probe.
    pub active: bool,
    /// Whether the active sandbox mounts the project read-only.
    #[serde(default)]
    pub project_read_only: bool,
    /// Probe/fallback explanation for display.
    pub reason: String,
    /// Literal sandbox launcher/arguments ending in `--`, ready to prefix a command.
    pub prefix: Vec<String>,
    #[serde(skip)]
    /// Retained local executable inode; never serialised to another host.
    pub helper: Option<Arc<HelperBinary>>,
}

/// Keep the helper's inode alive while a project is open, including when the
/// user rebuilds/replaces RODE itself. Workers retain it until their I/O ends.
#[derive(Debug)]
pub struct HelperBinary {
    directory: PathBuf,
    /// Retained executable path, owned by this lease.
    pub path: PathBuf,
}

impl HelperBinary {
    /// Retain the current helper across replacement until all launch users finish.
    pub fn new() -> Result<Arc<Self>> {
        let directory = std::env::temp_dir().join(format!(
            "rode-helper-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
            HELPER_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        let directory = directory.canonicalize()?;
        let helper = Self {
            path: directory.join("rode"),
            directory,
        };
        if std::fs::hard_link(std::env::current_exe()?, &helper.path).is_err() {
            #[cfg(target_os = "linux")]
            let source = PathBuf::from("/proc/self/exe");
            #[cfg(not(target_os = "linux"))]
            let source = std::env::current_exe()?;
            std::fs::copy(source, &helper.path)
                .context("Cannot prepare the local sandbox file helper")?;
        }
        Ok(Arc::new(helper))
    }
}

impl Drop for HelperBinary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[derive(Deserialize)]
struct Policy {
    system_read_only: Vec<String>,
    home_read_only: Vec<String>,
    home_read_write: Vec<String>,
}

impl Options {
    /// Validate policy bounds and reject conflicting or overbroad home-directory grants.
    pub fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let defaults = default_mounts();
        if self
            .overrides
            .keys()
            .any(|p| !defaults.iter().any(|(path, _)| path == p))
        {
            bail!(
                "Sandbox overrides may only name built-in mount paths. Use additional paths for other folders."
            );
        }
        if self.read_only.len() + self.read_write.len() > 32 {
            bail!("At most 32 additional sandbox paths are supported.");
        }
        if self
            .read_only
            .iter()
            .chain(&self.read_write)
            .map(String::len)
            .sum::<usize>()
            > 16 * 1024
        {
            bail!("Additional sandbox paths exceed the 16 KiB total limit.");
        }
        for path in self.read_only.iter().chain(&self.read_write) {
            if path.is_empty() || path.len() > 4096 || path.contains('\0') {
                bail!("Invalid sandbox path.");
            }
            if !path.starts_with('/') && path != "~" && !path.starts_with("~/") {
                bail!("Additional sandbox paths must be absolute or start with ~/.");
            }
        }
        Ok(())
    }
}

/// Prepare and probe a policy on this host; may return an inactive fallback plan. This performs
/// filesystem/process work and belongs off the UI thread.
pub fn prepare(root: &str, options: &Options) -> Result<Plan> {
    let helper = HelperBinary::new()?;
    let mut plan = prepare_with_helper(root, options, &helper.path)?;
    plan.helper = Some(helper);
    Ok(plan)
}

/// Native host preparation uses the already verified helper for launch-time work.
pub fn prepare_native(root: &str, options: &Options) -> Result<Plan> {
    let executable = std::env::current_exe()?.canonicalize()?;
    prepare_with_helper(root, options, &executable)
}

/// Prepare using a caller-owned Rust helper. The caller retains its executable for
/// the plan's lifetime; remote callers use a verified immutable cache generation.
pub fn prepare_with_helper(root: &str, options: &Options, executable: &Path) -> Result<Plan> {
    let executable = executable
        .canonicalize()
        .context("Cannot resolve the native host helper")?;
    let root = crate::files::expand_home(root)
        .canonicalize()
        .context("Cannot open project folder")?;
    if !root.is_dir() {
        bail!("Choose a directory to open as a project.");
    }
    options.validate()?;
    let mut plan = Plan {
        root: root.to_string_lossy().into(),
        native_helper: Some(executable.to_string_lossy().into_owned()),
        requested: options.enabled,
        backend: options.backend,
        project_read_only: options.project_read_only,
        ..Default::default()
    };
    if !options.enabled {
        return Ok(plan);
    }
    #[cfg(not(target_os = "linux"))]
    {
        plan.reason =
            "Project sandboxing requires Linux; this project is open in normal mode.".into();
        return Ok(plan);
    }
    #[cfg(target_os = "linux")]
    {
        let home = crate::files::expand_home("~");
        validate_project_root(&root, &home)?;
        let (binary, bundled) = match options.backend {
            Backend::Landlock => (executable.clone(), false),
            Backend::Bwrap => match find_program("bwrap") {
                Some(binary) => (binary, false),
                None => (executable.clone(), true),
            },
        };
        plan.prefix = arguments(&binary, &root, &home, options)?;
        if options.backend == Backend::Landlock {
            plan.prefix.insert(1, "--landlock-launch".into());
        } else if bundled {
            plan.prefix.insert(1, "--bwrap-launch".into());
        }
        let path = executable.to_string_lossy().into_owned();
        let at = plan.prefix.len() - 1;
        plan.prefix
            .splice(at..at, ["--ro-bind".into(), path.clone(), path]);
        let mut command = Command::new(&plan.prefix[0]);
        command.args(&plan.prefix[1..]).arg("/bin/true");
        match crate::command::run_bounded(command, vec![], Duration::from_secs(10)) {
            Ok(_) => {
                plan.active = true;
                if options.backend == Backend::Landlock {
                    plan.reason = crate::landlock::compatibility_notice()?;
                } else if bundled {
                    plan.reason =
                        "Using bundled bubblewrap; no system bwrap was found on this host.".into();
                }
            }
            Err(error) => {
                plan.prefix.clear();
                let detail: String = format!("{error:#}").chars().take(2048).collect();
                plan.reason = format!(
                    "{} cannot start on this host; normal mode: {detail}{}",
                    options.backend.label(),
                    if bundled {
                        " (bundled bubblewrap; system bwrap was not found)"
                    } else {
                        ""
                    }
                );
            }
        }
        Ok(plan)
    }
}

fn validate_project_root(root: &Path, home: &Path) -> Result<()> {
    if project_root_contains_home(root, home)? {
        bail!(
            "Choose a project below your home directory or a separate project directory. Sandboxing the whole home directory (or its parent) would expose it."
        );
    }
    Ok(())
}

/// Whether a canonical project root would expose the whole home directory.
/// Desktop startup also uses this check before inferring a project from a file.
/// Resolution failures remain errors, not permission to fall back to normal mode.
pub fn project_root_contains_home(root: &Path, home: &Path) -> Result<bool> {
    // HOME can be an automount/symlink alias while project and extra grants have
    // already been canonicalized. Compare both spellings before exposing a root.
    let actual_home = home
        .canonicalize()
        .context("Cannot resolve the home directory while checking sandbox grants")?;
    Ok(root == Path::new("/") || home.starts_with(root) || actual_home.starts_with(root))
}

/// Find an executable in PATH without invoking a shell.
pub fn find_program(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| {
                p.metadata()
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    })
}

/// Build the shared literal policy arguments, using bubblewrap's mount vocabulary.
/// Landlock consumes that vocabulary as access grants and rejects remaps/subtractive rules.
/// Orders parent mounts before nested overrides; does not execute or prove availability.
pub fn arguments(
    binary: &Path,
    root: &Path,
    home: &Path,
    options: &Options,
) -> Result<Vec<String>> {
    options.validate()?;
    validate_project_root(root, home)?;
    let policy: Policy = serde_json::from_str(include_str!("sandbox_policy.json"))?;
    // Resolve Landlock aliases once, on the project host during preparation.
    // Landlock launch must not reinterpret a captured grant after a tool changes
    // a symlink beneath an already-writable parent.
    // /etc preserves its nested symlinks in either backend. Share the resolver's
    // actual file too, commonly under /run/systemd/resolve, without exposing /run.
    let grant_path = |path: &Path| -> Result<String> {
        if options.backend == Backend::Landlock || path == Path::new("/etc/resolv.conf") {
            let path = path
                .canonicalize()
                .with_context(|| format!("Cannot resolve sandbox grant: {}", path.display()))?;
            validate_project_root(&path, home)?;
            Ok(path.to_string_lossy().into_owned())
        } else {
            Ok(path.to_string_lossy().into_owned())
        }
    };
    let launcher = if options.backend == Backend::Landlock {
        // Capture the native executable before any project tool runs.
        binary
            .canonicalize()
            .context("Cannot resolve the Landlock launcher")?
    } else {
        binary.to_owned()
    };
    let mut args = vec![
        launcher.to_string_lossy().into(),
        "--unshare-all".into(),
        "--die-with-parent".into(),
    ];
    if options.share_network {
        args.push("--share-net".into());
    }
    for path in policy.system_read_only {
        let access = options
            .overrides
            .get(&path)
            .copied()
            .unwrap_or(Access::ReadOnly);
        if let Some(flag) = access.flag()
            && Path::new(&path).exists()
        {
            let path = grant_path(Path::new(&path))?;
            args.extend([flag.into(), path.clone(), path]);
        }
    }
    args.extend([
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--tmpfs".into(),
        "/home".into(),
        "--dir".into(),
        home.to_string_lossy().into(),
        "--dir".into(),
        home.join(".config").to_string_lossy().into(),
    ]);
    for (access, paths) in [
        (Access::ReadOnly, policy.home_read_only),
        (Access::ReadWrite, policy.home_read_write),
    ] {
        for relative in paths {
            let access = options
                .overrides
                .get(&format!("~/{relative}"))
                .copied()
                .unwrap_or(access);
            let path = home.join(relative);
            if let Some(flag) = access.flag()
                && path.exists()
            {
                let path = grant_path(&path)?;
                args.extend([flag.into(), path.clone(), path]);
            }
        }
    }
    let mut mounts = vec![(
        if options.project_read_only {
            "--ro-bind"
        } else {
            "--bind"
        },
        root.to_path_buf(),
    )];
    let mut seen = std::collections::HashSet::from([root.to_path_buf()]);
    for (flag, paths) in [
        ("--ro-bind", &options.read_only),
        ("--bind", &options.read_write),
    ] {
        for path in paths {
            let path = crate::files::expand_home(path)
                .canonicalize()
                .with_context(|| format!("Additional sandbox path is unavailable: {path}"))?;
            validate_project_root(&path, home)?;
            if !seen.insert(path.clone()) {
                bail!(
                    "Duplicate sandbox grant. Use the project access checkbox for its root, and one rule per additional path."
                );
            }
            mounts.push((flag, path));
        }
    }
    // A parent grant must never mask a more specific child's restrictions.
    mounts.sort_by_key(|(_, path)| path.components().count());
    for (flag, path) in mounts {
        let path = path.to_string_lossy().into_owned();
        args.extend([flag.into(), path.clone(), path]);
    }
    let root = root.to_string_lossy().into_owned();
    if options.backend == Backend::Landlock {
        args.extend([
            "--rode-home".into(),
            home.canonicalize()?.to_string_lossy().into_owned(),
        ]);
        // Hidden paths must not be re-exposed through an additive parent grant.
        for (path, access) in &options.overrides {
            if *access == Access::Hidden {
                let path = path
                    .strip_prefix("~/")
                    .map_or_else(|| PathBuf::from(path), |p| home.join(p));
                args.extend(["--rode-deny".into(), path.to_string_lossy().into()]);
            }
        }
    }
    args.extend([
        "--setenv".into(),
        "HOME".into(),
        home.to_string_lossy().into(),
        "--setenv".into(),
        "UV_LINK_MODE".into(),
        "copy".into(),
        "--chdir".into(),
        root,
        "--".into(),
    ]);
    Ok(args)
}

impl Plan {
    /// Persistent UI label for actual isolation/read-only/fallback state.
    pub fn label(&self) -> &'static str {
        if self.active {
            match (self.backend, self.project_read_only) {
                (Backend::Bwrap, false) => "SANDBOX · bwrap",
                (Backend::Bwrap, true) => "SANDBOX · bwrap · READ ONLY",
                (Backend::Landlock, false) => "SANDBOX · Landlock",
                (Backend::Landlock, true) => "SANDBOX · Landlock · READ ONLY",
            }
        } else if self.requested {
            "NORMAL MODE · sandbox unavailable"
        } else {
            "NORMAL MODE"
        }
    }

    /// Prepare a policy-scoped helper command and return its binary lease alongside it.
    /// The caller must retain that lease until the launched helper no longer uses the file.
    pub fn coordinator_command(&self, flag: &str) -> Result<(Command, Option<Arc<HelperBinary>>)> {
        let helper = self.helper.clone();
        let executable = self
            .native_helper
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| helper.as_ref().map(|h| h.path.clone()))
            .unwrap_or(crate::command::current_executable()?);
        let mut command = if self.active {
            let destination = self.helper_destination(&executable, "/rode-extension-helper");
            let mut args = self.prefix[1..].to_vec();
            anyhow::ensure!(args.pop().as_deref() == Some("--"), "Invalid sandbox plan");
            args.extend([
                "--ro-bind".into(),
                executable.to_string_lossy().into(),
                destination.clone(),
                "--".into(),
            ]);
            args.extend(self.environment_prefix("")?);
            args.push(destination);
            let mut cmd = Command::new(&self.prefix[0]);
            cmd.args(args);
            cmd
        } else {
            let mut argv = self.environment_prefix("")?;
            argv.push(executable.to_string_lossy().into_owned());
            let mut command = Command::new(&argv[0]);
            command.args(&argv[1..]);
            command
        };
        command.arg(flag);
        Ok((command, helper))
    }
    /// Landlock grants the original path; only bubblewrap can remap it.
    pub fn helper_destination(&self, source: &Path, mounted: &str) -> String {
        if self.backend == Backend::Landlock {
            source.to_string_lossy().into_owned()
        } else {
            mounted.into()
        }
    }
}

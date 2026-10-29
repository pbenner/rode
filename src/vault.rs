//! Host-side gocryptfs mounts. Passwords travel over stdin, never arguments,
//! preferences or files. The decrypted tree is the ordinary project root.
//!
//! # Design decisions
//!
//! Filesystem encryption through installed gocryptfs lets ordinary editor I/O,
//! build tools and agents use the same decrypted project paths; RODE implements
//! mount ownership rather than its own cryptography or encrypted buffer format.
//! This requires working host FUSE/tools, and encryption protects stored files,
//! not arbitrary plaintext tool caches or a running process's memory.
//!
//! Locking requires verified normal unmount after jobs and other leases stop.
//! Lazy unmount could hide the desktop path while a sandbox or persistent shell
//! still holds decrypted data, making a "locked" label misleading. Shared leases,
//! mount identity and namespace checks trade a more involved close lifecycle for
//! a truthful lock result; unknown or busy state remains visibly unlocked.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
static CANCELLED: AtomicBool = AtomicBool::new(false);
/// Keep the underlying mountpoint inode inaccessible after unlock/failure, even while the
/// visible path is covered by FUSE.
struct CoveredDirectory(File);
impl Drop for CoveredDirectory {
    fn drop(&mut self) {
        let _ = self.0.set_permissions(fs::Permissions::from_mode(0o000));
    }
}

/// Persisted encrypted-storage paths and installed tool settings; never includes a password.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Encrypted backing directory on the selected host.
    pub cipherdir: String,
    /// Installed gocryptfs executable on that host.
    pub program: String,
    /// Optional installed normal-unmount executable override.
    pub unmount_program: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            cipherdir: String::new(),
            program: "gocryptfs".into(),
            unmount_program: String::new(),
        }
    }
}
impl Settings {
    /// Validate bounded paths/tool names without creating or mounting storage.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.cipherdir.trim().is_empty() && !self.program.trim().is_empty(),
            "Choose an encrypted folder and gocryptfs executable."
        );
        ensure!(
            [&self.cipherdir, &self.program, &self.unmount_program]
                .into_iter()
                .all(|s| s.len() <= 4096 && !s.contains('\0')),
            "Invalid encrypted-project path or executable."
        );
        Ok(())
    }
}
// Intentionally no Debug: this request contains a password.
/// Secret-bearing unlock/create request sent over stdin. Intentionally has no `Debug`
/// implementation and must never enter logs or preferences.
#[derive(Serialize, Deserialize)]
pub struct Open {
    /// Nonsecret encrypted-storage/tool configuration.
    pub settings: Settings,
    /// Transient password passed only to the owned helper's stdin.
    pub password: String,
    /// Explicit permission to initialize empty encrypted storage.
    pub create: bool,
    /// Request locking an existing mount without opening a project lease.
    #[serde(default)]
    pub lock_only: bool,
}
/// Verified host-side encrypted-storage and decrypted-mount identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    /// Canonical encrypted backing path.
    pub cipherdir: String,
    /// Verified decrypted FUSE mount path used as the project root.
    pub mountpoint: String,
}
/// Host lease-coordinator replies; absence of a verified lock is never described as locked.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    /// Encrypted mount verified and the caller's lease acquired.
    Opened {
        /// Canonical backing/mount identity.
        info: Info,
    },
    /// Lease close/release result, possibly still held open by another window or jobs.
    Closed {
        /// Canonical backing/mount identity.
        info: Info,
        /// Whether normal unmount was verified successful.
        locked: bool,
        /// Explanation for shared, busy, unknown or completed close state.
        message: String,
    },
    /// Unlock/create/lock failure with no secret included.
    Error {
        /// User-visible sanitized failure explanation.
        message: String,
    },
}

fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
        "Encrypted-project runtime must be an owner-only directory without symlinks."
    );
    Ok(())
}
use std::os::unix::fs::DirBuilderExt;
/// Host filesystem ownership separate from project execution. The shared lease and open
/// directory descriptor prevent normal lock while this view still uses decrypted storage.
struct Mount {
    settings: Settings,
    info: Info,
    runtime: PathBuf,
    name: String,
    lease: Option<File>,
    directory: Option<File>,
    attempted: bool,
}
fn decode_mount(value: &str) -> String {
    let mut result = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            result
                .push((bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + bytes[i + 3] - b'0');
            i += 4;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&result).into()
}
fn mount_table(path: impl AsRef<Path>) -> Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 8 * 1024 * 1024,
        "Mount information exceeds the verification limit."
    );
    Ok(String::from_utf8(bytes)?)
}
fn mount_entry(text: &str, point: &str) -> Option<(String, String, String)> {
    text.lines().find_map(|line| {
        let (left, right) = line.split_once(" - ")?;
        let fields: Vec<_> = left.split_whitespace().collect();
        let tail: Vec<_> = right.split_whitespace().collect();
        (fields.len() >= 6 && tail.len() >= 2 && decode_mount(fields[4]) == point)
            .then(|| (fields[2].into(), tail[0].into(), decode_mount(tail[1])))
    })
}
impl Mount {
    fn new(settings: Settings, create: bool) -> Result<Self> {
        settings.validate()?;
        let cipher = crate::files::expand_home(&settings.cipherdir);
        if create && !cipher.exists() {
            // Only create the selected directory; never recursively modify parents.
            fs::DirBuilder::new().mode(0o700).create(&cipher)?;
        }
        let cipher = cipher
            .canonicalize()
            .context("Cannot open the encrypted folder")?;
        ensure!(cipher.is_dir(), "Encrypted storage must be a directory.");
        let digest = crate::files::digest(cipher.to_string_lossy().as_bytes());
        let base = PathBuf::from(format!("/tmp/rode-vaults-{}", unsafe { libc::geteuid() }));
        private_directory(&base)?;
        let base = base.canonicalize()?;
        let runtime = base.join(&digest);
        private_directory(&runtime)?;
        let mountpoint = runtime.join("plain");
        if !mountpoint.exists() {
            private_directory(&mountpoint)?;
        }
        ensure!(
            !fs::symlink_metadata(&mountpoint)?.file_type().is_symlink(),
            "The decrypted mountpoint cannot be a symlink."
        );
        Ok(Self {
            settings,
            info: Info {
                cipherdir: cipher.to_string_lossy().into(),
                mountpoint: mountpoint.to_string_lossy().into(),
            },
            name: format!("rode-gocryptfs-{digest}"),
            runtime,
            lease: None,
            directory: None,
            attempted: false,
        })
    }
    fn lock_file(&self, name: &str) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.runtime.join(name))?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file()
                && meta.uid() == unsafe { libc::geteuid() }
                && meta.mode() & 0o077 == 0
                && meta.nlink() == 1,
            "Unsafe encrypted-project coordination file."
        );
        Ok(file)
    }
    fn operation(&self) -> Result<File> {
        let lock = self.lock_file("operation.lock")?;
        crate::persistence::lock(&lock)?;
        Ok(lock)
    }
    fn mounted(&self) -> Result<Option<String>> {
        let table = mount_table("/proc/self/mountinfo")
            .context("Encrypted projects require Linux mount information")?;
        match mount_entry(&table, &self.info.mountpoint) {
            Some((device, kind, name)) => {
                ensure!(
                    kind == "fuse.gocryptfs" && name == self.name,
                    "Refusing to manage an unrelated filesystem at the encrypted-project mountpoint."
                );
                Ok(Some(device))
            }
            None => Ok(None),
        }
    }
    fn acquire(&mut self) -> Result<()> {
        let lease = self.lock_file("views.lock")?;
        ensure!(
            unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0,
            "Encrypted project is being locked by another window."
        );
        self.directory = Some(File::open(&self.info.mountpoint)?);
        self.lease = Some(lease);
        Ok(())
    }
    fn open(&mut self, password: &str, create: bool) -> Result<()> {
        let _operation = self.operation()?;
        if self.mounted()?.is_some() {
            ensure!(
                !create,
                "This encrypted project is already initialized and unlocked."
            );
            return self.acquire();
        }
        fs::set_permissions(&self.info.mountpoint, fs::Permissions::from_mode(0o700))?;
        // The underlying empty directory is inaccessible whenever the FUSE
        // mount is absent. A late operation cannot save plaintext into it.
        let _covered = CoveredDirectory(File::open(&self.info.mountpoint)?);
        ensure!(
            fs::read_dir(&self.info.mountpoint)?.next().is_none(),
            "The decrypted mountpoint must be empty."
        );
        ensure!(
            !password.is_empty()
                && password.len() <= 1000
                && !password.contains(['\n', '\r', '\0']),
            "Enter a password of 1–1000 bytes without line breaks."
        );
        let config = Path::new(&self.info.cipherdir).join("gocryptfs.conf");
        if create {
            ensure!(
                fs::read_dir(&self.info.cipherdir)?.next().is_none(),
                "New encrypted storage must be empty. Existing files are never converted or removed."
            );
            self.tool(&["-init", "-q", "--", &self.info.cipherdir], password)?;
        }
        ensure!(
            config.is_file(),
            "No gocryptfs.conf found. Select existing encrypted storage or enable creation for an empty folder."
        );
        self.attempted = true;
        self.tool(
            &[
                "-q",
                "-fsname",
                &self.name,
                "--",
                &self.info.cipherdir,
                &self.info.mountpoint,
            ],
            password,
        )?;
        ensure!(
            self.mounted()?.is_some(),
            "gocryptfs exited without creating a verified encrypted mount. FUSE must be available on the project host."
        );
        self.acquire()
    }
    fn tool(&self, args: &[&str], password: &str) -> Result<()> {
        let mut command = Command::new(crate::files::expand_home(&self.settings.program));
        command.args(args).current_dir("/");
        let result = crate::project::run_controlled_limited(
            command,
            password.as_bytes().to_vec(),
            Duration::from_secs(60),
            &CANCELLED,
            false,
            64 * 1024,
            Duration::ZERO,
        )?;
        ensure!(
            result.success(),
            "gocryptfs failed ({}): {}",
            result.code,
            result.stderr.replace(password, "[redacted]")
        );
        Ok(())
    }
    fn release(&mut self, retain_on_failure: bool, close_shared: bool) -> Result<bool> {
        let _operation = self.operation()?;
        self.directory = None;
        self.lease = None;
        let result = self.unmount_locked();
        let shared = result.as_ref().err().is_some_and(|error| {
            error
                .to_string()
                .starts_with("Another RODE window still has this encrypted project open.")
        });
        if result.is_err()
            && retain_on_failure
            && !(close_shared && shared)
            && self.mounted()?.is_some()
        {
            self.acquire()?;
        }
        result
    }
    fn unmount_locked(&self) -> Result<bool> {
        let lease = self.lock_file("views.lock")?;
        ensure!(
            unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "Another RODE window still has this encrypted project open."
        );
        ensure!(
            !foreign_mount(&self.name, &self.info.mountpoint)?,
            "The decrypted filesystem is still mounted by another process or sandbox. Stop that work before locking."
        );
        if self.mounted()?.is_none() {
            return Ok(true);
        }

        let program = if self.settings.unmount_program.trim().is_empty() {
            crate::sandbox::find_program("fusermount3").or_else(|| crate::sandbox::find_program("fusermount")).context("Install FUSE's fusermount3/fusermount on the project host to lock this project")?
        } else {
            crate::files::expand_home(&self.settings.unmount_program)
        };
        let mut command = Command::new(program);
        command
            .args(["-u", "--", &self.info.mountpoint])
            .current_dir("/");
        let output = crate::project::run(command, vec![], Duration::from_secs(10))?;
        ensure!(
            output.success(),
            "Project remains unlocked (it may be busy): {}",
            output.stderr
        );
        ensure!(
            self.mounted()?.is_none(),
            "Encrypted filesystem is still mounted."
        );
        ensure!(
            !foreign_mount(&self.name, &self.info.mountpoint)?,
            "A process still holds the decrypted filesystem. Stop that work before locking."
        );
        Ok(true)
    }
}
fn foreign_mount(name: &str, point: &str) -> Result<bool> {
    fn references(table: &str, name: &str, ignored_point: Option<&str>) -> bool {
        table.lines().any(|line| {
            let Some((left, right)) = line.split_once(" - ") else {
                return false;
            };
            let mut tail = right.split_whitespace();
            tail.next() == Some("fuse.gocryptfs")
                && tail.next() == Some(name)
                && left
                    .split_whitespace()
                    .nth(4)
                    .is_some_and(|p| ignored_point != Some(decode_mount(p).as_str()))
        })
    }
    if references(&mount_table("/proc/self/mountinfo")?, name, Some(point)) {
        return Ok(true);
    }
    let own = fs::read_link("/proc/self/ns/mnt")?;
    let mut seen = std::collections::HashSet::from([own]);
    for (index, entry) in fs::read_dir("/proc")?.enumerate() {
        ensure!(
            index < 65536,
            "Too many processes to verify encrypted-project lock state safely."
        );
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
        {
            continue;
        }
        let path = entry.path();
        if !path
            .metadata()
            .is_ok_and(|m| m.uid() == unsafe { libc::geteuid() })
        {
            continue;
        }
        let namespace = match fs::read_link(path.join("ns/mnt")) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .context("Cannot verify other mount namespaces; project remains unlocked");
            }
        };
        if !seen.insert(namespace) {
            continue;
        }
        let table = match mount_table(path.join("mountinfo")) {
            Ok(value) => value,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(error) => {
                return Err(error)
                    .context("Cannot verify other mount namespaces; project remains unlocked");
            }
        };
        if references(&table, name, None) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn emit(reply: &Reply) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, reply)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}
fn release(mount: &mut Mount, retain_on_failure: bool, close_shared: bool) -> Reply {
    let result = mount.release(retain_on_failure, close_shared);
    Reply::Closed {
        info: mount.info.clone(),
        locked: result.is_ok(),
        message: result.err().map(|e| format!("{e:#}")).unwrap_or_default(),
    }
}
/// Serve the host-level stdin lease protocol. Serializes mount changes, verifies FUSE identity
/// and refuses busy normal unmounts; it runs outside project isolation solely for filesystem
/// management.
pub fn coordinator() -> Result<()> {
    // Termination must close the lease and try a normal unmount, never force it.
    extern "C" fn terminate(_: libc::c_int) {
        CANCELLED.store(true, Ordering::Release);
        unsafe {
            libc::close(libc::STDIN_FILENO);
        }
    }
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = terminate as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGHUP, &action, std::ptr::null_mut());
    }
    let mut reader = std::io::BufReader::new(std::io::stdin());
    let mut line = String::new();
    reader.by_ref().take(32 * 1024).read_line(&mut line)?;
    let request: Open = serde_json::from_str(&line).context("Invalid encrypted-project request")?;
    let mut mount = Mount::new(request.settings, request.create)?;
    if request.lock_only {
        ensure!(
            Path::new(&mount.info.cipherdir)
                .join("gocryptfs.conf")
                .is_file(),
            "No gocryptfs.conf found in the selected encrypted folder."
        );
        emit(&release(&mut mount, false, false))?;
        return Ok(());
    }
    let result = mount.open(&request.password, request.create);
    drop(request.password);
    line.clear();
    if let Err(error) = result {
        emit(&Reply::Error {
            message: format!("{error:#}"),
        })?;
        if mount.attempted || mount.lease.is_some() || mount.directory.is_some() {
            let _ = emit(&release(&mut mount, false, false));
        }
        return Ok(());
    }
    if emit(&Reply::Opened {
        info: mount.info.clone(),
    })
    .is_err()
    {
        let _ = mount.release(false, false);
        return Ok(());
    }
    loop {
        let mut control = String::new();
        let _ = reader.by_ref().take(4096).read_line(&mut control);
        let action = serde_json::from_str::<serde_json::Value>(&control)
            .ok()
            .and_then(|v| v["action"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let lock_requested = action == "lock" || action == "close";
        let close_shared = action == "close";
        let reply = release(
            &mut mount,
            lock_requested && !CANCELLED.load(Ordering::Acquire),
            close_shared,
        );
        let locked = matches!(reply, Reply::Closed { locked: true, .. });
        let shared_closed = close_shared
            && matches!(&reply, Reply::Closed { locked: false, message, .. } if message.starts_with("Another RODE window still has this encrypted project open."));
        let _ = emit(&reply);
        if !lock_requested || locked || shared_closed || CANCELLED.load(Ordering::Acquire) {
            break;
        }
    }
    Ok(())
}

/// A host-level lease uses the same bounded asynchronous transport as project
/// tools, with an explicit unsandboxed launch limited to filesystem management.
pub struct Client {
    /// Owned asynchronous lease-coordinator transport, independent of project tools.
    pub session: crate::process::Session,
    /// Mount identity once verified by the coordinator.
    pub info: Option<Info>,
    /// SSH identity owning this host lease, or local when absent.
    pub remote: Option<crate::ssh::Remote>,
    /// Whether the lease close lifecycle completed.
    pub finished: bool,
}
impl Client {
    /// Start local/SSH filesystem management with transient stdin credentials; launch no
    /// project process before verified mount information arrives.
    pub fn start(
        request: Open,
        remote: Option<crate::ssh::Remote>,
        wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        request.settings.validate()?;
        let argv = if remote.is_some() {
            vec![
                "python3".into(),
                "-u".into(),
                "-c".into(),
                include_str!("remote_vault.py").into(),
            ]
        } else {
            vec![
                format!("/proc/{}/exe", std::process::id()),
                "--vault-helper".into(),
            ]
        };
        let session = crate::process::Session::start(
            crate::process::Launch {
                native_debug: false,
                argv,
                plan: crate::sandbox::Plan {
                    root: "/".into(),
                    ..Default::default()
                },
                remote: remote.clone(),
                python: String::new(),
                python_program: false,
                line_limit: 64 * 1024,
            },
            wake,
        )?;
        session.send(&serde_json::to_value(request)?)?;
        Ok(Self {
            session,
            info: None,
            remote,
            finished: false,
        })
    }
    /// Request release of this caller's shared lease; does not force-unmount another window's
    /// project.
    pub fn release(&self) {
        let _ = self.session.send(&serde_json::json!({"action":"release"}));
    }
    /// Check whether a root is within this verified mount and uses the same local/SSH
    /// connection identity.
    pub fn matches(&self, root: &str, remote: Option<&crate::ssh::Remote>) -> bool {
        self.info
            .as_ref()
            .is_some_and(|info| Path::new(root).starts_with(&info.mountpoint))
            && self.remote.as_ref().map(|r| &r.socket) == remote.map(|r| &r.socket)
    }
}

/// Keep editor-controlled rendered artifacts off an ordinary temporary disk
/// when their source is on an encrypted mount. Private sandbox /tmp is tmpfs.
pub fn scratch_parent(root: &Path) -> Result<PathBuf> {
    let temporary = std::env::temp_dir();
    let table = mount_table("/proc/self/mountinfo")?;
    let encrypted = table.lines().any(|line| {
        let Some((left, right)) = line.split_once(" - ") else {
            return false;
        };
        right.split_whitespace().next() == Some("fuse.gocryptfs")
            && left
                .split_whitespace()
                .nth(4)
                .is_some_and(|point| root.starts_with(decode_mount(point)))
    });
    if !encrypted {
        return Ok(temporary);
    }
    for candidate in [temporary, PathBuf::from("/dev/shm")] {
        if let Ok(path) = std::ffi::CString::new(candidate.as_os_str().as_encoded_bytes()) {
            let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
            if unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) } == 0
                && unsafe { stats.assume_init() }.f_type == libc::TMPFS_MAGIC
            {
                return Ok(candidate);
            }
        }
    }
    // No available RAM filesystem: artifacts stay on the encrypted filesystem.
    // A read-only encrypted project then fails visibly instead of spilling data.
    Ok(root.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mount_identity_and_escaped_paths_are_checked() {
        let text = "10 9 0:42 / /tmp/a\\040b rw - fuse.gocryptfs rode-gocryptfs-test rw\n";
        assert_eq!(
            mount_entry(text, "/tmp/a b"),
            Some((
                "0:42".into(),
                "fuse.gocryptfs".into(),
                "rode-gocryptfs-test".into()
            ))
        );
        assert!(mount_entry(text, "/tmp/a").is_none());
    }
    #[test]
    fn shared_views_prevent_exclusive_lock_and_release_after_exit() {
        let root = tempfile::tempdir().unwrap();
        let mount = Mount::new(
            Settings {
                cipherdir: root.path().to_string_lossy().into(),
                ..Default::default()
            },
            false,
        )
        .unwrap();
        let first = mount.lock_file("views.lock").unwrap();
        let second = mount.lock_file("views.lock").unwrap();
        assert_eq!(unsafe { libc::flock(first.as_raw_fd(), libc::LOCK_SH) }, 0);
        assert_ne!(
            unsafe { libc::flock(second.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        drop(first);
        assert_eq!(
            unsafe { libc::flock(second.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        drop(second);
        fs::remove_file(mount.runtime.join("views.lock")).unwrap();
        fs::remove_dir(&mount.info.mountpoint).unwrap();
        fs::remove_dir(&mount.runtime).unwrap();
    }
}

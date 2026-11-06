//! Native agent file operations with a directory capability and no executable input.
//!
//! Every lookup uses Linux openat2 beneath the project descriptor. Symlinks,
//! magic links, nested mounts, special files and agent/Git configuration are
//! rejected. Writes replace directory entries atomically rather than modifying
//! hard-linked inodes. Landlock confines each one-shot helper even if another
//! process moves an opened directory outside the root during an operation.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::fs::PermissionsExt,
    },
    path::{Component, Path, PathBuf},
};

const MAX_FILE: u64 = 1024 * 1024;
const MAX_ENTRIES: usize = 1000;
const MAX_SEARCH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
/// Fixed one-shot helper envelope; only a trusted controller supplies its root/policy.
pub struct Request {
    /// Captured canonical project directory on this host.
    pub root: String,
    /// One of the supported native tool names.
    pub tool: String,
    /// Tool-specific object, never shell text.
    pub arguments: Value,
    /// The project's read-only restriction cannot be relaxed by agent mode.
    pub read_only: bool,
    /// Trusted executable paths that must not be replaced by their own agent.
    #[serde(default)]
    pub protected_paths: Vec<String>,
    /// Configuration aliases captured by the controller's sandbox plan. These
    /// supplement the helper's own effective CODEX_HOME and deny reads too.
    #[serde(default)]
    pub protected_configuration: Vec<String>,
}

/// Capture the effective Codex state root and its host/mount aliases without
/// granting access to it. Relative CODEX_HOME follows the project working folder;
/// an unset/empty value defaults to HOME/.codex. Unknown controller HOME produces
/// no extra aliases; the helper independently resolves its actual OS home and
/// rejects requests if its state root cannot be determined.
pub fn configuration_paths(
    root: &Path,
    codex_home: Option<&str>,
    home: Option<&str>,
    prefix: &[String],
) -> Result<Vec<String>> {
    ensure!(
        root.is_absolute(),
        "Agent configuration needs an absolute project root"
    );
    let configured = crate::command::sandbox_variable(prefix, "CODEX_HOME")
        .or(codex_home)
        .filter(|value| !value.is_empty());
    let path = if let Some(configured) = configured {
        let path = PathBuf::from(configured);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    } else {
        let Some(home) = crate::command::sandbox_variable(prefix, "HOME")
            .or(home)
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
        else {
            return Ok(vec![]);
        };
        home.join(".codex")
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            _ => normalized.push(component.as_os_str()),
        }
    }
    let mut paths = vec![path.clone(), normalized];
    if let Ok(resolved) = path.canonicalize() {
        paths.push(resolved);
    }
    paths.extend(
        paths
            .clone()
            .iter()
            .map(|path| crate::command::sandbox_host_path(path, prefix)),
    );
    paths.sort();
    paths.dedup();
    Ok(paths
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}

/// Decode the fixed helper response without treating a denied operation as success.
pub fn response(text: &str) -> Result<Value> {
    let reply: Value = serde_json::from_str(text)?;
    if let Some(error) = reply["error"].as_str() {
        bail!("{error}");
    }
    reply
        .get("value")
        .cloned()
        .context("Invalid native file tool response")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    path: String,
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    path: String,
    text: String,
    revision: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
    path: String,
    revision: String,
}

/// Read one bounded request and return one bounded response; never start a process.
pub fn serve() -> Result<()> {
    let mut bytes = vec![];
    std::io::stdin()
        .take(2 * MAX_FILE + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= 2 * MAX_FILE,
        "File tool request exceeds 2 MiB"
    );
    let request: Request = serde_json::from_slice(&bytes)?;
    let result = (|| {
        let root = Root::open(&request.root, &request.protected_configuration)?;
        crate::landlock::confine_agent_files(&root.directory, request.read_only)?;
        execute(&request, &root)
    })();
    let reply = match result {
        Ok(value) => json!({"value":value}),
        Err(error) => json!({"error":format!("{error:#}")}),
    };
    serde_json::to_writer(std::io::stdout().lock(), &reply)?;
    Ok(())
}

/// Dispatch only the native allowlist. Policy applies equally without an outer
/// sandbox; an active sandbox can impose additional read-only/hidden subpaths.
fn execute(request: &Request, root: &Root) -> Result<Value> {
    match request.tool.as_str() {
        "rode_list_files" => {
            let args: PathArgs = serde_json::from_value(request.arguments.clone())?;
            root.list(&root.relative(&args.path, true)?)
        }
        "rode_read_file" => {
            let args: PathArgs = serde_json::from_value(request.arguments.clone())?;
            let path = root.relative(&args.path, false)?;
            let (_, text, revision) = root.read(&path, false)?;
            Ok(json!({"path":path,"text":text,"revision":revision}))
        }
        "rode_search_files" => {
            let args: SearchArgs = serde_json::from_value(request.arguments.clone())?;
            ensure!(
                !args.text.is_empty() && args.text.len() <= 4096,
                "Search text must contain 1–4096 bytes"
            );
            root.search(root.relative(&args.path, true)?, &args.text)
        }
        "rode_write_file" => {
            ensure!(!request.read_only, "The project is read-only");
            let args: WriteArgs = serde_json::from_value(request.arguments.clone())?;
            root.check_mutation(&args.path, request)?;
            ensure!(
                request.arguments.get("revision").is_some(),
                "Supply revision, or null when creating a new file"
            );
            ensure!(
                args.text.len() as u64 <= MAX_FILE && !args.text.contains('\0'),
                "Only UTF-8 text files up to 1 MiB are supported"
            );
            root.write(
                &root.relative(&args.path, false)?,
                &args.text,
                args.revision.as_deref(),
            )
        }
        "rode_delete_file" => {
            ensure!(!request.read_only, "The project is read-only");
            let args: DeleteArgs = serde_json::from_value(request.arguments.clone())?;
            root.check_mutation(&args.path, request)?;
            let path = root.relative(&args.path, false)?;
            let (_lock, _, revision) = root.read(&path, true)?;
            ensure!(
                revision == args.revision,
                "File changed since it was read; read it again before deleting"
            );
            let (parent, name) = root.parent(&path)?;
            ensure!(
                unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } == 0,
                "Cannot delete file: {}",
                std::io::Error::last_os_error()
            );
            Ok(json!({"path":path,"deleted":true}))
        }
        "rode_create_directory" => {
            ensure!(!request.read_only, "The project is read-only");
            let args: PathArgs = serde_json::from_value(request.arguments.clone())?;
            root.check_mutation(&args.path, request)?;
            let path = root.relative(&args.path, false)?;
            let (parent, name) = root.parent(&path)?;
            ensure!(
                unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o755) } == 0,
                "Cannot create directory: {}",
                std::io::Error::last_os_error()
            );
            Ok(json!({"path":path,"created":true}))
        }
        _ => bail!("Unsupported file tool; command execution is unavailable"),
    }
}

/// An opened root, rather than a mutable current working directory, owns all lookups.
struct Root {
    directory: File,
    path: PathBuf,
    configuration: Vec<PathBuf>,
}
impl Root {
    fn check_mutation(&self, path: &str, request: &Request) -> Result<()> {
        let relative = self.relative(path, false)?;
        let absolute = Path::new(&request.root).join(&relative);
        for protected in &request.protected_paths {
            let protected = crate::files::expand_home(protected);
            let protected = if protected.is_absolute() {
                protected
            } else {
                Path::new(&request.root).join(protected)
            };
            let protected = fs::canonicalize(&protected).unwrap_or(protected);
            ensure!(
                !absolute.starts_with(&protected),
                "The active agent executable and configuration are protected from modification"
            );
        }
        Ok(())
    }
    fn open(root: &str, additional_configuration: &[String]) -> Result<Self> {
        ensure!(
            Path::new(root).is_absolute(),
            "A canonical project directory is required"
        );
        let directory = open(
            libc::AT_FDCWD,
            Path::new(root),
            libc::O_PATH | libc::O_DIRECTORY,
            false,
        )?;
        let path = fs::canonicalize(root).context("Cannot resolve the file-tool project")?;
        // The helper's actual environment is authoritative; controller aliases
        // additionally cover a state directory mounted at a different path.
        // Linux std's home lookup also handles HOME-less subprocesses using the
        // account database, matching the backend's normal Unix fallback. The
        // deprecated cross-platform caveat concerns Windows, not this helper.
        #[allow(deprecated)]
        let home = std::env::home_dir();
        let mut configuration_paths = configuration_paths(
            &path,
            std::env::var("CODEX_HOME").ok().as_deref(),
            home.as_deref().and_then(Path::to_str),
            &[],
        )?;
        ensure!(
            !configuration_paths.is_empty(),
            "Cannot protect agent configuration; set an absolute HOME or CODEX_HOME"
        );
        ensure!(
            additional_configuration.len() <= 32,
            "Too many protected configuration paths"
        );
        for additional in additional_configuration {
            configuration_paths.push(additional.clone());
            if let Ok(resolved) = Path::new(additional).canonicalize() {
                configuration_paths.push(resolved.to_string_lossy().into_owned());
            }
        }
        ensure!(
            configuration_paths
                .iter()
                .all(|path| Path::new(path).is_absolute()),
            "Protected configuration paths must be absolute"
        );
        Ok(Self {
            directory,
            path,
            configuration: configuration_paths.into_iter().map(PathBuf::from).collect(),
        })
    }

    fn is_configuration(&self, relative: &Path) -> bool {
        let absolute = self.path.join(relative);
        self.configuration
            .iter()
            .any(|path| absolute.starts_with(path))
    }

    fn relative(&self, path: &str, directory: bool) -> Result<PathBuf> {
        ensure!(
            path.len() <= 4096 && !path.contains('\0'),
            "Invalid project-relative path"
        );
        let mut relative = PathBuf::new();
        for component in Path::new(path).components() {
            match component {
                Component::Normal(name) => {
                    ensure!(
                        name != ".git" && name != ".codex",
                        "Git metadata and Codex configuration are protected"
                    );
                    relative.push(name);
                }
                Component::CurDir => {}
                _ => bail!(
                    "Paths must stay inside the project; absolute paths and '..' are unavailable"
                ),
            }
        }
        ensure!(
            directory || !relative.as_os_str().is_empty(),
            "A file path is required"
        );
        if relative.as_os_str().is_empty() {
            relative.push(".");
        }
        ensure!(
            !self.is_configuration(&relative),
            "Codex configuration and credentials are protected"
        );
        Ok(relative)
    }

    fn file(&self, path: &Path, flags: i32) -> Result<File> {
        open(self.directory.as_raw_fd(), path, flags, true).with_context(|| {
            format!(
                "Cannot access project path {} (symlinks and nested mounts are unavailable)",
                path.display()
            )
        })
    }

    fn read(&self, path: &Path, lock: bool) -> Result<(File, String, String)> {
        self.read_limited(path, lock, &mut (MAX_FILE + 1))
    }

    fn read_limited(
        &self,
        path: &Path,
        lock: bool,
        remaining: &mut u64,
    ) -> Result<(File, String, String)> {
        // O_PATH inspects the inode without opening a device/FIFO. Reopen that
        // exact regular inode through our trusted proc descriptor, not its name.
        let descriptor = self.file(path, libc::O_PATH)?;
        let metadata = descriptor.metadata()?;
        ensure!(
            metadata.is_file(),
            "Only regular project files are available"
        );
        ensure!(metadata.len() <= MAX_FILE, "File exceeds 1 MiB");
        let mut file = File::open(format!("/proc/self/fd/{}", descriptor.as_raw_fd()))?;
        if lock {
            ensure!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
                "File is busy; retry after its current writer finishes"
            );
        }
        let mut bytes = vec![];
        (&mut file)
            .take((MAX_FILE + 1).min(remaining.saturating_add(1)))
            .read_to_end(&mut bytes)?;
        let within_budget = bytes.len() as u64 <= *remaining;
        *remaining = remaining.saturating_sub(bytes.len() as u64);
        ensure!(within_budget, "Search read limit reached");
        ensure!(
            bytes.len() as u64 <= MAX_FILE && !bytes.contains(&0),
            "Only UTF-8 text files up to 1 MiB are available"
        );
        let revision = crate::files::digest(&bytes);
        Ok((
            file,
            String::from_utf8(bytes).context("File is not UTF-8 text")?,
            revision,
        ))
    }

    fn parent(&self, path: &Path) -> Result<(File, CString)> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = self.file(parent, libc::O_PATH | libc::O_DIRECTORY)?;
        let name = CString::new(
            path.file_name()
                .context("A filename is required")?
                .as_encoded_bytes(),
        )?;
        Ok((parent, name))
    }

    fn write(&self, path: &Path, text: &str, expected: Option<&str>) -> Result<Value> {
        let old = if let Some(expected) = expected {
            let old = self.read(path, true)?;
            ensure!(
                old.2 == expected,
                "File changed since it was read; read it again before writing"
            );
            Some(old)
        } else {
            None
        };
        let (parent, name) = self.parent(path)?;
        let directory = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
        let destination = directory.join(std::ffi::OsStr::from_bytes(name.as_bytes()));
        let mut pending = tempfile::Builder::new()
            .prefix(".rode-agent-")
            .tempfile_in(&directory)?;
        pending.write_all(text.as_bytes())?;
        if let Some((file, _, _)) = &old {
            pending
                .as_file()
                .set_permissions(fs::Permissions::from_mode(
                    file.metadata()?.permissions().mode() & 0o777,
                ))?;
            // Check again after preparing the new contents; cooperating writers hold the inode lock.
            ensure!(
                self.read(path, false)?.2 == expected.unwrap(),
                "File changed before publication; read it again"
            );
            pending.persist(&destination).map_err(|error| error.error)?;
        } else {
            pending
                .persist_noclobber(&destination)
                .map_err(|error| error.error)
                .context("File already exists or cannot be created; read it before replacing")?;
        }
        Ok(json!({"path":path,"revision":crate::files::digest(text.as_bytes())}))
    }

    fn entries(&self, path: &Path, remaining: &mut usize) -> Result<(Vec<(String, bool)>, bool)> {
        let directory = self.file(path, libc::O_PATH | libc::O_DIRECTORY)?;
        let mut entries = vec![];
        let mut truncated = false;
        for entry in fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))? {
            if *remaining == 0 {
                truncated = true;
                break;
            }
            *remaining -= 1;
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name == ".git"
                || name == ".codex"
                || name.starts_with(".rode-agent-")
                || self.is_configuration(&path.join(&name))
            {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_file() || kind.is_dir() {
                entries.push((name, kind.is_dir()));
            }
        }
        entries.sort();
        Ok((entries, truncated))
    }

    fn list(&self, path: &Path) -> Result<Value> {
        let mut remaining = MAX_ENTRIES;
        let (entries, truncated) = self.entries(path, &mut remaining)?;
        Ok(
            json!({"entries":entries.into_iter().map(|(name,directory)| json!({"path":path.join(name),"directory":directory})).collect::<Vec<_>>(),"truncated":truncated}),
        )
    }

    fn search(&self, path: PathBuf, needle: &str) -> Result<Value> {
        // A denied or missing starting path is an error, not a successful empty search.
        let start = self.file(&path, libc::O_PATH)?.metadata()?;
        ensure!(
            start.is_file() || start.is_dir(),
            "Only project files and directories can be searched"
        );
        let mut pending = vec![path];
        let mut hits = vec![];
        let mut remaining = MAX_SEARCH_BYTES as u64;
        let mut entries_left = MAX_ENTRIES;
        let mut visited = 0;
        let mut truncated = false;
        while let Some(path) = pending.pop() {
            visited += 1;
            if visited > MAX_ENTRIES || remaining == 0 || hits.len() >= 100 {
                truncated = true;
                break;
            }
            if let Ok((entries, clipped)) = self.entries(&path, &mut entries_left) {
                truncated |= clipped;
                for (name, directory) in entries.into_iter().rev() {
                    if directory
                        && matches!(
                            name.as_str(),
                            "node_modules" | ".venv" | "target" | "__pycache__"
                        )
                    {
                        continue;
                    }
                    if pending.len() < MAX_ENTRIES {
                        pending.push(path.join(name));
                    } else {
                        truncated = true;
                    }
                }
            } else if let Ok((_, text, _)) = self.read_limited(&path, false, &mut remaining) {
                for (index, line) in text.lines().enumerate() {
                    if line.contains(needle) {
                        hits.push(json!({"path":path,"line":index+1,"text":line.chars().take(512).collect::<String>()}));
                        if hits.len() == 100 {
                            truncated = true;
                            break;
                        }
                    }
                }
            }
        }
        truncated |= remaining == 0;
        Ok(json!({"matches":hits,"truncated":truncated}))
    }
}

use std::os::unix::ffi::OsStrExt;

/// No fallback to canonicalize-then-open: that would reintroduce symlink races.
fn open(directory: RawFd, path: &Path, flags: i32, beneath: bool) -> Result<File> {
    let path = CString::new(path.as_os_str().as_encoded_bytes())?;
    // SAFETY: open_how contains only integer fields; zero initializes its ABI tail too.
    let mut how = unsafe { std::mem::MaybeUninit::<libc::open_how>::zeroed().assume_init() };
    how.flags = (flags | libc::O_CLOEXEC | libc::O_NOFOLLOW) as u64;
    how.resolve = libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;
    if beneath {
        how.resolve |= libc::RESOLVE_BENEATH | libc::RESOLVE_NO_XDEV;
    }
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory,
            path.as_ptr(),
            &how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if descriptor == -1 {
        let error = std::io::Error::last_os_error();
        return if matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL)) {
            Err(error).context("Safe file access needs Linux openat2 support")
        } else {
            Err(error.into())
        };
    }
    // SAFETY: a successful openat2 returns a new descriptor owned by this File.
    Ok(unsafe { File::from_raw_fd(descriptor as RawFd) })
}

#[cfg(test)]
mod tests;

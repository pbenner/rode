//! Policy-scoped, bounded-memory copies. A channel owns its file handles and
//! unpublished stage; EOF, errors and cancellation drop that stage. Publication
//! checks the source digest, length and captured destination identity.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use rode_host_protocol::{
    framing,
    transfer::{BLOCK, Entry, Reply, Request},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CStr, CString},
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

#[cfg(test)]
mod tests;

fn version(metadata: &fs::Metadata) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec()
    )
}

fn inspect(path: &Path) -> Result<Option<Entry>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some(Entry {
        name: path
            .file_name()
            .context("Missing filename")?
            .to_str()
            .context("File names must be UTF-8")?
            .into(),
        directory: metadata.is_dir(),
        regular: metadata.is_file(),
        size: if metadata.is_file() {
            metadata.len()
        } else {
            0
        },
        executable: metadata.mode() & 0o100 != 0,
        version: version(&metadata),
    }))
}

fn path(value: &str) -> Result<PathBuf> {
    ensure!(
        !value.is_empty() && value.len() <= 4096 && !value.contains('\0'),
        "Invalid transfer path"
    );
    Ok(crate::files::expand_home(value))
}

struct Source {
    file: File,
    version: String,
    hash: Sha256,
}
struct Stage {
    file: File,
    parent: File,
    temporary: CString,
    name: CString,
    destination: PathBuf,
    expected: Option<String>,
    hash: Sha256,
    size: u64,
}
impl Drop for Stage {
    fn drop(&mut self) {
        let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                self.parent.as_raw_fd(),
                self.temporary.as_ptr(),
                &mut metadata,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
            && self
                .file
                .metadata()
                .is_ok_and(|file| file.ino() == metadata.st_ino && file.dev() == metadata.st_dev)
        {
            unsafe { libc::unlinkat(self.parent.as_raw_fd(), self.temporary.as_ptr(), 0) };
        }
    }
}

// Pin the actual directory before using relative *at operations. Never follow
// a newly substituted parent alias, even in normal host mode.
fn anchored_parent(path: &Path) -> Result<(File, CString)> {
    let parent = path.parent().context("Missing parent directory")?;
    ensure!(
        parent.is_absolute() && parent.canonicalize()? == parent,
        "Directory changed or is an alias; refresh its listing before transferring"
    );
    let expected = fs::metadata(parent)?;
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(parent)?;
    let opened = directory.metadata()?;
    ensure!(
        opened.ino() == expected.ino() && opened.dev() == expected.dev(),
        "Transfer directory changed while opening"
    );
    Ok((
        directory,
        CString::new(
            path.file_name()
                .context("Missing filename")?
                .as_encoded_bytes(),
        )?,
    ))
}
fn open_at(parent: &File, name: &CStr, flags: i32) -> std::io::Result<File> {
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn destination_at(parent: &File, name: &CStr, expected: Option<&str>) -> Result<Option<File>> {
    match open_at(parent, name, libc::O_RDONLY) {
        Ok(file) => {
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_file() && expected == Some(version(&metadata).as_str()),
                "Destination changed or already exists; refresh and choose again"
            );
            Ok(Some(file))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && expected.is_none() => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

/// One independently owned file stream; ordinary local copies use the same implementation.
#[derive(Default)]
pub struct Session {
    source: Option<Source>,
    stage: Option<Stage>,
}
impl Session {
    /// Execute one bounded request. An error abandons the current destination stage.
    pub fn request(&mut self, request: &Request) -> Result<Reply> {
        let result = self.execute(request);
        if result.is_err() {
            self.stage = None;
            self.source = None;
        }
        result
    }
    fn execute(&mut self, request: &Request) -> Result<Reply> {
        let mut reply = Reply::default();
        match request {
            Request::Ping => {}
            Request::Stat { path: value } => {
                reply.entry = inspect(&path(value)?)?;
            }
            Request::List { path: value } => {
                let directory = path(value)?.canonicalize()?;
                ensure!(fs::metadata(&directory)?.is_dir(), "Not a directory");
                for child in fs::read_dir(&directory)? {
                    if let Some(entry) = inspect(&child?.path())? {
                        reply.entries.push(entry);
                    }
                    ensure!(
                        reply.entries.len() <= crate::files::MAX_ENTRIES,
                        "Directory has too many entries; choose a smaller directory"
                    );
                }
                reply
                    .entries
                    .sort_by_key(|entry| (!entry.directory, entry.name.to_lowercase()));
                reply.path = directory.to_str().context("Paths must be UTF-8")?.into();
            }
            Request::Directory { path: value } => {
                let directory = path(value)?;
                let (parent, name) = anchored_parent(&directory)?;
                if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        open_at(&parent, &name, libc::O_RDONLY | libc::O_DIRECTORY)?;
                    } else {
                        return Err(error.into());
                    }
                }
            }
            Request::OpenRead {
                path: value,
                version: expected,
            } => {
                let source = path(value)?;
                let (parent, name) = anchored_parent(&source)?;
                let file = open_at(&parent, &name, libc::O_RDONLY)?;
                let metadata = file.metadata()?;
                ensure!(
                    metadata.is_file() && version(&metadata) == *expected,
                    "Source changed or is not a regular file; refresh and retry"
                );
                self.source = Some(Source {
                    file,
                    version: expected.clone(),
                    hash: Sha256::new(),
                });
            }
            Request::Read => {
                let source = self.source.as_mut().context("No source stream")?;
                ensure!(
                    version(&source.file.metadata()?) == source.version,
                    "Source changed during transfer"
                );
                let mut block = vec![0; BLOCK];
                let count = source.file.read(&mut block)?;
                source.hash.update(&block[..count]);
                reply.data = STANDARD.encode(&block[..count]);
                if count == 0 {
                    ensure!(
                        version(&source.file.metadata()?) == source.version,
                        "Source changed during transfer"
                    );
                    reply.digest = format!("{:x}", source.hash.clone().finalize());
                    self.source = None;
                }
            }
            Request::OpenWrite {
                path: value,
                expected,
            } => {
                self.stage = None;
                let destination = path(value)?;
                let (parent, name) = anchored_parent(&destination)?;
                destination_at(&parent, &name, expected.as_deref())?;
                // tempfile-like create_new ownership; collision never removes another file.
                let mut stage = None;
                for _ in 0..8 {
                    let mut random = [0u8; 16];
                    File::open("/dev/urandom")?.read_exact(&mut random)?;
                    let temporary = CString::new(format!(
                        ".rode-transfer-{}.part",
                        crate::files::digest(&random)
                    ))?;
                    match open_at(
                        &parent,
                        &temporary,
                        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                    ) {
                        Ok(file) => {
                            stage = Some(Stage {
                                file,
                                parent,
                                name: name.clone(),
                                temporary,
                                destination: destination.clone(),
                                expected: expected.clone(),
                                hash: Sha256::new(),
                                size: 0,
                            });
                            break;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                self.stage = Some(stage.context("Cannot create a private transfer stage")?);
            }
            Request::Write { data } => {
                ensure!(
                    data.len() <= BLOCK.div_ceil(3) * 4,
                    "Oversized transfer block"
                );
                let block = STANDARD.decode(data)?;
                ensure!(block.len() <= BLOCK, "Oversized transfer block");
                let stage = self.stage.as_mut().context("No destination stream")?;
                stage.file.write_all(&block)?;
                stage.hash.update(&block);
                stage.size = stage
                    .size
                    .checked_add(block.len() as u64)
                    .context("Transfer size overflow")?;
            }
            Request::Commit {
                size,
                digest,
                executable,
            } => {
                let stage = self.stage.take().context("No destination stream")?;
                let hash = format!("{:x}", stage.hash.clone().finalize());
                ensure!(
                    stage.size == *size && hash == *digest,
                    "Incomplete or corrupted transfer; destination preserved"
                );
                let original = stage
                    .destination
                    .parent()
                    .context("Missing transfer parent")?;
                ensure!(
                    original.canonicalize()? == original,
                    "Transfer directory was redirected"
                );
                let current = fs::metadata(original)?;
                let pinned = stage.parent.metadata()?;
                ensure!(
                    current.ino() == pinned.ino() && current.dev() == pinned.dev(),
                    "Transfer directory was replaced"
                );
                let existing =
                    destination_at(&stage.parent, &stage.name, stage.expected.as_deref())?;
                if let Some(file) = &existing {
                    crate::locking::lock(file)?;
                }
                destination_at(&stage.parent, &stage.name, stage.expected.as_deref())?;
                if let Some(file) = &existing {
                    stage.file.set_permissions(file.metadata()?.permissions())?;
                } else {
                    use std::os::unix::fs::PermissionsExt;
                    stage
                        .file
                        .set_permissions(fs::Permissions::from_mode(if *executable {
                            0o700
                        } else {
                            0o600
                        }))?;
                }
                stage.file.sync_all()?;
                let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
                ensure!(
                    unsafe {
                        libc::fstatat(
                            stage.parent.as_raw_fd(),
                            stage.temporary.as_ptr(),
                            &mut metadata,
                            libc::AT_SYMLINK_NOFOLLOW,
                        )
                    } == 0
                        && metadata.st_ino == stage.file.metadata()?.ino()
                        && metadata.st_dev == stage.file.metadata()?.dev(),
                    "Transfer stage was replaced"
                );
                if stage.expected.is_some() {
                    ensure!(
                        unsafe {
                            libc::renameat(
                                stage.parent.as_raw_fd(),
                                stage.temporary.as_ptr(),
                                stage.parent.as_raw_fd(),
                                stage.name.as_ptr(),
                            )
                        } == 0,
                        "Cannot publish transfer: {}",
                        std::io::Error::last_os_error()
                    );
                } else {
                    ensure!(
                        unsafe {
                            libc::linkat(
                                stage.parent.as_raw_fd(),
                                stage.temporary.as_ptr(),
                                stage.parent.as_raw_fd(),
                                stage.name.as_ptr(),
                                0,
                            )
                        } == 0,
                        "Destination appeared or cannot be published; original preserved: {}",
                        std::io::Error::last_os_error()
                    );
                }
                reply.path = stage.destination.to_string_lossy().into();
                reply.digest = hash;
            }
            Request::Abort => {
                self.stage = None;
                self.source = None;
            }
        }
        Ok(reply)
    }
}

/// Run a dedicated helper in its captured policy. The client waits for the
/// worker's ready reply before sending any bytes after this bounded plan header.
pub fn launch() -> Result<()> {
    crate::supervisor::signals()?;
    let mut bytes = Vec::new();
    use std::io::BufRead;
    std::io::stdin()
        .lock()
        .take(128 * 1024 + 1)
        .read_until(b'\n', &mut bytes)?;
    ensure!(
        bytes.len() <= 128 * 1024 && bytes.last() == Some(&b'\n'),
        "Invalid transfer policy header"
    );
    let mut plan: crate::sandbox::Plan = serde_json::from_slice(&bytes)?;
    ensure!(
        Path::new(&plan.root).is_absolute(),
        "Transfer policy requires an absolute project root"
    );
    ensure!(
        !plan.active || plan.prefix.len() >= 2 && plan.prefix.last().is_some_and(|arg| arg == "--"),
        "Invalid transfer sandbox plan"
    );
    plan.environment.validate()?;
    plan.native_helper = Some(std::env::current_exe()?.to_string_lossy().into());
    plan.environment.mise = false;
    let (mut command, _lease) = plan.coordinator_command("--file-transfer-worker")?;
    let status = command.status()?;
    ensure!(status.success(), "Transfer worker exited with {status}");
    Ok(())
}

/// Serve sequential bounded requests. EOF drops any unpublished file; no
/// listener, persistent service or automatic reconnect is created.
pub fn serve() -> Result<()> {
    crate::supervisor::signals()?;
    let mut input = crate::connection::Input::stdin()?;
    let mut output = crate::connection::Output::stdout()?;
    serve_stream(&mut input, &mut output, &crate::command::CANCELLED)
}

fn serve_stream(
    input: &mut impl Read,
    output: &mut impl Write,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    let mut session = Session::default();
    framing::write(output, &Ok::<_, String>(Reply::default()))?;
    while let Some(request) = framing::read::<Request>(input)? {
        ensure!(
            !cancel.load(std::sync::atomic::Ordering::Acquire),
            "Transfer cancelled"
        );
        let result = session
            .request(&request)
            .map_err(|error| format!("{error:#}"));
        framing::write(output, &result)?;
    }
    Ok(())
}

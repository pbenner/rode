//! Rootless, verified deployment of explicitly selected native runtime artifacts.
//! The desktop supplies trusted pins and bytes; this module never downloads or authenticates.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{BufRead, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Shared release metadata and immutable cache identity for desktop and host.
pub mod pins;
use pins::{Notice, Target, valid_sha256, validate_version};

#[derive(Deserialize)]
struct Specification {
    action: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    targets: BTreeMap<String, Target>,
    #[serde(default)]
    notices: Vec<Notice>,
    #[serde(default)]
    notice_data: BTreeMap<String, String>,
}
#[derive(Serialize)]
struct Reply {
    architecture: &'static str,
    platform: &'static str,
    installed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached: Option<bool>,
}
fn private_directory(path: &Path, create: bool) -> Result<bool> {
    if create {
        use std::os::unix::fs::DirBuilderExt;
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0,
        "Unsafe Codex cache directory"
    );
    Ok(true)
}
fn verified(path: &Path, size: Option<u64>, digest: &str) -> Result<bool> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let metadata = file.metadata()?;
    let limit = size.unwrap_or(1024 * 1024);
    ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.nlink() == 1
            && metadata.mode() & 0o022 == 0
            && metadata.len() <= limit
            && size.is_none_or(|n| n == metadata.len()),
        "Unsafe Codex runtime file"
    );
    let mut hash = Sha256::new();
    let mut input = file.take(limit + 1);
    let mut buffer = [0; 64 * 1024];
    let mut total = 0;
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(total <= limit, "Cached Codex exceeds its pinned size");
        hash.update(&buffer[..n]);
    }
    ensure!(
        format!("{:x}", hash.finalize()) == digest,
        "Cached Codex failed its SHA-256 check"
    );
    Ok(true)
}
fn cached(levels: &[PathBuf], target: &Target, notices: &[Notice]) -> Result<bool> {
    for path in levels {
        if !private_directory(path, false)? {
            return Ok(false);
        }
    }
    let runtime = levels.last().context("Missing runtime directory")?;
    let expected: BTreeSet<_> = target
        .artifacts()
        .iter()
        .map(|(name, _, _)| (*name).to_owned())
        .chain(notices.iter().map(|n| n.file.clone()))
        .collect();
    let actual = fs::read_dir(runtime)?
        .take(expected.len() + 1)
        .map(|e| {
            e?.file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Invalid runtime filename"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    ensure!(actual == expected, "Unexpected Codex runtime files");
    for (name, size, digest) in target.artifacts() {
        ensure!(
            verified(&runtime.join(name), Some(size), digest)?,
            "Missing cached Codex executable"
        );
        ensure!(
            fs::metadata(runtime.join(name))?.mode() & 0o111 != 0,
            "Cached Codex is not executable"
        );
    }
    for notice in notices {
        ensure!(
            verified(&runtime.join(&notice.file), None, &notice.sha256)?,
            "Missing cached Codex notice"
        );
    }
    Ok(true)
}
fn run(spec: Specification, input: &mut impl Read) -> Result<Reply> {
    let mut reply = Reply {
        architecture: std::env::consts::ARCH,
        platform: std::env::consts::OS,
        installed: crate::sandbox::find_program("codex")
            .or_else(|| {
                let path = crate::files::expand_home("~/.local/bin/codex");
                path.metadata()
                    .is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0)
                    .then_some(path)
            })
            .map(|p| p.to_string_lossy().into_owned()),
        executable: None,
        cached: None,
    };
    if spec.action == "host" {
        return Ok(reply);
    }
    ensure!(
        matches!(spec.action.as_str(), "probe" | "install"),
        "Invalid Codex deployment action"
    );
    let target = match spec
        .targets
        .get(std::env::consts::ARCH)
        .filter(|_| cfg!(target_os = "linux"))
    {
        Some(target) => target,
        None if spec.action == "probe" => return Ok(reply),
        None => anyhow::bail!("This host needs a configured Codex executable for its architecture"),
    };
    validate_version(&spec.version)?;
    target.validate()?;
    ensure!(spec.notices.len() <= 32, "Too many Codex notices");
    let mut names = BTreeSet::new();
    for notice in &spec.notices {
        ensure!(
            !notice.file.is_empty()
                && notice.file.len() <= 255
                && !notice.file.contains(['/', '\\', '\0'])
                && !matches!(notice.file.as_str(), "." | ".." | "manifest.json")
                && !target.artifacts().iter().any(|(n, _, _)| *n == notice.file)
                && valid_sha256(&notice.sha256)
                && names.insert(&notice.file),
            "Invalid Codex notice filename or pin"
        );
    }
    let base = crate::files::expand_home(
        &std::env::var("RODE_CODEX_CACHE").unwrap_or_else(|_| "~/.cache/rode/codex".into()),
    );
    ensure!(base.is_absolute(), "Codex cache path must be absolute");
    crate::paths::trusted_ancestors(&base).context("Unsafe Codex cache directory ancestors")?;
    let version = base.join(&spec.version);
    let folder = version.join(std::env::consts::ARCH);
    let key = target.cache_key()?;
    let runtime = folder.join(&key);
    let levels = [base.clone(), version, folder.clone(), runtime.clone()];
    reply.executable = Some(runtime.join("codex").to_string_lossy().into_owned());
    reply.cached = Some(cached(&levels, target, &spec.notices)?);
    if spec.action == "probe" {
        return Ok(reply);
    }
    let parent = base.parent().context("Missing Codex cache parent")?;
    fs::create_dir_all(parent)?;
    let metadata = parent.metadata()?;
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
        "Unsafe Codex cache parent"
    );
    for directory in &levels[..3] {
        private_directory(directory, true)?;
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(folder.join(format!("{key}.lock")))?;
    let metadata = lock.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.nlink() == 1
            && metadata.mode() & 0o077 == 0,
        "Unsafe Codex deployment lock"
    );
    crate::locking::lock_with_timeout(&lock, std::time::Duration::from_secs(30), || {
        crate::command::CANCELLED.load(std::sync::atomic::Ordering::Acquire)
    })?;
    let stage = tempfile::Builder::new()
        .prefix(".incoming-")
        .tempdir_in(&folder)?;
    let staged_runtime = stage.path().join("runtime");
    private_directory(&staged_runtime, true)?;
    for (name, size, digest) in target.artifacts() {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(staged_runtime.join(name))?;
        let mut hash = Sha256::new();
        let mut remaining = size;
        let mut buffer = [0; 64 * 1024];
        while remaining > 0 {
            ensure!(
                !crate::command::CANCELLED.load(std::sync::atomic::Ordering::Acquire),
                "Codex deployment cancelled"
            );
            let capacity = remaining.min(buffer.len() as u64) as usize;
            let n = input.read(&mut buffer[..capacity])?;
            ensure!(n > 0, "Incomplete Codex transfer");
            output.write_all(&buffer[..n])?;
            hash.update(&buffer[..n]);
            remaining -= n as u64;
        }
        output.sync_all()?;
        ensure!(
            format!("{:x}", hash.finalize()) == digest,
            "Codex transfer failed its pinned size/SHA-256 check: {name}"
        );
        output.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    ensure!(
        input.read(&mut [0])? == 0,
        "Codex transfer exceeds its pinned size"
    );
    for notice in &spec.notices {
        let text = spec
            .notice_data
            .get(&notice.file)
            .context("Missing Codex notice content")?;
        ensure!(
            text.len() <= 1024 * 1024 && crate::files::digest(text.as_bytes()) == notice.sha256,
            "Codex notice failed its SHA-256 check"
        );
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(staged_runtime.join(&notice.file))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    if !cached(&levels, target, &spec.notices)? {
        ensure!(!runtime.try_exists()?, "Unexpected Codex runtime directory");
        fs::rename(&staged_runtime, &runtime)?;
        crate::locking::open_directory(&folder)?.sync_all()?;
    }
    Ok(reply)
}

/// Metadata is a bounded stdin line; subsequent bytes are the pinned artifact stream.
pub fn serve() -> Result<()> {
    crate::supervisor::signals()?;
    let flags = unsafe { libc::fcntl(0, libc::F_GETFL) };
    ensure!(
        flags >= 0 && unsafe { libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "Cannot configure runtime upload input"
    );
    let mut input = std::io::BufReader::new(UploadInput {
        until: std::time::Instant::now() + std::time::Duration::from_secs(180),
    });
    let mut bytes = vec![];
    input
        .by_ref()
        .take(96 * 1024 + 1)
        .read_until(b'\n', &mut bytes)?;
    ensure!(
        bytes.len() <= 96 * 1024 && bytes.last() == Some(&b'\n'),
        "Invalid runtime deployment metadata"
    );
    let response = run(serde_json::from_slice(&bytes)?, &mut input)?;
    serde_json::to_writer(std::io::stdout().lock(), &response)?;
    Ok(())
}

// Interrupted reads in stdio normally retry automatically. Poll nonblocking input
// here so cancellation also cleans staging files when a sender stops mid-upload.
struct UploadInput {
    until: std::time::Instant,
}
impl Read for UploadInput {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if crate::command::CANCELLED.load(std::sync::atomic::Ordering::Acquire)
                || std::time::Instant::now() >= self.until
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Runtime upload cancelled or timed out",
                ));
            }
            let n = unsafe { libc::read(0, bytes.as_mut_ptr().cast(), bytes.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let error = std::io::Error::last_os_error();
            if !matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ) {
                return Err(error);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

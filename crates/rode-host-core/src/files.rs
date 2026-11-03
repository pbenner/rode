//! Bounded file operations shared by the local and remote workspace backends.
//!
//! # Design decisions
//!
//! Content digests identify the bytes a buffer was based on; timestamp/size
//! checks alone can miss an external edit with unchanged metadata. Cooperating
//! writers lock and verify the current inode before publishing a replacement.
//! Source directories need no permanent lock files, which avoids leaving editor
//! artifacts in repositories; configuration uses a different stable-lock scheme
//! because merging shared preferences must also cover destination creation.
//!
//! Saves acknowledge their submitted snapshot, not the buffer at completion, so
//! typing during I/O remains unsaved rather than being incorrectly marked clean.
//! Symlink targets are resolved before replacement to preserve the link itself.
//! These rules coordinate RODE's local and remote writers; external programs remain
//! subject to best-effort conflict detection rather than a universal file lock.
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub use rode_host_protocol::files::{
    Entry, MAX_ENTRIES, MAX_FILE_BYTES, MAX_PREVIEW_BYTES, Response,
};
/// Concrete shared file envelope for headless service callers.
pub type Request =
    rode_host_protocol::files::Request<crate::project::Request, crate::sandbox::Options>;
use rode_host_protocol::files::Request as Envelope;

/// Return the lowercase hexadecimal SHA-256 used as an optimistic file revision.
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Expand only `~` and `~/` using this process's home; do not evaluate shell syntax.
pub fn expand_home(path: &str) -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        if path == "~" {
            return PathBuf::from(home);
        }
        if let Some(rest) = path.strip_prefix("~/") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// Execute a blocking, bounded host file request. Resolves symlinks before saving, locks
/// cooperating writers, compares revisions, and publishes through a neighboring temporary
/// inode. Call off the UI thread or inside the prepared helper boundary.
pub fn dispatch<T, S>(request: &Envelope<T, S>) -> Result<Response> {
    let path = match request {
        Envelope::ReadAuto { path }
        | Envelope::ReadHex { path }
        | Envelope::WriteBinary { path, .. }
        | Envelope::List { path }
        | Envelope::Read { path }
        | Envelope::ReadImage { path }
        | Envelope::ReadBinary { path }
        | Envelope::Write { path, .. } => expand_home(path),
        Envelope::Prepare { .. } | Envelope::Tool { .. } => bail!("Not a filesystem operation"),
    };
    let path = if matches!(
        request,
        Envelope::Write { .. } | Envelope::WriteBinary { .. }
    ) && !path.exists()
    {
        let parent = path
            .parent()
            .context("File has no parent directory")?
            .canonicalize()?;
        parent.join(path.file_name().context("Missing filename")?)
    } else {
        path.canonicalize()
            .with_context(|| format!("Cannot open {}", path.display()))?
    };
    let mut response = Response {
        path: path.to_string_lossy().into(),
        ..Default::default()
    };
    match request {
        Envelope::Prepare { .. } | Envelope::Tool { .. } => bail!("Not a filesystem operation"),
        Envelope::List { .. } => {
            for entry in fs::read_dir(&path)? {
                let entry = entry?;
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                response.entries.push(Entry {
                    path: entry.path().to_string_lossy().into(),
                    name,
                    dir: entry.path().is_dir(),
                });
                if response.entries.len() > MAX_ENTRIES {
                    bail!("Directory has more than {MAX_ENTRIES} entries. Open a smaller folder.");
                }
            }
            response
                .entries
                .sort_by_key(|e| (!e.dir, e.name.to_lowercase()));
        }
        Envelope::ReadAuto { .. } | Envelope::ReadHex { .. } => {
            use base64::{Engine, engine::general_purpose::STANDARD};
            let bytes = read_bounded(&path)?;
            response.revision = digest(&bytes);
            if !matches!(request, Envelope::ReadHex { .. })
                && !bytes.contains(&0)
                && std::str::from_utf8(&bytes).is_ok()
            {
                response.text = String::from_utf8(bytes)?;
            } else {
                response.binary = Some(STANDARD.encode(bytes));
            }
        }
        Envelope::Read { .. } => {
            let bytes = read_bounded(&path)?;
            if bytes.contains(&0) {
                bail!("Binary files cannot be edited.");
            }
            response.revision = digest(&bytes);
            response.text =
                String::from_utf8(bytes).context("Only UTF-8 text files are supported")?;
        }
        Envelope::ReadImage { .. } | Envelope::ReadBinary { .. } => {
            let kind = if matches!(request, Envelope::ReadBinary { .. }) {
                "Preview"
            } else {
                "Image"
            };
            use base64::{Engine, engine::general_purpose::STANDARD};
            use std::io::Read;
            anyhow::ensure!(
                fs::metadata(&path)?.is_file(),
                "Only regular preview files can be opened."
            );
            let file = crate::locking::open_regular(&path, false)?;
            let metadata = file.metadata()?;
            anyhow::ensure!(
                metadata.is_file(),
                "Only regular image files can be opened."
            );
            anyhow::ensure!(
                metadata.len() <= MAX_PREVIEW_BYTES as u64,
                "{kind} file exceeds 32 MiB."
            );
            let mut bytes = Vec::new();
            file.take(MAX_PREVIEW_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() <= MAX_PREVIEW_BYTES,
                "{kind} file exceeds 32 MiB."
            );
            response.revision = digest(&bytes);
            if matches!(request, Envelope::ReadBinary { .. }) {
                response.binary = Some(STANDARD.encode(bytes));
            } else {
                response.image = Some(STANDARD.encode(bytes));
            }
        }
        Envelope::Write { expected, .. } | Envelope::WriteBinary { expected, .. } => {
            use base64::{Engine, engine::general_purpose::STANDARD};
            let content = match request {
                Envelope::Write { text, .. } => text.as_bytes().to_vec(),
                Envelope::WriteBinary { data, .. } => {
                    anyhow::ensure!(
                        data.len() <= (MAX_FILE_BYTES as usize).div_ceil(3) * 4,
                        "Binary file exceeds 8 MiB."
                    );
                    STANDARD.decode(data).context("Invalid binary save data")?
                }
                _ => unreachable!(),
            };
            if content.len() as u64 > MAX_FILE_BYTES {
                bail!("File exceeds the 8 MiB editing limit.");
            }
            let existing = crate::locking::lock_existing(&path)?;
            let permissions = if let Some(file) = &existing {
                let bytes = read_bounded(&path)?;
                if expected.as_deref() != Some(digest(&bytes).as_str()) {
                    bail!(
                        "Save conflict: this file changed on disk. Your edits are still open; use Save As to keep a separate copy, or close and reopen to load the disk version."
                    );
                }
                Some(file.metadata()?.permissions())
            } else {
                if expected.is_some() {
                    bail!(
                        "Save conflict: this file was removed on disk. Your edits are still open."
                    );
                }
                None
            };
            atomic_write(&path, &content, permissions)?;
            response.revision = digest(&content);
        }
    }
    Ok(response)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = crate::locking::open_regular(path, false)?;
    if !file.metadata()?.is_file() {
        bail!("Only regular files can be edited.");
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bail!("File exceeds the 8 MiB editing limit.");
    }
    Ok(bytes)
}

fn atomic_write(path: &Path, bytes: &[u8], permissions: Option<fs::Permissions>) -> Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // Sandboxed helpers can all have the same PID and a fresh counter. Include
    // the destination and time so parallel saves in one directory do not collide.
    let destination = digest(path.as_os_str().as_encoded_bytes());
    let tmp = path.with_file_name(format!(
        ".rode-{}-{}-{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        &destination[..16],
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    ));
    write_temporary(path, &tmp, bytes, permissions)
}

fn write_temporary(
    path: &Path,
    tmp: &Path,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Only clean up files this save actually created. A name collision must
    // leave the existing temporary file (and the destination) intact.
    let mut file = options.open(tmp)?;
    let overwrite = permissions.is_some();
    let result = (|| -> Result<()> {
        crate::locking::lock(&file)?;
        file.write_all(bytes)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        if overwrite {
            fs::rename(tmp, path)?;
        } else {
            fs::hard_link(tmp,path).context("Save conflict: another process created this file. Your edits are still open; use Save As.")?;
            fs::remove_file(tmp)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

/// Execute a filesystem-only request on the current host.
pub fn local(request: &Request) -> Result<Response> {
    dispatch(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temporary_name_collision_does_not_remove_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.py");
        let tmp = directory.path().join("pending.tmp");
        fs::write(&path, "original").unwrap();
        fs::write(&tmp, "another save").unwrap();
        assert!(write_temporary(&path, &tmp, b"replacement", None).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "original");
        assert_eq!(fs::read_to_string(tmp).unwrap(), "another save");
    }
}

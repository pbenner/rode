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
//! These rules coordinate RODE's native/Python writers; external programs remain
//! subject to best-effort conflict detection rather than a universal file lock.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Maximum text/hex file size in bytes; image/PDF transport has its own limit.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum retained entries in one nonrecursive directory listing.
pub const MAX_ENTRIES: usize = 10_000;

/// One directory entry on the operation's host.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Entry {
    /// Host-side path to the entry.
    pub path: String,
    /// Filename used for display, without a file-type prefix.
    pub name: String,
    /// Whether following this entry resolves to a directory.
    pub dir: bool,
}

/// Serde-tagged file protocol shared with `remote_files.py`. Writes use content revisions, not
/// timestamps. Dispatch through a sandbox plan when project isolation is active; [`local`]
/// itself has no sandbox boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Run a project-host service through the file channel.
    Tool {
        /// Project service request payload.
        request: crate::project::Request,
    },
    /// Canonicalize a project root and probe the requested sandbox policy.
    Prepare {
        /// Project directory on the chosen host.
        path: String,
        /// Requested access policy; the returned plan records actual availability.
        sandbox: crate::sandbox::Options,
    },
    /// Read one directory without recursive traversal.
    List {
        /// Directory path on the chosen host.
        path: String,
    },
    /// Read bounded bytes, returning UTF-8 text or base64 binary according to content.
    ReadAuto {
        /// Source file path on the chosen host.
        path: String,
    },
    /// Read bounded bytes explicitly as hex-editor data.
    ReadHex {
        /// Source file path on the chosen host.
        path: String,
    },
    /// Atomically save binary bytes, preserving revision and permission protections.
    WriteBinary {
        /// Destination path on the chosen host.
        path: String,
        /// Standard base64-encoded replacement bytes.
        data: String,
        /// SHA-256 revision required for replacement; `None` means create without overwriting.
        expected: Option<String>,
    },
    /// Read a UTF-8 text file, rejecting NUL-containing data.
    Read {
        /// Source file path on the chosen host.
        path: String,
    },
    /// Transfer encoded raster bytes without remote decoding.
    ReadImage {
        /// Image path on the chosen host.
        path: String,
    },
    /// Transfer preview bytes, including PDFs, without remote rendering.
    ReadBinary {
        /// Preview source path on the chosen host.
        path: String,
    },
    /// Atomically save UTF-8 source using the same conflict rules as binary saves.
    Write {
        /// Destination path on the chosen host.
        path: String,
        /// Replacement source text.
        text: String,
        /// SHA-256 revision required for replacement; `None` means create without overwriting.
        expected: Option<String>,
    },
}

/// File-channel reply plus desktop-only decoded attachments. Unused wire fields retain
/// defaults; fields marked `serde(skip)` never cross SSH.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Response {
    /// Canonical host path associated with the operation.
    pub path: String,
    /// Sorted immediate children for a directory listing.
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// UTF-8 source contents for a text read.
    #[serde(default)]
    pub text: String,
    /// SHA-256 of the bytes read or successfully saved.
    #[serde(default)]
    pub revision: String,
    /// Actual sandbox plan returned by project preparation.
    #[serde(default)]
    pub project: Option<crate::sandbox::Plan>,
    /// Project-service result when the request was a tool operation.
    #[serde(default)]
    pub tool: Option<crate::project::Response>,
    /// Standard base64-encoded raster bytes, before desktop decoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Standard base64-encoded binary/preview bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    /// Desktop-only rendered PDF and retained source snapshot.
    #[serde(skip)]
    pub pdf: Option<crate::pdf::Loaded>,
    /// Desktop-only decoded raster pixels and budget lease.
    #[serde(skip)]
    pub preview: Option<crate::images::Decoded>,
    /// Desktop-only decoded binary snapshot and static report.
    #[serde(skip)]
    pub hex: Option<crate::binary::Loaded>,
    /// Desktop-only notebook parse result; failure preserves access to source JSON.
    #[serde(skip)]
    pub notebook: Option<Result<crate::notebook::Notebook, String>>,
}

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
pub fn local(request: &Request) -> Result<Response> {
    if let Request::Tool { request } = request {
        return Ok(Response {
            path: String::new(),
            entries: vec![],
            text: String::new(),
            revision: String::new(),
            project: None,
            tool: Some(crate::project::local(request)?),
            image: None,
            binary: None,
            pdf: None,
            preview: None,
            hex: None,
            notebook: None,
        });
    }
    if let Request::Prepare { path, sandbox } = request {
        let plan = crate::sandbox::prepare(path, sandbox)?;
        let mut response = local(&Request::List {
            path: plan.root.clone(),
        })?;
        response.project = Some(plan);
        return Ok(response);
    }
    let path = match request {
        Request::ReadAuto { path }
        | Request::ReadHex { path }
        | Request::WriteBinary { path, .. }
        | Request::List { path }
        | Request::Read { path }
        | Request::ReadImage { path }
        | Request::ReadBinary { path }
        | Request::Write { path, .. } => expand_home(path),
        Request::Prepare { .. } | Request::Tool { .. } => unreachable!(),
    };
    let path = if matches!(request, Request::Write { .. } | Request::WriteBinary { .. })
        && !path.exists()
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
        entries: vec![],
        text: String::new(),
        revision: String::new(),
        project: None,
        tool: None,
        image: None,
        binary: None,
        pdf: None,
        preview: None,
        hex: None,
        notebook: None,
    };
    match request {
        Request::Prepare { .. } | Request::Tool { .. } => unreachable!(),
        Request::List { .. } => {
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
        Request::ReadAuto { .. } | Request::ReadHex { .. } => {
            use base64::{Engine, engine::general_purpose::STANDARD};
            let bytes = read_bounded(&path)?;
            response.revision = digest(&bytes);
            if !matches!(request, Request::ReadHex { .. })
                && !bytes.contains(&0)
                && std::str::from_utf8(&bytes).is_ok()
            {
                response.text = String::from_utf8(bytes)?;
            } else {
                response.binary = Some(STANDARD.encode(bytes));
            }
        }
        Request::Read { .. } => {
            let bytes = read_bounded(&path)?;
            if bytes.contains(&0) {
                bail!("Binary files cannot be edited.");
            }
            response.revision = digest(&bytes);
            response.text =
                String::from_utf8(bytes).context("Only UTF-8 text files are supported")?;
        }
        Request::ReadImage { .. } | Request::ReadBinary { .. } => {
            let kind = if matches!(request, Request::ReadBinary { .. }) {
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
            let file = crate::persistence::open_regular(&path, false)?;
            let metadata = file.metadata()?;
            anyhow::ensure!(
                metadata.is_file(),
                "Only regular image files can be opened."
            );
            anyhow::ensure!(
                metadata.len() <= crate::images::MAX_ENCODED as u64,
                "{kind} file exceeds 32 MiB."
            );
            let mut bytes = Vec::new();
            file.take(crate::images::MAX_ENCODED as u64 + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() <= crate::images::MAX_ENCODED,
                "{kind} file exceeds 32 MiB."
            );
            response.revision = digest(&bytes);
            if matches!(request, Request::ReadBinary { .. }) {
                response.binary = Some(STANDARD.encode(bytes));
            } else {
                response.image = Some(STANDARD.encode(bytes));
            }
        }
        Request::Write { expected, .. } | Request::WriteBinary { expected, .. } => {
            use base64::{Engine, engine::general_purpose::STANDARD};
            let content = match request {
                Request::Write { text, .. } => text.as_bytes().to_vec(),
                Request::WriteBinary { data, .. } => {
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
            let existing = crate::persistence::lock_existing(&path)?;
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
    let file = crate::persistence::open_regular(path, false)?;
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
        crate::persistence::lock(&file)?;
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

/// Runs over an SSH channel; no package installation or persistent remote process.
pub const REMOTE_HELPER: &str = concat!(
    "RODE_SANDBOX_POLICY = ",
    include_str!("sandbox_policy.json"),
    "\n",
    include_str!("remote_sandbox.py"),
    "\n",
    include_str!("remote_project.py"),
    "\n",
    include_str!("remote_sessions.py"),
    "\n",
    include_str!("remote_extensions.py"),
    "\n",
    include_str!("remote_marp.py"),
    "\n",
    include_str!("remote_latex.py"),
    "\nRODE_LANGUAGES = json.loads(r\"\"\"",
    include_str!("languages.json"),
    "\"\"\")\n",
    "\n",
    include_str!("remote_files.py")
);

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

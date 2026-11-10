//! Bounded configuration transactions shared by headless services.
pub use crate::locking::{lock, lock_existing, open_regular};
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
/// Maximum bounded configuration size in bytes.
pub const LIMIT: usize = 8 * 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);

/// Read a bounded regular file; a missing path returns `None`, while oversized or nonregular
/// files fail. This reads bytes without parsing their configuration format.
pub fn read(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    let file = match open_regular(path, false) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("Cannot read {}", path.display())),
    };
    ensure!(
        file.metadata()?.is_file(),
        "{} is not a regular file",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "{} exceeds its size limit",
        path.display()
    );
    Ok(Some(bytes))
}

fn private_file(path: &Path, exclusive: bool) -> Result<File> {
    use crate::locking::PrivateFile;
    Ok(crate::locking::open_private(
        path,
        if exclusive {
            PrivateFile::New
        } else {
            PrivateFile::Lock
        },
    )?)
}

/// Serialize a read/modify/replace transaction under the stable configuration lock and
/// current destination inode lock. The callback receives whether the file existed after
/// locking; it must reload the latest data, parse/merge it, and call [`replace`] if needed.
///
/// This function does not write or roll back callback side effects. Parse before publishing
/// so malformed files remain intact. Lock waits may block; call from a worker. Handles are
/// released on return, while the stable adjacent lock file remains for other processes.
pub fn transaction<T>(path: &Path, update: impl FnOnce(bool) -> Result<T>) -> Result<T> {
    let parent = path.parent().context("Configuration path has no parent")?;
    fs::create_dir_all(parent)?;
    let file = private_file(
        &path.with_file_name(format!(
            "{}.lock",
            path.file_name()
                .context("Missing filename")?
                .to_string_lossy()
        )),
        false,
    )?;
    lock(&file)?;
    let existing = lock_existing(path)?;
    update(existing.is_some())
}

/// Atomically publish configuration bytes; `overwrite = false` refuses an existing destination.
/// Call inside [`transaction`] when merging shared state. Parent-directory sync makes
/// publication durable where supported.
pub fn replace(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    ensure!(
        bytes.len() <= LIMIT,
        "Configuration exceeds 8 MiB; changes were not saved."
    );
    let temporary = path.with_file_name(format!(
        ".rode-config-{}-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    // Only remove a temporary file after successfully creating it ourselves.
    let mut file = private_file(&temporary, true)?;
    let result = (|| {
        lock(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if overwrite {
            fs::rename(&temporary, path)?;
        } else {
            // A prior existence check would race another creator. Linking the
            // completed inode publishes only if the destination is still absent.
            fs::hard_link(&temporary, path).context(
                "Configuration appeared in another process; changes were not saved. Try again.",
            )?;
            fs::remove_file(&temporary)?;
        }
        crate::locking::open_directory(path.parent().context("Missing configuration directory")?)?
            .sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

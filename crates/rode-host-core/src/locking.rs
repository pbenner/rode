//! Advisory locks shared by source and configuration writers.
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    time::{Duration, Instant},
};

/// Open before checking file type without waiting for a FIFO peer. Regular-file
/// reads/writes retain their usual behavior; symlinks remain supported.
pub fn open_regular(path: &Path, writable: bool) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .write(writable)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Only regular files can be accessed.",
        ));
    }
    Ok(file)
}

/// The file remains locked until its handle is dropped. Never unlink lock files.
pub fn lock(file: &File) -> Result<()> {
    lock_with_timeout(file, Duration::from_secs(3), || false)
}

/// Acquire a shared advisory lock with a caller-owned deadline and cancellation check.
/// Deployment can wait longer than interactive source/configuration saves.
pub fn lock_with_timeout(
    file: &File,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
) -> Result<()> {
    use std::os::fd::AsRawFd;
    let until = Instant::now() + timeout;
    loop {
        ensure!(!cancelled(), "File lock cancelled");
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock
            && error.kind() != std::io::ErrorKind::Interrupted
        {
            return Err(error).context("Cannot lock the file safely");
        }
        ensure!(
            Instant::now() < until,
            "File is busy in another process; changes were not saved. Try again."
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Follow inode replacements after waiting, so all RODE file writers cooperate.
pub fn lock_existing(path: &Path) -> Result<Option<File>> {
    use std::os::unix::fs::MetadataExt;
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let mut file = match open_regular(path, false) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            file.metadata()?.is_file(),
            "Only regular files can be written."
        );
        if let Err(error) = lock(&file) {
            // Linux local filesystems permit read-only flock handles. Some
            // network filesystems require a writable descriptor for EX locks.
            if error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error)
                == Some(libc::EBADF)
            {
                file = open_regular(path, true)?;
                lock(&file)?;
            } else {
                return Err(error);
            }
        }
        let opened = file.metadata()?;
        // Waiting on an inode that has since been replaced does not protect the
        // current path. Reopen before doing a revision check or merge on it.
        if fs::metadata(path)
            .is_ok_and(|current| current.dev() == opened.dev() && current.ino() == opened.ino())
        {
            return Ok(Some(file));
        }
        ensure!(
            Instant::now() < until,
            "File keeps changing in another process; changes were not saved."
        );
    }
}

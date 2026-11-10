//! Descriptor type guards and advisory locks shared by host filesystem services.
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    time::{Duration, Instant},
};

/// Open a directory handle for locking, permissions or durable publication.
/// Enforce the type during open rather than after a pathname check: replacing
/// a previously checked directory with a FIFO must not wait for a writer.
/// Ordinary directory symlinks remain supported. This does not check ownership,
/// protect ancestor names or bound I/O on unavailable/network filesystems.
pub fn open_directory(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
        .open(path)
}

/// Open before checking file type without waiting for a FIFO peer. Regular-file
/// reads/writes retain their usual behavior; symlinks remain supported.
pub fn open_regular(path: &Path, writable: bool) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .write(writable)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Only regular files can be accessed.",
        ));
    }
    Ok(file)
}

/// Open an SSH private key and validate the opened inode before reading secrets.
/// Ordinary file symlinks are allowed, but the resolved file must belong to this
/// user and deny all group/other access. A pathname preflight would leave a race
/// between checking permissions and opening a substituted key.
pub fn open_private_key(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::MetadataExt;
    let file = open_regular(path, false)?;
    let metadata = file.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "SSH private key {} must be owned by the current user and inaccessible to group/others; check ownership and use chmod 600",
                path.display()
            ),
        ));
    }
    Ok(file)
}

/// Creation/write mode for an owner-only regular runtime or configuration file.
#[derive(Clone, Copy)]
pub enum PrivateFile {
    /// Reuse or create a stable read/write lock without truncating its contents.
    Lock,
    /// Reuse or create a log, appending every write to its end.
    Append,
    /// Create a new temporary file, refusing any existing directory entry.
    New,
}

/// Open without following a leaf symlink, acquiring a controlling terminal or
/// waiting on a FIFO. Before any caller writes, validate the opened inode as an
/// owner-only regular file with one link; redirected logs/locks cannot modify
/// another file through a pre-existing hard link. New files request mode 0600.
pub fn open_private(path: &Path, mode: PrivateFile) -> std::io::Result<File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC);
    match mode {
        PrivateFile::New => {
            options.create_new(true);
        }
        PrivateFile::Lock => {
            options.create(true);
        }
        PrivateFile::Append => {
            options.create(true).append(true);
        }
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "Private file {} must be a singly linked regular file owned by the current user with no group/other access",
                path.display()
            ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn checked_directory_replaced_by_a_fifo_is_rejected_without_waiting() {
        use std::os::unix::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("output");
        fs::create_dir(&path).unwrap();
        // Reproduce replacement after a caller's successful create/canonicalize
        // checks but before it acquires the directory lock or sync handle.
        let checked = path.canonicalize().unwrap();
        fs::rename(&path, root.path().join("original-output")).unwrap();
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(open_directory(&checked));
        });
        let result = rx.recv_timeout(Duration::from_secs(1));
        // Release a legacy blocking open before failing so the regression never
        // leaves a stuck worker behind, even against the vulnerable primitive.
        let _release = result.is_err().then(|| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
                .unwrap()
        });
        worker.join().unwrap();
        assert!(
            result
                .expect("Directory open waited for a substituted FIFO peer")
                .is_err()
        );
    }

    #[test]
    fn directory_handles_allow_directory_aliases_and_reject_other_entry_types() {
        let root = tempfile::tempdir().unwrap();
        let alias = root.path().join("alias");
        let regular = root.path().join("file");
        fs::write(&regular, "keep").unwrap();
        symlink(root.path(), &alias).unwrap();
        for path in [root.path(), &alias] {
            assert!(open_directory(path).unwrap().metadata().unwrap().is_dir());
        }
        for path in [&regular, &root.path().join("missing")] {
            assert!(open_directory(path).is_err());
        }
        fs::remove_file(&alias).unwrap();
        symlink(&regular, &alias).unwrap();
        assert!(open_directory(&alias).is_err());
        assert_eq!(fs::read(&regular).unwrap(), b"keep");
    }

    #[test]
    fn private_files_preserve_append_and_lock_contents_and_reject_redirected_entries() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private");
        open_private(&path, PrivateFile::New)
            .unwrap()
            .write_all(b"first")
            .unwrap();
        assert!(open_private(&path, PrivateFile::New).is_err());
        open_private(&path, PrivateFile::Append)
            .unwrap()
            .write_all(b"second")
            .unwrap();
        let mut contents = String::new();
        open_private(&path, PrivateFile::Lock)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "firstsecond");
        let alias = directory.path().join("alias");
        symlink(&path, &alias).unwrap();
        for mode in [PrivateFile::Append, PrivateFile::Lock] {
            assert!(open_private(&alias, mode).is_err());
        }
        fs::remove_file(&alias).unwrap();
        fs::hard_link(&path, &alias).unwrap();
        for mode in [PrivateFile::Append, PrivateFile::Lock] {
            assert!(open_private(&alias, mode).is_err());
        }
        fs::remove_file(&alias).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(open_private(&path, PrivateFile::Lock).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"firstsecond");
    }

    #[test]
    fn private_runtime_files_reject_fifos_without_waiting_for_a_peer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fifo");
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        for mode in [PrivateFile::Append, PrivateFile::Lock] {
            let path = path.clone();
            let reader_path = path.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let _ = tx.send(open_private(&reader_path, mode));
            });
            let result = rx.recv_timeout(Duration::from_secs(1));
            let _release = result.is_err().then(|| {
                use std::os::unix::fs::OpenOptionsExt;
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&path)
                    .unwrap()
            });
            worker.join().unwrap();
            assert!(
                result
                    .expect("A private file open waited for a FIFO peer")
                    .is_err()
            );
        }
    }

    #[test]
    fn private_keys_validate_the_resolved_file_permissions_without_exposing_contents() {
        let directory = tempfile::tempdir().unwrap();
        let key = directory.path().join("key");
        let alias = directory.path().join("alias");
        let secret = "synthetic-private-key-content";
        fs::write(&key, secret).unwrap();
        symlink(&key, &alias).unwrap();
        for mode in [0o400, 0o600, 0o640, 0o644, 0o660, 0o666] {
            fs::set_permissions(&key, fs::Permissions::from_mode(mode)).unwrap();
            for path in [&key, &alias] {
                if mode & 0o077 == 0 {
                    assert!(open_private_key(path).is_ok(), "{mode:o}");
                } else {
                    let error = open_private_key(path).unwrap_err();
                    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
                    assert!(error.to_string().contains("chmod 600"));
                    assert!(!error.to_string().contains(secret));
                }
            }
        }
        assert!(open_private_key(directory.path()).is_err());
        assert_eq!(
            open_private_key(&directory.path().join("missing"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }
}

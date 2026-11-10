//! Host-owned runtime directory and filesystem discovery socket setup.
//!
//! Validation applies to the opened inode. Keep that descriptor alive while
//! using [`runtime_path`] so Linux log, lock, capability and socket operations
//! cannot be redirected by replacing the caller-supplied pathname. Binding the
//! discovery socket changes the process umask briefly and is valid only during
//! dedicated helper startup, before worker threads or user processes exist.
use crate::PROTOCOL;
use anyhow::{Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
            net::UnixListener,
        },
    },
    path::{Path, PathBuf},
};

/// Resolve the host runtime directory, honoring `RODE_SESSION_DIR`; ownership validation occurs
/// on use.
pub fn directory() -> PathBuf {
    std::env::var_os("RODE_SESSION_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!("/tmp/rode-session-v{PROTOCOL}-{}", unsafe {
                libc::geteuid()
            }))
        })
}
/// Create or validate the host directory and keep its inode open for startup.
pub(super) fn private_directory() -> Result<(PathBuf, File)> {
    let path = directory();
    let directory = private_directory_at(&path)?;
    Ok((path, directory))
}
/// Open an absolute, owner-only directory without following a final symlink.
pub(super) fn private_directory_at(path: &Path) -> Result<File> {
    ensure!(
        path.is_absolute(),
        "Terminal runtime directory must be absolute"
    );
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)?;
    let meta = directory.metadata()?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
        "Session directory must be an owner-only directory: {}",
        path.display()
    );
    Ok(directory)
}
/// Address runtime entries through the validated directory inode on Linux.
/// Renaming a writable runtime path must not redirect host writes or cleanup.
pub(super) fn runtime_path(path: &Path, directory: &File) -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        let _ = path;
        PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = directory;
        path.to_owned()
    }
}

/// Bind in the dedicated helper's startup phase, before creating workers.
/// Creation permissions avoid a pathname chmod that could follow a substituted
/// symlink/hard link. Restore the original mask before launching user processes.
pub(super) fn private_listener(path: &Path) -> std::io::Result<UnixListener> {
    struct Mask(libc::mode_t);
    impl Drop for Mask {
        fn drop(&mut self) {
            unsafe {
                libc::umask(self.0);
            }
        }
    }
    let _mask = Mask(unsafe { libc::umask(0o177) });
    UnixListener::bind(path)
}

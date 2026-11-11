//! Host-owned runtime directory and filesystem discovery socket setup.
//!
//! Validation applies to the opened inode. Keep that descriptor alive while
//! using [`runtime_path`] so Linux log, lock, capability and socket operations
//! cannot be redirected by replacing the caller-supplied pathname. Binding the
//! discovery socket changes the process umask briefly and is valid only during
//! dedicated helper startup, before worker threads or user processes exist.
use anyhow::{Context, Result, ensure};
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
pub fn directory() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("RODE_SESSION_DIR") {
        return Ok(path.into());
    }
    let home = rode_host_core::private_tmp::home()?;
    let hostname = crate::tunnels::local_hostname()
        .context("Cannot determine the host for the session runtime directory")?;
    Ok(host_directory(&home, &hostname))
}

fn host_directory(home: &Path, hostname: &str) -> PathBuf {
    // Home directories may be shared across cluster nodes; singleton locks and
    // capabilities belong to one host. Hashing also bounds and escapes the name.
    let host = rode_host_core::files::digest(hostname.as_bytes());
    home.join(format!(".cache/rode/session-{host}"))
}
/// Create or validate the host directory and keep its inode open for startup.
pub(super) fn private_directory() -> Result<(PathBuf, File)> {
    let path = directory()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_homes_keep_stable_separate_host_runtime_directories() {
        let home = Path::new("/shared/home/user");
        let first = host_directory(home, "node-a");
        assert_eq!(first, host_directory(home, "node-a"));
        assert_ne!(first, host_directory(home, "node-b"));
        assert_eq!(first.parent(), Some(home.join(".cache/rode").as_path()));
        assert_ne!(
            first,
            host_directory(Path::new("/other/home/user"), "node-a")
        );
        let unusual = host_directory(home, "/invalid/../hostname");
        assert_eq!(first.parent(), unusual.parent());
        assert_eq!(
            first.file_name().unwrap().len(),
            unusual.file_name().unwrap().len()
        );
    }
}

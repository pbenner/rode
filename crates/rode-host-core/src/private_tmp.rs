//! Owner-only temporary storage shared by desktop and native host entry points.
//! Environment and umask initialization happens only before worker threads start.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::CString,
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::{Path, PathBuf},
};

/// Temporary-directory environment variables managed by RODE.
pub const VARIABLES: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];

fn opened(raw: libc::c_int) -> std::io::Result<File> {
    if raw < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(raw) })
    }
}

fn flags() -> libc::c_int {
    #[cfg(target_os = "linux")]
    let access = libc::O_PATH;
    #[cfg(not(target_os = "linux"))]
    let access = libc::O_RDONLY;
    access | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

fn directory(home: &Path, project: Option<&str>) -> Result<(PathBuf, File)> {
    let home = home
        .canonicalize()
        .context("Cannot resolve home for private temporary storage")?;
    ensure!(
        home.is_absolute(),
        "Private temporary storage requires an absolute home"
    );
    let mut handle = opened(unsafe { libc::open(c"/".as_ptr(), flags()) })?;
    for component in home.components().skip(1) {
        let name = CString::new(component.as_os_str().as_encoded_bytes())?;
        handle = opened(unsafe { libc::openat(handle.as_raw_fd(), name.as_ptr(), flags()) })?;
    }
    let metadata = handle.metadata()?;
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
        "Home must be owned by the current user and not writable by group/others: {}",
        home.display()
    );
    let mut path = home;
    let mut components = vec![".cache", "rode", "tmp"];
    if let Some(project) = project {
        components.push(project);
    }
    for (index, component) in components.iter().enumerate() {
        path.push(component);
        let name = CString::new(*component)?;
        let open = || opened(unsafe { libc::openat(handle.as_raw_fd(), name.as_ptr(), flags()) });
        let next = match open() {
            Ok(next) => next,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let result = unsafe { libc::mkdirat(handle.as_raw_fd(), name.as_ptr(), 0o700) };
                if result != 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(error.into());
                    }
                }
                open()?
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Unsafe temporary directory {}", path.display()));
            }
        };
        let metadata = next.metadata()?;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
            "Unsafe temporary directory ownership or permissions: {}",
            path.display()
        );
        ensure!(
            index + 1 != components.len() || metadata.mode() & 0o077 == 0,
            "Temporary directory must be owner-only (0700): {}",
            path.display()
        );
        handle = next;
    }
    Ok((path, handle))
}

/// Create/validate a host's private temporary root; ignore inherited public TMPDIR values.
pub fn root(home: &Path) -> Result<PathBuf> {
    Ok(directory(home, None)?.0)
}

/// Pin a project-specific owner-only directory for either sandbox implementation.
pub fn project(home: &Path, root: &Path) -> Result<(PathBuf, File)> {
    directory(
        home,
        Some(&crate::files::digest(root.as_os_str().as_encoded_bytes())),
    )
}

/// Set private defaults for this process and descendants, before any threads start.
/// An already sandboxed helper retains its captured project-specific directory.
///
/// # Safety
/// Call only at single-threaded process startup: environment and umask are process-wide.
pub unsafe fn initialize() -> Result<()> {
    unsafe {
        libc::umask(0o077);
    }
    let path = from_environment()?;
    if std::env::var_os("HOME").is_none() {
        unsafe {
            std::env::set_var("HOME", home()?);
        }
    }
    for name in VARIABLES {
        unsafe {
            std::env::set_var(name, &path);
        }
    }
    Ok(())
}

fn from_environment() -> Result<PathBuf> {
    let home = home()?;
    let base = home.join(".cache/rode/tmp");
    let inherited = std::env::var_os("TMPDIR").map(PathBuf::from);
    let project = inherited
        .as_ref()
        .filter(|path| path.parent() == Some(base.as_path()))
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()));
    Ok(directory(&home, project)?.0)
}

/// Resolve this host's home, including its account database when HOME is unset.
pub fn home() -> Result<PathBuf> {
    let home =
        std::env::home_dir().context("Cannot determine home for private temporary storage")?;
    ensure!(
        home.is_absolute(),
        "Private temporary storage requires an absolute home"
    );
    home.canonicalize()
        .context("Cannot resolve home for private temporary storage")
}

/// Restore host-local defaults after receiving a serialized child environment.
pub fn apply(environment: &mut std::collections::BTreeMap<String, String>) -> Result<()> {
    environment
        .entry("HOME".into())
        .or_insert(home()?.to_string_lossy().into_owned());
    let path = from_environment()?.to_string_lossy().into_owned();
    for name in VARIABLES {
        environment.insert(name.into(), path.clone());
    }
    Ok(())
}

/// Protect transient command children even when their environment was explicitly cleared.
pub fn command(command: &mut std::process::Command) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let path = from_environment()?;
    if !command
        .get_envs()
        .any(|(name, value)| name == "HOME" && value.is_some())
    {
        command.env("HOME", home()?);
    }
    for name in VARIABLES {
        command.env(name, &path);
    }
    // Only the forked child changes its mask; no process-wide race in workers.
    unsafe {
        command.pre_exec(|| {
            libc::umask(0o077);
            Ok(())
        });
    }
    Ok(())
}

/// Restore private defaults after a shell's startup files or environment provider.
pub fn shell_exports(path: &Path) -> String {
    let value = crate::command::quote(&path.to_string_lossy());
    format!("umask 077; export TMPDIR={value} TMP={value} TEMP={value};\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn directories_are_private_stable_and_separate_for_each_project() {
        let home = tempfile::tempdir().unwrap();
        let base = root(home.path()).unwrap();
        assert!(base.starts_with(home.path()));
        assert_eq!(base.metadata().unwrap().mode() & 0o777, 0o700);
        let (first, handle) = project(home.path(), Path::new("/project/first")).unwrap();
        let (second, _) = project(home.path(), Path::new("/project/second")).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(base.as_path()));
        assert_eq!(handle.metadata().unwrap().mode() & 0o777, 0o700);
        assert_eq!(
            project(home.path(), Path::new("/project/first")).unwrap().0,
            first
        );
    }

    #[test]
    fn unsafe_existing_directories_and_symlinks_are_rejected_without_repair() {
        let home = tempfile::tempdir().unwrap();
        let base = root(home.path()).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(root(home.path()).is_err());
        assert_eq!(base.metadata().unwrap().mode() & 0o777, 0o750);
        std::fs::remove_dir(&base).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        symlink(elsewhere.path(), &base).unwrap();
        assert!(root(home.path()).is_err());
        assert!(
            std::fs::read_dir(elsewhere.path())
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn concurrent_creation_reuses_only_verified_directories() {
        let home = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            let workers = (0..8)
                .map(|_| scope.spawn(|| project(home.path(), Path::new("/project")).unwrap().0))
                .collect::<Vec<_>>();
            let paths = workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>();
            assert!(paths.iter().all(|path| path == &paths[0]));
        });
    }

    #[test]
    fn cleared_command_environments_still_get_private_temporary_storage_and_mask() {
        let mut child = std::process::Command::new("/bin/sh");
        child.env_clear().env("TMPDIR", "/tmp").args([
            "-c",
            "printf '%s\\n' \"$TMPDIR\" \"$TMP\" \"$TEMP\"; umask; printf '%s\\n' \"$HOME\"",
        ]);
        command(&mut child).unwrap();
        let result = child.output().unwrap();
        assert!(result.status.success());
        let output = String::from_utf8(result.stdout).unwrap();
        let lines = output.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], lines[1]);
        assert_eq!(lines[0], lines[2]);
        assert!(Path::new(lines[0]).starts_with(std::env::var_os("HOME").unwrap()));
        assert_eq!(u32::from_str_radix(lines[3], 8).unwrap(), 0o077);
        assert_eq!(Path::new(lines[4]), home().unwrap());
    }

    #[test]
    fn serialized_environments_keep_host_local_private_defaults_without_home() {
        let mut environment = std::collections::BTreeMap::from([
            ("PATH".into(), "/bin".into()),
            ("TMPDIR".into(), "/tmp/client".into()),
            ("TEMP".into(), "/other/client".into()),
        ]);
        apply(&mut environment).unwrap();
        assert_eq!(Path::new(&environment["HOME"]), home().unwrap());
        for name in VARIABLES {
            assert_eq!(Path::new(&environment[name]), from_environment().unwrap());
        }
        assert_eq!(environment["PATH"], "/bin");
    }
}

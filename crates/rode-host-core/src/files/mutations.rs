//! Native explorer mutations. Rename/publication never overwrite an existing
//! destination. Copies stage beside their destination and stream regular-file
//! contents. Traversal is bounded; symlinks are retained and special files are
//! rejected. The caller owns the sandbox boundary.
use super::{Response, expand_home};
use anyhow::{Context, Result, bail, ensure};
use rode_host_protocol::files::Mutation;
use std::{
    fs,
    path::{Path, PathBuf},
};

const COPY_ENTRIES: usize = 10_000;
const COPY_DEPTH: usize = 64;

/// Resolve the parent only: canonicalizing the leaf would rename/delete a
/// symlink's target instead of the entry the user actually selected.
fn entry_path(value: &str) -> Result<PathBuf> {
    ensure!(!value.contains('\0'), "Invalid file path");
    let path = expand_home(value);
    let name = path
        .file_name()
        .context("Choose a file or folder below the filesystem root")?;
    let parent = path
        .parent()
        .context("File has no parent directory")?
        .canonicalize()
        .with_context(|| format!("Cannot open the parent of {}", path.display()))?;
    Ok(parent.join(name))
}

fn rename_new(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let source = CString::new(source.as_os_str().as_bytes())?;
        let destination = CString::new(destination.as_os_str().as_bytes())?;
        // SAFETY: Both C strings stay alive for the syscall; NOREPLACE makes
        // collision checks atomic, including dangling destination symlinks.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .context("Cannot move entry without replacing an existing destination");
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, destination);
        bail!("Atomic explorer moves require Linux")
    }
}

#[derive(Default)]
struct Budget {
    entries: usize,
}

fn copy_entry(source: &Path, destination: &Path, budget: &mut Budget, depth: usize) -> Result<()> {
    budget.entries += 1;
    ensure!(
        budget.entries <= COPY_ENTRIES && depth <= COPY_DEPTH,
        "Copy exceeds the 10,000-entry or 64-level folder limit"
    );
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
        #[cfg(not(unix))]
        bail!("Copying symbolic links requires Unix");
    } else if metadata.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(
                &entry.path(),
                &destination.join(entry.file_name()),
                budget,
                depth + 1,
            )?;
        }
        fs::set_permissions(destination, metadata.permissions())?;
    } else if metadata.is_file() {
        let mut source = crate::locking::open_regular(source, false)?;
        let metadata = source.metadata()?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        std::io::copy(&mut source, &mut output)?;
        output.set_permissions(metadata.permissions())?;
    } else {
        bail!("Only regular files, folders and symbolic links can be copied");
    }
    Ok(())
}

/// Copies can contain read-only directories. Restore owner access only inside
/// our private staging directory so a failed operation can remove its own files.
fn writable_staging(path: &Path) {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.is_dir()
    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(
                path,
                fs::Permissions::from_mode(metadata.permissions().mode() | 0o700),
            );
        }
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                writable_staging(&entry.path());
            }
        }
    }
}

fn duplicate(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        ensure!(
            !destination.starts_with(source),
            "A folder cannot be copied inside itself"
        );
    }
    ensure!(
        fs::symlink_metadata(destination)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "The destination already exists or cannot be inspected"
    );
    let staging = tempfile::Builder::new()
        .prefix(".rode-copy-")
        .tempdir_in(destination.parent().context("Missing destination parent")?)?;
    let result = copy_entry(
        source,
        &staging.path().join("entry"),
        &mut Budget::default(),
        0,
    )
    .and_then(|()| rename_new(&staging.path().join("entry"), destination));
    if result.is_err() {
        writable_staging(staging.path());
    }
    result
}

pub(super) fn dispatch(mutation: &Mutation) -> Result<Response> {
    let path = match mutation {
        Mutation::CreateDirectory { path }
        | Mutation::Rename { path, .. }
        | Mutation::Copy { path, .. }
        | Mutation::Remove { path } => entry_path(path)?,
    };
    let result = match mutation {
        Mutation::CreateDirectory { .. } => {
            fs::create_dir(&path)?;
            path
        }
        Mutation::Rename { destination, .. } => {
            let destination = entry_path(destination)?;
            rename_new(&path, &destination)?;
            destination
        }
        Mutation::Copy { destination, .. } => {
            let destination = entry_path(destination)?;
            duplicate(&path, &destination)?;
            destination
        }
        Mutation::Remove { .. } => {
            if fs::symlink_metadata(&path)?.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
            path
        }
    };
    Ok(Response {
        path: result.to_string_lossy().into(),
        ..Default::default()
    })
}

//! Trust checks for executable-cache directories and their renameable ancestors.
//!
//! Protecting only a cache's final directory does not protect its pathname: a
//! different user who can rename an ancestor can substitute code after hashing.
//! Check both the supplied spelling and resolved path so symlinked homes remain
//! usable without trusting an alias beneath a writable parent.
use anyhow::{Context, Result, ensure};
use std::{fs, os::unix::fs::MetadataExt, path::Path};

/// Reject directory paths another user can substitute through an ancestor.
/// Missing tails are allowed for publication; the closest existing directory
/// and its ancestors must belong to root/current UID and deny group/other writes.
/// Sticky directories such as `/tmp` are allowed when owned by root/current UID.
/// Symlinks must have a trusted owner, and their resolved ancestry is checked too.
///
/// This does not create directories, repair permissions, validate leaf files or
/// isolate unrestricted processes running as the same UID.
pub fn trusted_ancestors(path: &Path) -> Result<()> {
    let absolute = std::path::absolute(path)?;
    let mut existing = absolute.as_path();
    loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                existing = existing
                    .parent()
                    .context("Missing executable-cache ancestor")?;
            }
            Err(error) => return Err(error).context("Cannot inspect executable-cache ancestors"),
        }
    }
    let uid = unsafe { libc::geteuid() };
    let mut checked = std::collections::HashSet::new();
    let mut pending = vec![existing.to_owned()];
    while let Some(spelling) = pending.pop() {
        // Inspect link targets as paths too: a target may traverse an unsafe
        // tree before another symlink leads back into a protected directory.
        let resolved = spelling
            .canonicalize()
            .context("Cannot resolve executable-cache directory")?;
        for ancestor in spelling.ancestors().chain(resolved.ancestors()) {
            if !checked.insert(ancestor.to_owned()) {
                continue;
            }
            let metadata = fs::symlink_metadata(ancestor)?;
            ensure!(
                (metadata.is_dir() || metadata.file_type().is_symlink())
                    && (metadata.uid() == 0 || metadata.uid() == uid),
                "Unsafe executable-cache directory owner/type: {}",
                ancestor.display()
            );
            ensure!(
                metadata.file_type().is_symlink()
                    || metadata.mode() & 0o022 == 0
                    || (metadata.is_dir() && metadata.mode() & 0o1000 != 0),
                "Executable-cache ancestor is writable by another user: {}",
                ancestor.display()
            );
            if metadata.file_type().is_symlink() {
                let target = fs::read_link(ancestor)?;
                pending.push(if target.is_absolute() {
                    target
                } else {
                    ancestor
                        .parent()
                        .context("Missing executable-cache symlink parent")?
                        .join(target)
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn protected_leaves_cannot_hide_writable_ancestors_or_aliases() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        fs::create_dir(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
        let private = shared.join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(trusted_ancestors(&private).is_err());
        assert!(trusted_ancestors(&private.join("not-created/yet")).is_err());

        let alias = root.path().join("alias");
        symlink(&private, &alias).unwrap();
        assert!(trusted_ancestors(&alias).is_err());
        fs::remove_file(&alias).unwrap();
        symlink(root.path(), shared.join("alias")).unwrap();
        assert!(trusted_ancestors(&shared.join("alias")).is_err());
        symlink(&private, &alias).unwrap();
        symlink(root.path(), private.join("back")).unwrap();
        assert!(trusted_ancestors(&alias.join("back")).is_err());
        fs::remove_file(&alias).unwrap();
        symlink(private.join("back"), &alias).unwrap();
        assert!(trusted_ancestors(&alias).is_err());

        fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
        trusted_ancestors(&private).unwrap();
        trusted_ancestors(&private.join("not-created/yet")).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o700)).unwrap();
        trusted_ancestors(&shared.join("alias")).unwrap();
    }
}

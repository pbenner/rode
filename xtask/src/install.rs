//! Install both target builds with a static sibling that SSH discovers automatically.

use crate::{Workspace, helper, run};
use anyhow::{Context, Result, ensure};
use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

/// Resolve the documented prefix without modifying process-global environment.
pub(super) fn default_root(
    install_root: Option<OsString>,
    cargo_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf> {
    if let Some(root) = install_root
        .filter(|root| !root.is_empty())
        .or_else(|| cargo_home.filter(|root| !root.is_empty()))
    {
        return Ok(root.into());
    }
    Ok(PathBuf::from(
        home.filter(|home| !home.is_empty())
            .context("Cannot locate the install directory; pass --root PREFIX")?,
    )
    .join(".cargo"))
}

/// Stage before changing the editor, and rename only after Cargo succeeds.
/// Existing jobs retain their old inode and a failed install retains the old helper.
pub(super) fn publish(
    source: &Path,
    destination: &Path,
    install_editor: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let directory = destination.parent().context("Missing helper directory")?;
    fs::create_dir_all(directory)?;
    let mut staged = tempfile::Builder::new()
        .prefix(".rode-sessiond-static-")
        .tempfile_in(directory)?;
    io::copy(&mut fs::File::open(source)?, staged.as_file_mut())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    install_editor()?;
    staged
        .persist(destination)
        .context("Cannot publish the static helper")?;
    Ok(())
}

pub(super) fn install(workspace: &Workspace, destination: PathBuf) -> Result<()> {
    ensure!(
        env::consts::OS == "linux",
        "Source installation supports Linux x86_64 and aarch64"
    );
    let architecture = helper::Architecture::parse(env::consts::ARCH)?;
    let helper = helper::build(workspace, architecture)?;
    let destination = env::current_dir()?.join(destination);
    let binaries = destination.join("bin");
    publish(&helper, &binaries.join("rode-sessiond-static"), || {
        run(workspace
            .cargo()
            .args(["install", "--locked", "--path"])
            .arg(&workspace.root)
            .arg("--root")
            .arg(&destination)
            .args(["--force", "--bins"]))
    })?;
    println!(
        "Installed RODE and its static SSH helper in {}",
        binaries.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn install_prefixes_follow_environment_precedence_and_reject_missing_home() {
        let root = |install: Option<&str>, cargo: Option<&str>, home: Option<&str>| {
            default_root(
                install.map(Into::into),
                cargo.map(Into::into),
                home.map(Into::into),
            )
        };
        assert_eq!(
            root(Some("/install"), Some("/cargo"), Some("/home")).unwrap(),
            Path::new("/install")
        );
        assert_eq!(
            root(Some(""), Some("/cargo"), None).unwrap(),
            Path::new("/cargo")
        );
        assert_eq!(
            root(None, None, Some("/home")).unwrap(),
            Path::new("/home/.cargo")
        );
        assert!(root(None, None, None).is_err());
    }

    #[test]
    fn failed_editor_install_preserves_the_helper_and_removes_staging_files() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let binaries = root.path().join("prefix with spaces/bin");
        let destination = binaries.join("rode-sessiond-static");
        fs::create_dir_all(&binaries).unwrap();
        fs::write(&source, b"new helper").unwrap();
        fs::write(&destination, b"old helper").unwrap();
        assert!(publish(&source, &destination, || anyhow::bail!("cargo failed")).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"old helper");
        assert_eq!(fs::read_dir(&binaries).unwrap().count(), 1);

        let mut running = fs::File::open(&destination).unwrap();
        publish(&source, &destination, || Ok(())).unwrap();
        let mut previous = Vec::new();
        running.read_to_end(&mut previous).unwrap();
        assert_eq!(previous, b"old helper");
        assert_eq!(fs::read(&destination).unwrap(), b"new helper");
        assert_eq!(fs::read_dir(&binaries).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
    }
}

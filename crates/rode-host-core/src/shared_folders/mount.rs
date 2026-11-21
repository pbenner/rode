//! One foreground SSHFS process per mount. No listeners or extra authentication.
//! A private filesystem name identifies the mount before any unmount operation;
//! ordinary Unmount refuses busy mounts, while lost-stream cleanup detaches them.
use super::*;
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// Captured mount identity, separate from portable bookmark settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mount {
    /// Existing empty directory on this machine.
    pub path: String,
    /// Unique filesystem name for this attempt; never reused after reconnect.
    pub tag: String,
    /// Also enforced by the exporting SFTP service.
    pub read_only: bool,
}
impl Mount {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.path.len() <= 4096
                && (self.path.starts_with('/') || self.path.starts_with("~/"))
                && !self.path.chars().any(char::is_control),
            "Invalid shared-folder mount path"
        );
        ensure!(
            self.tag.starts_with("rode-share-")
                && self.tag.len() == 43
                && self.tag[11..].bytes().all(|c| c.is_ascii_hexdigit()),
            "Invalid shared-folder mount identity"
        );
        Ok(())
    }
    fn path(&self) -> PathBuf {
        crate::files::expand_home(&self.path)
    }
}

fn unescape(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}
fn mounted(mount: &Mount) -> Result<bool> {
    #[cfg(target_os = "linux")]
    {
        let path = mount.path();
        let mut found = false;
        for line in fs::read_to_string("/proc/self/mountinfo")?.lines() {
            let Some((before, after)) = line.split_once(" - ") else {
                continue;
            };
            let fields: Vec<_> = before.split_whitespace().collect();
            let tail: Vec<_> = after.split_whitespace().collect();
            if fields.len() > 4 && tail.len() > 1 && Path::new(&unescape(fields[4])) == path {
                ensure!(
                    !found,
                    "Mount folder has stacked filesystems; remove the other mount first"
                );
                ensure!(
                    tail[0] == "fuse.sshfs" && unescape(tail[1]) == mount.tag,
                    "Mount folder is occupied by another filesystem"
                );
                found = true;
            }
        }
        Ok(found)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = mount;
        anyhow::bail!("Shared-folder mounts currently require Linux")
    }
}

/// Unmount only the exact filesystem owned by this attempt. Explicit UI actions
/// pass false and preserve a busy share. EOF cleanup passes true after transport loss.
pub fn unmount(mount: &Mount, lazy: bool) -> Result<()> {
    mount.validate()?;
    if !mounted(mount)? {
        return Ok(());
    }
    let tool = crate::sandbox::find_program("fusermount3")
        .or_else(|| crate::sandbox::find_program("fusermount"))
        .context("Install FUSE's fusermount3 on the mounting host")?;
    let mut command = Command::new(tool);
    command
        .arg(if lazy { "-uz" } else { "-u" })
        .arg("--")
        .arg(mount.path());
    crate::command::run_controlled(
        command,
        vec![],
        Duration::from_secs(10),
        &std::sync::atomic::AtomicBool::new(false),
        false,
    )?
    .checked()
    .context("Cannot unmount shared folder; close programs using it and retry")?;
    Ok(())
}

pub(super) fn run(mut mount: Mount) -> Result<()> {
    crate::supervisor::signals()?;
    mount.validate()?;
    let path = mount.path();
    let canonical = path
        .canonicalize()
        .context("Create an empty mount folder first")?;
    ensure!(
        canonical == path && canonical != Path::new("/"),
        "Use a canonical mount folder without symbolic links"
    );
    let meta = fs::symlink_metadata(&path)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o022 == 0,
        "Mount folder must be owned by you and not writable by other users"
    );
    ensure!(!mounted(&mount)?, "Shared folder is already mounted");
    ensure!(
        fs::read_dir(&path)?.next().is_none(),
        "Mount folder must be empty"
    );
    mount.path = canonical
        .to_str()
        .context("Mount path must be UTF-8")?
        .into();
    let program = crate::sandbox::find_program("sshfs")
        .context("Install sshfs and FUSE on the machine receiving the shared folder")?;
    let mut child = Command::new(program)
        .args([
            "-f",
            "-o",
            "passive,nodev,nosuid,noexec,default_permissions,cache=no",
        ])
        .arg("-o")
        .arg(format!(
            "fsname={},uid={},gid={}{}",
            mount.tag,
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
            if mount.read_only { ",ro" } else { "" }
        ))
        .arg(":/")
        .arg(&mount.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut input = child.stdin.take().context("Missing SSHFS stream")?;
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = std::io::copy(&mut std::io::stdin().lock(), &mut input);
        drop(input);
        let _ = send.send(result);
    });
    let start = Instant::now();
    let mut ready = false;
    let result = (|| -> Result<()> {
        loop {
            if let Some(status) = child.try_wait()? {
                ensure!(
                    status.success(),
                    "SSHFS exited with {status}; check FUSE availability and mount permissions"
                );
                return Ok(());
            }
            if crate::command::CANCELLED.load(std::sync::atomic::Ordering::Acquire)
                || receive.try_recv().is_ok()
            {
                anyhow::bail!("Shared-folder connection closed");
            }
            if !ready {
                if mounted(&mount)? {
                    ready = true;
                    eprintln!("RODE_SHARE_READY");
                } else {
                    ensure!(
                        start.elapsed() < Duration::from_secs(20),
                        "SSHFS mount did not become ready within 20 seconds"
                    );
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    // Losing the SFTP stream invalidates open handles; lazy detachment keeps a
    // dead mount from hanging unrelated file browsing. Explicit unmount never uses it.
    let cleanup = unmount(&mount, true);
    let _ = child.kill();
    let _ = child.wait();
    result.and(cleanup)
}

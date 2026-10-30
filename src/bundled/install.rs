//! Explicit per-user Codex installation. This is application setup, outside the
//! project sandbox; executing Codex still uses the normal project-host launcher.
//!
//! Only the editor's embedded release pin supplies download URLs and checksums.
//! Stream into private temporary files, verify before extracting a single regular
//! file, then verify the complete bundle before publishing its protected selector.
//! A shared flock also coordinates the optional Python preparation script. Jobs
//! coalesce progress in memory, cancel on drop and never wait on the GUI thread.

use super::*;
use flate2::read::GzDecoder;
use std::{
    io::{Seek, SeekFrom, Write},
    os::{fd::AsRawFd, unix::fs::DirBuilderExt},
    sync::Mutex,
    time::Instant,
};

macro_rules! notice {
    ($name:literal) => {
        (
            $name,
            include_bytes!(concat!("../../packaging/codex-licenses/", $name)).as_slice(),
        )
    };
}

const NOTICES: &[(&str, &[u8])] = &[
    notice!("LICENSE"),
    notice!("NOTICE"),
    notice!("docs-license.md"),
    notice!("third_party-voice-NOTICE.md"),
    notice!("third_party-voice-licenses-LGPL-2.1.txt"),
    notice!("third_party-voice-licenses-Opus.txt"),
    notice!("third_party-voice-licenses-PCRE2.md"),
    notice!("third_party-voice-licenses-libffi.txt"),
    notice!("third_party-voice-licenses-proxy-libintl.txt"),
    notice!("third_party-voice-licenses-sljit.txt"),
    notice!("third_party-voice-licenses-zlib.txt"),
    notice!("third_party-wezterm-LICENSE"),
    notice!("ratatui-LICENSE"),
];

#[derive(Clone, Deserialize)]
struct Download {
    url: String,
    archive_sha256: String,
    archive_size: u64,
    member: String,
}

struct Package<'a> {
    release: &'a Release,
    architecture: &'a str,
    download: Download,
    notices: &'a [(&'a str, &'a [u8])],
}

/// Completed setup; no agent process or authentication was started.
#[derive(Clone)]
pub(crate) struct Installed {
    pub directory: PathBuf,
    pub architecture: String,
}

/// Coalesced worker state; progress storage stays constant through large downloads.
#[derive(Clone)]
pub(crate) enum State {
    Working {
        label: String,
        done: u64,
        total: u64,
    },
    Finished(std::result::Result<Installed, String>),
}

/// One explicit setup request. Dropping it cancels only this window's work.
pub(crate) struct Job {
    state: Arc<Mutex<State>>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    /// Start setup on a worker, downloading on the desktop for the selected host's CPU.
    pub fn start(remote: Option<ssh::Remote>, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let job = Self {
            state: Arc::new(Mutex::new(State::Working {
                label: "Preparing Codex installation…".into(),
                done: 0,
                total: 0,
            })),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let state = job.state.clone();
        let cancel = job.cancel.clone();
        std::thread::Builder::new().name("codex-install".into()).spawn(move || {
            let mut last = Instant::now() - Duration::from_secs(1);
            let mut phase = "";
            let mut progress = |label: &'static str, done: u64, total: u64| {
                if label != phase || done == total || last.elapsed() >= Duration::from_millis(100) {
                    *state.lock().unwrap() = State::Working {label: label.into(), done, total};
                    wake();
                    phase = label;
                    last = Instant::now();
                }
            };
            let result = (|| {
                let architecture = if let Some(remote) = remote {
                    progress("Checking SSH host architecture…", 0, 0);
                    let host = remote_call(&remote, &serde_json::json!({"action":"host"}), std::io::empty(), &cancel)?;
                    ensure!(host["platform"] == "linux", "Automatic Codex installation supports Linux hosts; configure a compatible executable for this host");
                    host["architecture"].as_str().context("Missing remote architecture")?.to_owned()
                } else {
                    ensure!(std::env::consts::OS == "linux", "Automatic Codex installation supports Linux");
                    std::env::consts::ARCH.into()
                };
                let package = pinned(&architecture)?;
                let base = directory().context("Set HOME or an absolute XDG_DATA_HOME to install Codex")?;
                let path = install_to(&base, &package, &cancel, &mut progress, || fetch(&package.download))?;
                Ok(Installed {directory: path, architecture})
            })();
            *state.lock().unwrap() = State::Finished(result.map_err(|e: anyhow::Error| {
                if cancel.load(Ordering::Acquire) {"Codex installation cancelled".into()}
                else {format!("{e:#}").chars().take(4096).collect()}
            }));
            wake();
        })?;
        Ok(job)
    }

    /// Read cached progress without filesystem or network access.
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// Ask the worker to stop; in-flight reads have a bounded timeout.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    /// Whether cancellation is waiting for the current bounded operation to finish.
    pub fn cancelling(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Stable local setup directory, deliberately separate from credentials/CODEX_HOME.
pub(crate) fn directory() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME");
    let home = std::env::var_os("HOME");
    user_runtime_root(
        data.as_deref().map(Path::new),
        home.as_deref().map(Path::new),
    )
}

/// Version shown before an explicit installation; never queries a release service.
pub(crate) fn version() -> &'static str {
    &embedded().manifest.version
}

fn pinned(architecture: &str) -> Result<Package<'static>> {
    ensure!(
        embedded().manifest.targets.contains_key(architecture),
        "No pinned Codex binary for {architecture}; choose a compatible custom executable or another agent"
    );
    let raw: serde_json::Value = serde_json::from_str(PIN)?;
    let download: Download = serde_json::from_value(raw["targets"][architecture].clone())?;
    ensure!(
        download
            .url
            .starts_with("https://github.com/openai/codex/releases/download/")
            && (1..=512 * 1024 * 1024).contains(&download.archive_size)
            && download.archive_sha256.len() == 64
            && Path::new(&download.member)
                .file_name()
                .and_then(|p| p.to_str())
                == Some(download.member.as_str()),
        "Invalid embedded Codex download pin"
    );
    // Borrow the matching embedded key so the package owns no caller/UI references.
    Ok(Package {
        release: embedded(),
        architecture: embedded()
            .manifest
            .targets
            .get_key_value(architecture)
            .unwrap()
            .0,
        download,
        notices: NOTICES,
    })
}

fn fetch(spec: &Download) -> Result<Box<dyn Read>> {
    let agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .timeout_global(Some(Duration::from_secs(20 * 60)))
        .timeout_resolve(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(20)))
        .timeout_per_read(Some(Duration::from_secs(5)))
        .build()
        .new_agent();
    let response = agent
        .get(&spec.url)
        .header("Accept-Encoding", "identity")
        .call()
        .context("Cannot download Codex; check the network/proxy and try again")?;
    if let Some(length) = response.body().content_length() {
        ensure!(
            length == spec.archive_size,
            "Codex archive has an unexpected download size"
        );
    }
    Ok(Box::new(response.into_body().into_reader()))
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Codex installation cancelled"
    );
    Ok(())
}

/// Create only private directories; never follow a final symlink or repair unsafe permissions.
fn private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            metadata.is_dir()
                && protected(&metadata)
                && metadata.uid() == unsafe { libc::geteuid() },
            "Unsafe Codex installation directory: {}",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().context("Missing installation parent")?;
            if !parent.exists() {
                private_directory(parent)?;
            }
            match fs::DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => private_directory(path)?,
                Err(e) => return Err(e).context("Cannot create Codex installation directory"),
            }
        }
        Err(e) => return Err(e).context("Cannot inspect Codex installation directory"),
    }
    Ok(())
}

/// RODE's shared preference directory may predate this installer and inherit
/// umask 002. Tighten only that owned directory, through its checked descriptor;
/// existing settings and their locks stay untouched. Runtime roots themselves
/// still reject unsafe permissions instead of trusting previously writable pins.
fn application_directory(path: &Path) -> Result<()> {
    if !path.try_exists()? {
        return private_directory(path);
    }
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = directory.metadata()?;
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "Codex application-data directory must belong to this user"
    );
    if metadata.permissions().mode() & 0o022 != 0 {
        directory.set_permissions(fs::Permissions::from_mode(
            metadata.permissions().mode() & 0o7777 & !0o022,
        ))?;
    }
    Ok(())
}

fn install_lock(base: &Path, cancel: &AtomicBool) -> Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(base.join(".install.lock"))?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && protected(&metadata) && metadata.uid() == unsafe { libc::geteuid() },
        "Unsafe Codex install lock"
    );
    loop {
        cancelled(cancel)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        let error = std::io::Error::last_os_error();
        ensure!(
            matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ),
            "Cannot lock Codex installation: {error}"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn copy_pinned(
    mut input: impl Read,
    mut out: impl Write,
    size: u64,
    sha256: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    let mut digest = Sha256::new();
    let mut total = 0;
    let mut block = [0; 64 * 1024];
    loop {
        cancelled(cancel)?;
        let count = input.read(&mut block)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(total <= size, "Codex file exceeds its pinned size");
        out.write_all(&block[..count])?;
        digest.update(&block[..count]);
        progress(total);
    }
    ensure!(
        total == size && format!("{:x}", digest.finalize()) == sha256,
        "Codex file failed its pinned size/SHA-256 check"
    );
    Ok(())
}

fn install_to(
    base: &Path,
    package: &Package<'_>,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(&'static str, u64, u64),
    fetch: impl FnOnce() -> Result<Box<dyn Read>>,
) -> Result<PathBuf> {
    cancelled(cancel)?;
    application_directory(base.parent().context("Missing user-data directory")?)?;
    private_directory(base)?;
    progress("Waiting for other Codex installers…", 0, 0);
    let _lock = install_lock(base, cancel)?;
    let version = base.join(&package.release.manifest.version);
    private_directory(&version)?;
    let destination = version.join(package.architecture);
    let target = &package.release.manifest.targets[package.architecture];
    if destination.try_exists()? || fs::symlink_metadata(&destination).is_ok() {
        progress("Verifying installed Codex…", 0, 0);
        verify(&destination, package.release, target, cancel)
            .with_context(|| format!("Existing runtime is damaged or differs from this pin: {}. Choose another executable or remove that runtime before reinstalling", destination.display()))?;
    } else {
        let mut archive = tempfile::NamedTempFile::new_in(base)?;
        progress("Downloading Codex…", 0, package.download.archive_size);
        cancelled(cancel)?;
        copy_pinned(
            fetch()?,
            &mut archive,
            package.download.archive_size,
            &package.download.archive_sha256,
            cancel,
            |done| progress("Downloading Codex…", done, package.download.archive_size),
        )?;
        archive.seek(SeekFrom::Start(0))?;
        let stage = tempfile::Builder::new()
            .prefix(".codex-")
            .tempdir_in(&version)?;
        fs::set_permissions(stage.path(), fs::Permissions::from_mode(0o700))?;
        progress("Extracting Codex…", 0, target.size);
        let decoded = GzDecoder::new(archive).take(target.size + 64 * 1024 + 1);
        let mut tar = tar::Archive::new(decoded);
        {
            let mut entries = tar.entries()?.raw(true);
            let mut entry = entries.next().context("Empty Codex archive")??;
            ensure!(
                entry.header().entry_type().is_file()
                    && entry.path()?.as_ref() == Path::new(&package.download.member)
                    && entry.size() == target.size,
                "Unexpected Codex archive member"
            );
            let mut binary = fs::File::create(stage.path().join("codex"))?;
            copy_pinned(
                &mut entry,
                &mut binary,
                target.size,
                &target.sha256,
                cancel,
                |done| progress("Extracting Codex…", done, target.size),
            )?;
            binary.sync_all()?;
            drop(entry);
            ensure!(
                entries.next().is_none(),
                "Unexpected extra Codex archive member"
            );
        }
        let mut decoded = tar.into_inner();
        let mut block = [0; 64 * 1024];
        loop {
            cancelled(cancel)?;
            if decoded.read(&mut block)? == 0 {
                break;
            }
        }
        ensure!(
            decoded.limit() > 0,
            "Codex archive exceeds its decoded size limit"
        );
        fs::set_permissions(
            stage.path().join("codex"),
            fs::Permissions::from_mode(0o755),
        )?;
        fs::write(stage.path().join("manifest.json"), &package.release.raw)?;
        fs::set_permissions(
            stage.path().join("manifest.json"),
            fs::Permissions::from_mode(0o644),
        )?;
        for notice in &package.release.manifest.notices {
            let bytes = package
                .notices
                .iter()
                .find(|(name, _)| *name == notice.file)
                .context("Codex license snapshot is missing from this editor")?
                .1;
            ensure!(
                bytes.len() <= 1024 * 1024 && files::digest(bytes) == notice.sha256,
                "Codex license snapshot failed its SHA-256 check"
            );
            fs::write(stage.path().join(&notice.file), bytes)?;
            fs::set_permissions(
                stage.path().join(&notice.file),
                fs::Permissions::from_mode(0o644),
            )?;
        }
        progress("Verifying Codex…", 0, 0);
        verify(stage.path(), package.release, target, cancel)?;
        cancelled(cancel)?;
        fs::rename(stage.path(), &destination)?;
    }
    cancelled(cancel)?;
    progress("Selecting verified Codex runtime…", 0, 0);
    let mut selector = tempfile::NamedTempFile::new_in(base)?;
    selector.write_all(package.release.raw.as_bytes())?;
    selector
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o644))?;
    selector.as_file().sync_all()?;
    cancelled(cancel)?;
    selector.persist(base.join("manifest.json"))?;
    fs::File::open(base)?.sync_all()?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::atomic::AtomicUsize};

    const BINARY: &[u8] = b"#!/bin/sh\necho installer-fixture\n";
    const LICENSES: &[(&str, &[u8])] = &[("LICENSE", b"fixture license")];

    fn archive(members: &[(&str, tar::EntryType)]) -> Vec<u8> {
        let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gzip);
        for (name, kind) in members {
            let mut header = tar::Header::new_ustar();
            header.set_entry_type(*kind);
            header.set_mode(0o755);
            header.set_size(if kind.is_file() {
                BINARY.len() as u64
            } else {
                0
            });
            if kind.is_symlink() || kind.is_hard_link() {
                header.set_link_name("../outside").unwrap();
            }
            header.set_cksum();
            tar.append_data(&mut header, name, if kind.is_file() { BINARY } else { b"" })
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    fn fixture(archive: &[u8]) -> (Release, Download) {
        (Release::parse(serde_json::json!({
            "version":"9-fixture", "targets":{"fixture":{"size":BINARY.len(), "sha256":files::digest(BINARY)}},
            "notices":[{"file":"LICENSE","sha256":files::digest(b"fixture license")}]
        }).to_string()).unwrap(), Download {
            url:"https://github.com/openai/codex/releases/download/fixture/codex.tar.gz".into(),
            archive_size: archive.len() as u64, archive_sha256:files::digest(archive), member:"codex-fixture".into(),
        })
    }

    #[test]
    fn embedded_download_pins_and_license_bytes_match_every_supported_target() {
        for architecture in embedded().manifest.targets.keys() {
            let package = pinned(architecture).unwrap();
            for pin in &package.release.manifest.notices {
                let bytes = NOTICES
                    .iter()
                    .find(|(name, _)| *name == pin.file)
                    .unwrap()
                    .1;
                assert_eq!(files::digest(bytes), pin.sha256, "{}", pin.file);
            }
        }
        assert!(pinned("unavailable-fixture").is_err());
    }

    #[test]
    fn concurrent_installers_publish_once_reuse_verified_files_and_never_execute_them() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("user data/rode/codex");
        private_directory(base.parent().unwrap()).unwrap();
        fs::write(
            base.parent().unwrap().join("preferences"),
            b"existing settings",
        )
        .unwrap();
        fs::set_permissions(base.parent().unwrap(), fs::Permissions::from_mode(0o775)).unwrap();
        let bytes = archive(&[("codex-fixture", tar::EntryType::Regular)]);
        let (release, download) = fixture(&bytes);
        let package = Package {
            release: &release,
            architecture: "fixture",
            download,
            notices: LICENSES,
        };
        let downloads = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        install_to(
                            &base,
                            &package,
                            &AtomicBool::new(false),
                            &mut |_, _, _| {},
                            || {
                                downloads.fetch_add(1, Ordering::Relaxed);
                                Ok(Box::new(Cursor::new(bytes.clone())))
                            },
                        )
                        .unwrap()
                    })
                })
                .collect();
            for job in jobs {
                let directory = job.join().unwrap();
                assert_eq!(fs::read(directory.join("codex")).unwrap(), BINARY);
                assert_eq!(
                    fs::read(directory.join("LICENSE")).unwrap(),
                    b"fixture license"
                );
                assert_eq!(
                    fs::metadata(directory.join("codex"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o755
                );
            }
        });
        assert_eq!(downloads.load(Ordering::Relaxed), 1);
        assert_eq!(
            fs::read(base.parent().unwrap().join("preferences")).unwrap(),
            b"existing settings"
        );
        assert_eq!(
            fs::metadata(base.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o022,
            0
        );
        assert_eq!(read_release(&base).unwrap().raw, release.raw);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 3); // version, lock, selector only
    }

    #[test]
    fn untrusted_archives_and_failed_verification_preserve_the_previous_selection() {
        for problem in [
            "hash",
            "truncated",
            "excess",
            "binary",
            "license",
            "name",
            "symlink",
            "hardlink",
            "extra",
        ] {
            let root = tempfile::tempdir().unwrap();
            let base = root.path().join("rode/codex");
            private_directory(&base).unwrap();
            fs::write(base.join("manifest.json"), b"previous selection").unwrap();
            let members = match problem {
                "symlink" => vec![("codex-fixture", tar::EntryType::Symlink)],
                "hardlink" => vec![("codex-fixture", tar::EntryType::Link)],
                "extra" => vec![
                    ("codex-fixture", tar::EntryType::Regular),
                    ("extra", tar::EntryType::Regular),
                ],
                "name" => vec![("unexpected", tar::EntryType::Regular)],
                _ => vec![("codex-fixture", tar::EntryType::Regular)],
            };
            let mut bytes = archive(&members);
            let (mut release, mut download) = fixture(&bytes);
            match problem {
                "hash" => download.archive_sha256 = "0".repeat(64),
                "truncated" => {
                    bytes.pop();
                }
                "excess" => bytes.push(1),
                "binary" => {
                    release.manifest.targets.get_mut("fixture").unwrap().sha256 = "0".repeat(64)
                }
                _ => {}
            }
            let package = Package {
                release: &release,
                architecture: "fixture",
                download,
                notices: if problem == "license" { &[] } else { LICENSES },
            };
            let result = install_to(
                &base,
                &package,
                &AtomicBool::new(false),
                &mut |_, _, _| {},
                || Ok(Box::new(Cursor::new(bytes))),
            );
            assert!(result.is_err(), "{problem}");
            assert_eq!(
                fs::read(base.join("manifest.json")).unwrap(),
                b"previous selection",
                "{problem}"
            );
            assert!(!base.join("9-fixture/fixture").exists(), "{problem}");
            assert!(!root.path().join("outside").exists());
            assert_eq!(
                fs::read_dir(base.join("9-fixture")).unwrap().count(),
                0,
                "{problem}"
            );
        }
    }

    #[test]
    fn cancellation_cleans_partial_downloads_and_extraction_without_changing_selection() {
        for phase in ["Downloading Codex…", "Extracting Codex…"] {
            let root = tempfile::tempdir().unwrap();
            let base = root.path().join("rode/codex");
            private_directory(&base).unwrap();
            fs::write(base.join("manifest.json"), b"previous selection").unwrap();
            let bytes = archive(&[("codex-fixture", tar::EntryType::Regular)]);
            let (release, download) = fixture(&bytes);
            let package = Package {
                release: &release,
                architecture: "fixture",
                download,
                notices: LICENSES,
            };
            let cancel = AtomicBool::new(false);
            let result = install_to(
                &base,
                &package,
                &cancel,
                &mut |label, done, _| {
                    if label == phase && done > 0 {
                        cancel.store(true, Ordering::Release);
                    }
                },
                || Ok(Box::new(Cursor::new(bytes))),
            );
            assert!(result.unwrap_err().to_string().contains("cancelled"));
            assert_eq!(
                fs::read(base.join("manifest.json")).unwrap(),
                b"previous selection"
            );
            assert_eq!(fs::read_dir(base.join("9-fixture")).unwrap().count(), 0);
            assert_eq!(fs::read_dir(&base).unwrap().count(), 3);
        }
    }

    #[test]
    fn installation_rejects_symlinked_directories_locks_and_damaged_existing_runtimes() {
        for problem in ["base", "lock", "destination", "damaged", "writable"] {
            let root = tempfile::tempdir().unwrap();
            let base = root.path().join("rode/codex");
            private_directory(&base).unwrap();
            let victim = root.path().join("victim");
            private_directory(&victim).unwrap();
            match problem {
                "base" => {
                    fs::remove_dir(&base).unwrap();
                    std::os::unix::fs::symlink(&victim, &base).unwrap();
                }
                "lock" => std::os::unix::fs::symlink(
                    victim.join("must-not-exist"),
                    base.join(".install.lock"),
                )
                .unwrap(),
                "writable" => {
                    fs::set_permissions(&base, fs::Permissions::from_mode(0o777)).unwrap()
                }
                _ => {
                    private_directory(&base.join("9-fixture")).unwrap();
                    if problem == "destination" {
                        std::os::unix::fs::symlink(&victim, base.join("9-fixture/fixture"))
                            .unwrap();
                    } else {
                        private_directory(&base.join("9-fixture/fixture")).unwrap();
                        fs::write(base.join("9-fixture/fixture/codex"), b"changed").unwrap();
                    }
                }
            }
            let bytes = archive(&[("codex-fixture", tar::EntryType::Regular)]);
            let (release, download) = fixture(&bytes);
            let package = Package {
                release: &release,
                architecture: "fixture",
                download,
                notices: LICENSES,
            };
            assert!(
                install_to(
                    &base,
                    &package,
                    &AtomicBool::new(false),
                    &mut |_, _, _| {},
                    || panic!("Unsafe install must not download")
                )
                .is_err(),
                "{problem}"
            );
            assert_eq!(fs::read_dir(&victim).unwrap().count(), 0);
        }
    }

    #[test]
    fn waiting_for_a_shared_install_lock_can_be_cancelled() {
        let root = tempfile::tempdir().unwrap();
        let first = install_lock(root.path(), &AtomicBool::new(false)).unwrap();
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let waiting = scope.spawn(|| install_lock(root.path(), &cancel));
            std::thread::sleep(Duration::from_millis(60));
            cancel.store(true, Ordering::Release);
            assert!(
                waiting
                    .join()
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("cancelled")
            );
        });
        drop(first);
        install_lock(root.path(), &AtomicBool::new(false)).unwrap();
    }

    #[test]
    #[ignore = "explicit HTTPS download of the pinned upstream runtime; no credentials or model"]
    fn official_native_download_installs_a_working_runtime_without_a_source_installer() {
        let root = tempfile::tempdir().unwrap();
        let package = pinned(std::env::consts::ARCH).unwrap();
        let installed = install_to(
            &root.path().join("rode/codex"),
            &package,
            &AtomicBool::new(false),
            &mut |_, _, _| {},
            || fetch(&package.download),
        )
        .unwrap();
        let output = Command::new(installed.join("codex"))
            .arg("--version")
            .env("CODEX_HOME", root.path().join("private-auth"))
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(&package.release.manifest.version)
        );
    }
}

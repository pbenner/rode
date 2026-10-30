//! Pinned native Codex discovery and on-demand SSH deployment.
//!
//! # Design decisions
//!
//! The optional runtime package carries an independent upstream Rust executable,
//! keeping Node.js, npm and VS Code out of the runtime requirements. Its protected
//! manifest selects a pinned version independently of editor updates. Package
//! installation (or an explicit user bundle override) is the trust boundary;
//! manifest checksums detect damaged assets, not a malicious package publisher.
//! Pins and notices are build inputs;
//! the application never downloads or updates executables from the internet.
//! Discovery, hashing and transfer run in the existing process worker. Remote copies
//! are immutable, versioned and published atomically under an owned cache lock.
//! Only the verified tool directory is mounted read-only into an active sandbox;
//! project grants, credentials, model servers and inference remain independent.

use crate::{files, project, sandbox, ssh};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(crate) const AUTO_CODEX: &str = "@rode/codex";
const PIN: &str = include_str!("../packaging/codex.json");

#[derive(Deserialize)]
struct Manifest {
    version: String,
    targets: BTreeMap<String, Target>,
    notices: Vec<Notice>,
}
#[derive(Deserialize)]
struct Target {
    size: u64,
    sha256: String,
}
#[derive(Deserialize)]
struct Notice {
    file: String,
    sha256: String,
}

struct Release {
    raw: String,
    manifest: Manifest,
}

struct Bundle {
    directory: PathBuf,
    release: Arc<Release>,
}

impl Release {
    fn parse(raw: String) -> Result<Self> {
        ensure!(
            raw.len() <= 32 * 1024,
            "Codex manifest exceeds its size limit"
        );
        let manifest: Manifest = serde_json::from_str(&raw)?;
        let version = &manifest.version;
        ensure!(
            !version.is_empty()
                && version.len() <= 64
                && version.as_bytes()[0].is_ascii_digit()
                && version
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".+-".contains(&c)),
            "Invalid Codex runtime version"
        );
        ensure!(
            !manifest.targets.is_empty() && manifest.targets.len() <= 16,
            "Invalid Codex target count"
        );
        let valid_hash = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        };
        for (architecture, target) in &manifest.targets {
            ensure!(
                !architecture.is_empty()
                    && architecture.len() <= 32
                    && architecture
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
                "Invalid Codex architecture"
            );
            ensure!(
                (1..=512 * 1024 * 1024).contains(&target.size) && valid_hash(&target.sha256),
                "Invalid Codex executable pin"
            );
        }
        ensure!(
            !manifest.notices.is_empty() && manifest.notices.len() <= 32,
            "Invalid Codex notice count"
        );
        let mut names = BTreeSet::new();
        for notice in &manifest.notices {
            ensure!(
                notice.file.len() <= 128
                    && Path::new(&notice.file).file_name().and_then(|n| n.to_str())
                        == Some(notice.file.as_str())
                    && !matches!(notice.file.as_str(), "codex" | "manifest.json" | "." | "..")
                    && names.insert(&notice.file)
                    && valid_hash(&notice.sha256),
                "Invalid Codex notice pin"
            );
        }
        Ok(Self { raw, manifest })
    }
}

fn embedded() -> &'static Arc<Release> {
    static VALUE: OnceLock<Arc<Release>> = OnceLock::new();
    VALUE.get_or_init(|| {
        Arc::new(Release::parse(PIN.into()).expect("embedded Codex runtime manifest"))
    })
}

fn protected(metadata: &fs::Metadata) -> bool {
    metadata.permissions().mode() & 0o022 == 0
        && (metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() })
}

/// The optional package's selector is bounded data from a trusted installation,
/// never a URL, executable probe or project-provided configuration file.
fn read_release(directory: &Path) -> Result<Arc<Release>> {
    let metadata = fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir() && protected(&metadata),
        "Unsafe Codex runtime directory"
    );
    let file = crate::persistence::open_regular(&directory.join("manifest.json"), false)?;
    ensure!(
        protected(&file.metadata()?),
        "Unsafe Codex runtime manifest"
    );
    let mut raw = String::new();
    file.take(32 * 1024 + 1).read_to_string(&mut raw)?;
    Ok(Arc::new(Release::parse(raw)?))
}

fn hash(path: &Path, limit: u64, cancel: &AtomicBool) -> Result<String> {
    let file = crate::persistence::open_regular(path, false)?;
    ensure!(
        file.metadata()?.len() <= limit,
        "Bundled Codex file exceeds its size limit"
    );
    let mut input = file.take(limit + 1);
    let mut digest = Sha256::new();
    let mut total = 0;
    let mut block = [0; 64 * 1024];
    loop {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Codex preparation cancelled"
        );
        let count = input.read(&mut block)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(
            total <= limit,
            "Bundled Codex file grew beyond its size limit"
        );
        digest.update(&block[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn verify(directory: &Path, release: &Release, target: &Target, cancel: &AtomicBool) -> Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir() && protected(&metadata),
        "Codex bundle must be a protected directory, not a symlink"
    );
    let expected: BTreeSet<_> = release
        .manifest
        .notices
        .iter()
        .map(|n| n.file.as_str())
        .chain(["codex", "manifest.json"])
        .collect();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        ensure!(
            expected.contains(entry.file_name().to_string_lossy().as_ref()),
            "Unexpected file in the Codex bundle"
        );
        let metadata = entry.metadata()?;
        ensure!(
            !entry.file_type()?.is_symlink() && metadata.is_file() && protected(&metadata),
            "Codex bundle contains an unsafe file"
        );
    }
    let binary = directory.join("codex");
    ensure!(
        crate::persistence::read(&directory.join("manifest.json"), release.raw.len())?.as_deref()
            == Some(release.raw.as_bytes()),
        "Codex bundle manifest does not match its runtime selector"
    );
    ensure!(
        binary.metadata()?.permissions().mode() & 0o111 != 0,
        "Bundled Codex is not executable"
    );
    ensure!(
        binary.metadata()?.len() == target.size
            && hash(&binary, target.size, cancel)? == target.sha256,
        "Bundled Codex failed its pinned size/SHA-256 check"
    );
    for notice in &release.manifest.notices {
        ensure!(
            hash(&directory.join(&notice.file), 1024 * 1024, cancel)? == notice.sha256,
            "Bundled Codex notice failed its SHA-256 check: {}",
            notice.file
        );
    }
    Ok(())
}

fn find(architecture: &str, cancel: &AtomicBool) -> Result<Option<Bundle>> {
    if let Some(path) = std::env::var_os("RODE_CODEX_BUNDLE") {
        let directory = PathBuf::from(path);
        let release = read_release(&directory)?;
        let Some(target) = release.manifest.targets.get(architecture) else {
            return Ok(None);
        };
        verify(&directory, &release, target, cancel)?;
        return Ok(Some(Bundle {
            directory: directory.canonicalize()?,
            release,
        }));
    }
    let exe = std::env::current_exe()?;
    let mut bin = exe
        .parent()
        .context("Cannot find RODE's binary directory")?;
    if bin.file_name().is_some_and(|n| n == "deps") {
        bin = bin.parent().context("Missing Cargo profile directory")?;
    }
    for base in [
        bin.join("rode-codex"),
        bin.join("../lib/rode/codex"),
        bin.join("../codex"),
    ] {
        let release = if base.join("manifest.json").try_exists()? {
            read_release(&base)?
        } else {
            embedded().clone()
        };
        // A missing upstream target affects only Codex. Installed/custom agents
        // and all other editor features remain available on that architecture.
        let Some(target) = release.manifest.targets.get(architecture) else {
            continue;
        };
        let path = base.join(&release.manifest.version).join(architecture);
        if path.try_exists()? {
            verify(&path, &release, target, cancel)
                .with_context(|| format!("Cannot use Codex bundle {}", path.display()))?;
            return Ok(Some(Bundle {
                directory: path.canonicalize()?,
                release,
            }));
        }
    }
    Ok(None)
}

fn installed() -> Option<PathBuf> {
    sandbox::find_program("codex").or_else(|| {
        let path = files::expand_home("~/.local/bin/codex");
        path.is_file().then_some(path)
    })
}

fn mounted_program(directory: &Path, plan: &sandbox::Plan) -> Result<(String, Vec<String>)> {
    let mut prefix = plan.prefix.clone();
    if !plan.active {
        return Ok((directory.join("codex").to_string_lossy().into(), prefix));
    }
    ensure!(
        prefix.pop().as_deref() == Some("--"),
        "Invalid sandbox plan"
    );
    prefix.extend([
        "--ro-bind".into(),
        directory.to_string_lossy().into(),
        "/rode-codex".into(),
        "--".into(),
    ]);
    Ok(("/rode-codex/codex".into(), prefix))
}

fn remote_call(
    remote: &ssh::Remote,
    spec: &serde_json::Value,
    input: impl Read + Send + 'static,
    cancel: &AtomicBool,
) -> Result<serde_json::Value> {
    ensure!(
        remote.connected(),
        "SSH is offline. Reconnect before preparing Codex"
    );
    let data = serde_json::to_string(spec)?;
    ensure!(
        data.len() < 96 * 1024,
        "Codex deployment metadata exceeds its limit"
    );
    let mut command = Command::new("ssh");
    command.args(remote.channel_args(false)).arg(format!(
        "python3 -c {} {}",
        ssh::quote(include_str!("remote_codex.py")),
        ssh::quote(&data)
    ));
    let output = project::run_controlled_stream(
        command,
        input,
        Duration::from_secs(180),
        cancel,
        false,
        64 * 1024,
        Duration::ZERO,
    )?
    .checked()?;
    serde_json::from_str(&output).context("Invalid Codex deployment reply")
}

/// Prepare only the selected tool assets. Call exclusively from a process worker.
pub(crate) fn prepare(
    plan: &sandbox::Plan,
    remote: Option<&ssh::Remote>,
    cancel: &AtomicBool,
) -> Result<(String, Vec<String>)> {
    if let Some(remote) = remote {
        let host = remote_call(
            remote,
            &serde_json::json!({"action":"host"}),
            std::io::empty(),
            cancel,
        )?;
        let architecture = host["architecture"]
            .as_str()
            .context("Missing remote architecture")?
            .to_owned();
        if host["platform"] != "linux" {
            if let Some(program) = host["installed"].as_str() {
                return Ok((program.into(), plan.prefix.clone()));
            }
            bail!("This SSH host needs a compatible Codex executable configured in Agent settings");
        }
        let bundle = find(&architecture, cancel)?;
        let release = bundle
            .as_ref()
            .map(|bundle| &bundle.release)
            .unwrap_or(embedded());
        let Some(target) = release.manifest.targets.get(&architecture) else {
            if let Some(program) = host["installed"].as_str() {
                return Ok((program.into(), plan.prefix.clone()));
            }
            bail!(
                "No Codex runtime for {architecture}. Configure a compatible executable on the SSH host or select another agent; editing remains available."
            );
        };
        let mut spec: serde_json::Value = serde_json::from_str(&release.raw)?;
        spec["action"] = "probe".into();
        let result = remote_call(remote, &spec, std::io::empty(), cancel)?;
        let path = result["executable"]
            .as_str()
            .context("Missing remote Codex path")?
            .to_owned();
        let suffix = Path::new(&release.manifest.version)
            .join(&architecture)
            .join(&target.sha256)
            .join("codex");
        ensure!(
            Path::new(&path).is_absolute() && Path::new(&path).ends_with(suffix),
            "Invalid remote Codex cache path"
        );
        if result["cached"] != true {
            let Some(bundle) = &bundle else {
                if let Some(program) = result["installed"].as_str() {
                    return Ok((program.into(), plan.prefix.clone()));
                }
                bail!(
                    "No optional Codex runtime for {architecture}. Install rode-codex, prepare that architecture with scripts/bundle-codex.py, or configure an installed executable on the SSH host."
                );
            };
            spec["action"] = "install".into();
            let mut notices = serde_json::Map::new();
            for notice in &release.manifest.notices {
                notices.insert(
                    notice.file.clone(),
                    fs::read_to_string(bundle.directory.join(&notice.file))?.into(),
                );
            }
            spec["notice_data"] = notices.into();
            remote_call(
                remote,
                &spec,
                crate::persistence::open_regular(&bundle.directory.join("codex"), false)?,
                cancel,
            )?;
        }
        return mounted_program(
            Path::new(&path)
                .parent()
                .context("Invalid remote Codex path")?,
            plan,
        );
    }
    if let Some(bundle) = find(std::env::consts::ARCH, cancel)? {
        return mounted_program(&bundle.directory, plan);
    }
    if let Some(program) = installed() {
        return Ok((program.to_string_lossy().into(), plan.prefix.clone()));
    }
    bail!(
        "Codex is unavailable. Install the optional rode-codex package, run python3 scripts/bundle-codex.py, or choose a compatible executable or another agent in Agent settings. Editing remains available."
    )
}

/// Forward a terminal command to the optional Codex runtime without opening a GUI.
/// Authentication remains owned by Codex and no process starts during editor startup.
pub fn codex(args: &[String]) -> Result<()> {
    let cancel = AtomicBool::new(false);
    let program = find(std::env::consts::ARCH, &cancel)?.map(|bundle| bundle.directory.join("codex")).or_else(installed)
        .context("Codex is unavailable. Install the optional rode-codex package, prepare it with python3 scripts/bundle-codex.py, or install a compatible Codex executable")?;
    Err(Command::new(program).args(args).exec()).context("Cannot run Codex")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_manifest_rejects_path_escapes_and_unbounded_pins() {
        let original: serde_json::Value = serde_json::from_str(PIN).unwrap();
        for (key, value) in [
            ("version", serde_json::json!("../outside")),
            (
                "notices",
                serde_json::json!([{"file":"../credentials", "sha256":"a".repeat(64)}]),
            ),
            (
                "notices",
                serde_json::json!([{"file":"manifest.json", "sha256":"a".repeat(64)}]),
            ),
            (
                "targets",
                serde_json::json!({"../host":{"size":1,"sha256":"a".repeat(64)}}),
            ),
            (
                "targets",
                serde_json::json!({"x86_64":{"size":0,"sha256":"a".repeat(64)}}),
            ),
            (
                "targets",
                serde_json::json!({"x86_64":{"size":513 * 1024 * 1024,"sha256":"a".repeat(64)}}),
            ),
            (
                "targets",
                serde_json::json!({"x86_64":{"size":1,"sha256":"bad"}}),
            ),
        ] {
            let mut value_under_test = original.clone();
            value_under_test[key] = value;
            assert!(
                Release::parse(value_under_test.to_string()).is_err(),
                "{key}"
            );
        }
        assert!(Release::parse(format!("{PIN}{}", " ".repeat(32 * 1024))).is_err());
    }

    #[test]
    fn runtime_selector_rejects_writable_or_symlinked_manifests() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("manifest.json");
        fs::write(&path, PIN).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_release(root.path()).is_ok());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(read_release(root.path()).is_err());
        fs::remove_file(&path).unwrap();
        fs::write(root.path().join("other.json"), PIN).unwrap();
        std::os::unix::fs::symlink(root.path().join("other.json"), &path).unwrap();
        assert!(read_release(root.path()).is_err());
    }

    #[test]
    fn bundled_asset_grant_is_read_only_and_does_not_expose_sibling_data() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let assets = root.path().join("assets");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&assets).unwrap();
        fs::write(root.path().join("private"), "private").unwrap();
        let binary = assets.join("codex");
        fs::write(&binary, format!("#!/bin/sh\nset -eu\n! test -e {}\n! touch /rode-codex/forbidden\necho ok > project-output\n", ssh::quote(root.path().join("private").to_str().unwrap()))).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let plan = sandbox::prepare(
            project.to_str().unwrap(),
            &sandbox::Options {
                enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        if !plan.active {
            eprintln!("Namespace check unavailable: {}", plan.reason);
            return;
        }
        let original = plan.prefix.clone();
        let (executable, prefix) = mounted_program(&assets, &plan).unwrap();
        assert_eq!(plan.prefix, original);
        assert_eq!(
            &prefix[..original.len() - 1],
            &original[..original.len() - 1]
        );
        let mut command = Command::new(&prefix[0]);
        command.args(&prefix[1..]).arg(executable);
        project::run(command, vec![], Duration::from_secs(5))
            .unwrap()
            .checked()
            .unwrap();
        assert_eq!(
            fs::read_to_string(project.join("project-output"))
                .unwrap()
                .trim(),
            "ok"
        );
        assert!(!assets.join("forbidden").exists());
    }
}

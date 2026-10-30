//! Native runtime selection and streamed, atomic SSH-cache publication without a
//! model, user credentials, Node.js, VS Code or a real SSH account.
use rode::{
    agent::{Config, Provider},
    files, project,
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};

fn call(root: &Path, spec: &Value, input: Vec<u8>) -> project::Output {
    let mut command = Command::new("python3");
    command
        .args([
            "-c",
            include_str!("../src/remote_codex.py"),
            &spec.to_string(),
        ])
        .env("RODE_CODEX_CACHE", root);
    project::run(command, input, Duration::from_secs(10)).unwrap()
}
fn fixture() -> (tempfile::TempDir, Vec<u8>, Value) {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let bytes = fs::read("/usr/bin/true").unwrap();
    let target = json!({"size":bytes.len(),"sha256":files::digest(&bytes)});
    let spec = json!({"version":"0-fixture","action":"install",
        "targets":{"x86_64":target,"aarch64":target},
        "notices":[{"file":"LICENSE","sha256":files::digest(b"fixture license\n")}],
        "notice_data":{"LICENSE":"fixture license\n"}});
    (root, bytes, spec)
}

#[test]
fn defaults_and_old_preferences_select_bundled_codex_but_overrides_remain_literal() {
    for provider in [Provider::Codex, Provider::Local] {
        for program in ["auto", "codex", ""] {
            let config = Config {
                provider,
                codex: program.into(),
                local_model: "fixture".into(),
                ..Default::default()
            };
            assert_eq!(config.argv().unwrap()[0], "@rode/codex");
        }
        for program in ["/custom/codex", "./tools/codex", "custom-codex"] {
            let config = Config {
                provider,
                codex: program.into(),
                local_model: "fixture".into(),
                ..Default::default()
            };
            assert_eq!(config.argv().unwrap()[0], program);
        }
    }
}

#[test]
fn streamed_cache_is_atomic_reusable_and_shared_between_concurrent_clients() {
    let (root, bytes, spec) = fixture();
    let cache = root.path().join("cache");
    let first = call(&cache, &spec, bytes.clone()).checked().unwrap();
    let executable = serde_json::from_str::<Value>(&first).unwrap()["executable"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut probe = spec.clone();
    probe["action"] = "probe".into();
    let info: Value =
        serde_json::from_str(&call(&cache, &probe, vec![]).checked().unwrap()).unwrap();
    assert_eq!(info["cached"], true);
    assert_eq!(fs::read(&executable).unwrap(), bytes);
    assert_eq!(
        fs::read_to_string(Path::new(&executable).with_file_name("LICENSE")).unwrap(),
        "fixture license\n"
    );
    std::thread::scope(|scope| {
        for _ in 0..3 {
            let bytes = bytes.clone();
            let spec = &spec;
            let cache = &cache;
            scope.spawn(move || {
                call(cache, spec, bytes).checked().unwrap();
            });
        }
    });
    // Valid bytes cannot conceal an unexpected file in the automatic sandbox grant.
    fs::write(
        Path::new(&executable).with_file_name("unrelated-private-file"),
        "private",
    )
    .unwrap();
    let output = call(&cache, &probe, vec![]);
    assert!(!output.success());
    assert!(output.stderr.contains("Unexpected Codex runtime files"));
}

#[test]
fn corrupt_truncated_and_excess_transfers_never_publish_or_replace_a_valid_runtime() {
    let (root, bytes, spec) = fixture();
    let cache = root.path().join("cache");
    call(&cache, &spec, bytes.clone()).checked().unwrap();
    for (index, bad) in [
        bytes[..bytes.len() - 1].to_vec(),
        vec![0; bytes.len()],
        [bytes.as_slice(), b"extra"].concat(),
    ]
    .into_iter()
    .enumerate()
    {
        let mut request = spec.clone();
        request["version"] = format!("0-bad-{index}").into();
        let output = call(&cache, &request, bad);
        assert!(!output.success(), "Invalid transfer was accepted");
        assert!(output.stderr.contains("pinned size"), "{}", output.stderr);
        let architecture = std::env::consts::ARCH;
        let directory = cache.join(format!("0-bad-{index}")).join(architecture);
        assert!(!directory.join(files::digest(&bytes)).exists());
        assert!(!fs::read_dir(directory).unwrap().any(|p| {
            p.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".incoming-")
        }));
    }
    let mut probe = spec.clone();
    probe["action"] = "probe".into();
    assert_eq!(
        serde_json::from_str::<Value>(&call(&cache, &probe, vec![]).checked().unwrap()).unwrap()["cached"],
        true
    );
}

#[test]
fn cache_symlinks_and_modified_executables_are_rejected() {
    use std::os::unix::fs::symlink;
    let (root, bytes, spec) = fixture();
    let victim = root.path().join("victim");
    fs::create_dir(&victim).unwrap();
    fs::write(victim.join("keep"), "untouched").unwrap();
    let cache = root.path().join("cache");
    symlink(&victim, &cache).unwrap();
    let output = call(&cache, &spec, bytes.clone());
    assert!(!output.success());
    assert!(output.stderr.contains("Unsafe Codex cache directory"));
    assert_eq!(
        fs::read_to_string(victim.join("keep")).unwrap(),
        "untouched"
    );
    fs::remove_file(&cache).unwrap();
    let output: Value =
        serde_json::from_str(&call(&cache, &spec, bytes.clone()).checked().unwrap()).unwrap();
    let executable = output["executable"].as_str().unwrap();
    fs::write(executable, vec![0; bytes.len()]).unwrap();
    let mut probe = spec.clone();
    probe["action"] = "probe".into();
    let output = call(&cache, &probe, vec![]);
    assert!(!output.success());
    assert!(output.stderr.contains("SHA-256"));
}

#[test]
fn a_changed_local_bundle_is_never_executed() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let marker = root.path().join("executed");
    let binary = root.path().join("codex");
    fs::write(
        &binary,
        format!(
            "#!/bin/sh\ntouch {}\n",
            rode::ssh::quote(marker.to_str().unwrap())
        ),
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        root.path().join("manifest.json"),
        include_str!("../packaging/codex.json"),
    )
    .unwrap();
    fs::set_permissions(
        root.path().join("manifest.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
    command
        .args(["--codex", "--version"])
        .env("RODE_CODEX_BUNDLE", root.path());
    let output = project::run(command, vec![], Duration::from_secs(5)).unwrap();
    assert!(!output.success());
    assert!(output.stderr.contains("SHA-256"), "{}", output.stderr);
    assert!(!marker.exists());
}

fn write_runtime(directory: &Path, architecture: &str) -> Value {
    fs::create_dir_all(directory).unwrap();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    let bytes = b"#!/bin/sh\nprintf 'optional-runtime-fixture\\n'\n";
    fs::write(directory.join("codex"), bytes).unwrap();
    fs::set_permissions(directory.join("codex"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(directory.join("LICENSE"), "fixture license\n").unwrap();
    let manifest = json!({"version":"9-independent-fixture",
        "targets":{architecture:{"size":bytes.len(),"sha256":files::digest(bytes)}},
        "notices":[{"file":"LICENSE","sha256":files::digest(b"fixture license\n")}]});
    fs::write(directory.join("manifest.json"), manifest.to_string()).unwrap();
    for name in ["manifest.json", "LICENSE"] {
        fs::set_permissions(directory.join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }
    manifest
}

#[test]
fn optional_package_selects_its_own_pinned_version_without_an_editor_rebuild() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("usr/bin");
    fs::create_dir_all(&bin).unwrap();
    let editor = bin.join("rode");
    fs::copy(env!("CARGO_BIN_EXE_rode"), &editor).unwrap();
    let base = root.path().join("usr/lib/rode/codex");
    let directory = base
        .join("9-independent-fixture")
        .join(std::env::consts::ARCH);
    let manifest = write_runtime(&directory, std::env::consts::ARCH);
    fs::set_permissions(&base, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(base.join("manifest.json"), manifest.to_string()).unwrap();
    fs::set_permissions(
        base.join("manifest.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let mut command = Command::new(&editor);
    command
        .args(["--codex", "--version"])
        .env_remove("RODE_CODEX_BUNDLE");
    assert_eq!(
        project::run(command, vec![], Duration::from_secs(5))
            .unwrap()
            .checked()
            .unwrap()
            .trim(),
        "optional-runtime-fixture"
    );
    // A damaged selector never redirects execution to a different payload.
    let mut damaged = manifest;
    damaged["version"] = "../outside".into();
    fs::write(base.join("manifest.json"), damaged.to_string()).unwrap();
    let mut command = Command::new(&editor);
    command
        .args(["--codex", "--version"])
        .env_remove("RODE_CODEX_BUNDLE");
    let output = project::run(command, vec![], Duration::from_secs(5)).unwrap();
    assert!(!output.success());
    assert!(
        output.stderr.contains("Invalid Codex runtime version"),
        "{}",
        output.stderr
    );
}

#[test]
fn a_missing_runtime_architecture_preserves_installed_codex_fallback() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("bundle");
    write_runtime(&bundle, "unsupported-fixture");
    let installed = root.path().join("bin");
    fs::create_dir(&installed).unwrap();
    fs::write(
        installed.join("codex"),
        "#!/bin/sh\necho installed-fallback\n",
    )
    .unwrap();
    fs::set_permissions(installed.join("codex"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
    command
        .args(["--codex", "--version"])
        .env("RODE_CODEX_BUNDLE", &bundle)
        .env("PATH", installed);
    assert_eq!(
        project::run(command, vec![], Duration::from_secs(5))
            .unwrap()
            .checked()
            .unwrap()
            .trim(),
        "installed-fallback"
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
    command
        .args(["--codex", "--version"])
        .env("RODE_CODEX_BUNDLE", &bundle)
        .env("PATH", "")
        .env("HOME", root.path());
    let output = project::run(command, vec![], Duration::from_secs(5)).unwrap();
    assert!(!output.success());
    assert!(
        output.stderr.contains("optional rode-codex"),
        "{}",
        output.stderr
    );
}

#[test]
fn host_discovery_needs_no_runtime_pin_and_does_not_modify_the_cache() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("absent-cache");
    let response: Value = serde_json::from_str(
        &call(&cache, &json!({"action":"host"}), vec![])
            .checked()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response["architecture"], std::env::consts::ARCH);
    assert_eq!(response["platform"], "linux");
    assert!(!cache.exists());
}

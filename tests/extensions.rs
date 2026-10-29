//! Native activation preferences and opt-in API v1 executable adapters with cancellation and
//! host-policy checks.

use rode::{
    extensions::{self, Feature, Registry},
    files, project,
};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn manifest() -> extensions::Manifest {
    serde_json::from_value(serde_json::json!({"api":1,"id":"test.adapter","name":"Test adapter","version":"1.0","description":"A test","tool":{"program":"python3","args":[concat!(env!("CARGO_MANIFEST_DIR"),"/tests/fixtures/extension.py")],"languages":["rust","text"],"format":true,"check":true,"commands":[{"id":"test.inspect","title":"Inspect"}]}})).unwrap()
}
#[test]
fn registry_keeps_bundled_features_and_never_activates_installed_tools_implicitly() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = manifest();
    let source = dir.path().join("source");
    fs::write(&source, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let destination = dir.path().join("extensions");
    extensions::install(&source, &destination).unwrap();
    assert!(extensions::install(&source, &destination).is_err());
    let mut registry = Registry::load(&destination, Default::default());
    assert!(registry.feature(Feature::Git));
    assert!(registry.feature(Feature::Binary));
    assert!(!registry.enabled("test.adapter"));
    assert!(registry.commands().is_empty());
    assert!(registry.provider("rust", true, true).is_none());
    registry.set_enabled("test.adapter", true);
    assert_eq!(registry.commands().len(), 1);
    assert!(registry.provider("rust", true, true).is_some());
    registry.set_enabled(Feature::Monitor.id(), false);
    assert!(!registry.feature(Feature::Monitor));
    assert!(registry.feature(Feature::Git));
    let text = serde_json::to_string(&registry.preferences).unwrap();
    let restored = Registry::load(&destination, extensions::preferences(&text).unwrap());
    assert!(restored.enabled("test.adapter"));
    assert!(!restored.feature(Feature::Monitor));
    let mut bad = manifest.clone();
    bad.id = "../outside".into();
    assert!(bad.validate().is_err());
    bad = manifest.clone();
    bad.api = 99;
    assert!(bad.validate().is_err());
    bad = manifest;
    bad.tool.program = "./from-project".into();
    assert!(bad.validate().is_err());
}
#[test]
fn adapters_format_unsaved_snapshots_and_match_the_python_peer_without_saving_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.rs");
    fs::write(&path, "disk").unwrap();
    let manifest = manifest();
    let registry = Registry::default();
    let invocation = registry.invocation(
        &manifest,
        "format".into(),
        Some(path.to_string_lossy().into()),
        "unsaved 🦀".into(),
    );
    let request = project::Request::Extension {
        root: root.path().to_string_lossy().into(),
        invocation,
    };
    let project::Response::Extension(local) = project::local(&request).unwrap() else {
        panic!()
    };
    assert_eq!(local.text.as_deref(), Some("UNSAVED 🦀"));
    assert_eq!(local.diagnostics[0].code, "DEMO");
    let mut child = Command::new("python3")
        .args(["-c", files::REMOTE_HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&files::Request::Tool { request }).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let peer: files::Response = serde_json::from_slice(&output.stdout).unwrap();
    let project::Response::Extension(reply) = peer.tool.unwrap() else {
        panic!()
    };
    reply.validate("format").unwrap();
    assert_eq!(local.text, reply.text);
    assert_eq!(fs::read_to_string(path).unwrap(), "disk");
    let reply = extensions::Reply {
        api: 1,
        text: Some("bad".into()),
        ..Default::default()
    };
    assert!(reply.validate("check").is_err());
}
#[test]
fn closing_a_coordinator_lifeline_stops_the_adapter_process() {
    let root = tempfile::tempdir().unwrap();
    let pid = root.path().join("pid");
    let manifest = manifest();
    let registry = Registry::default();
    let mut invocation = registry.invocation(&manifest, "test.inspect".into(), None, String::new());
    invocation
        .settings
        .insert("marker".into(), serde_json::json!(pid));
    invocation
        .settings
        .insert("sleep".into(), serde_json::json!(30));
    let request = project::Request::Extension {
        root: root.path().to_string_lossy().into(),
        invocation,
    };
    for remote in [false, true] {
        let _ = fs::remove_file(&pid);
        let mut command = if remote {
            let mut command = Command::new("python3");
            command.args(["-c", files::REMOTE_HELPER, "--extension-helper"]);
            command
        } else {
            let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
            command.arg("--extension-helper");
            command
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
        let start = Instant::now();
        while !pid.exists() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        let adapter: i32 = fs::read_to_string(&pid).unwrap().parse().unwrap();
        drop(input);
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "coordinator did not stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { libc::kill(adapter, 0) },
            -1,
            "adapter survived coordinator cancellation"
        );
    }
}

#[test]
fn coordinators_finish_successfully_while_the_lifetime_pipe_stays_open() {
    let root = tempfile::tempdir().unwrap();
    let manifest = manifest();
    let registry = Registry::default();
    let invocation = registry.invocation(&manifest, "format".into(), None, "normal finish".into());
    let request = project::Request::Extension {
        root: root.path().to_string_lossy().into(),
        invocation,
    };
    for remote in [false, true] {
        let mut command = if remote {
            let mut command = Command::new("python3");
            command.args(["-c", files::REMOTE_HELPER, "--extension-helper"]);
            command
        } else {
            let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
            command.arg("--extension-helper");
            command
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(4),
                "normal coordinator exit waited for lifetime EOF"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reply: extensions::Reply = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply.text.as_deref(), Some("NORMAL FINISH"));
        drop(input);
    }
}

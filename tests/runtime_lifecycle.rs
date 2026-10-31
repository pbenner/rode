//! Exercise desktop shutdown races against a real, private host service. Each
//! probe runs in a subprocess so service/helper environment stays test-local.
use rode::{runtime, sandbox, sessions};
use rode_runtime_core::service::{Info, Spec, Start};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

#[test]
fn runtime_shutdown_and_drop_preserve_the_correct_lifetime_during_startup() {
    for scenario in [
        "end_before_reply",
        "end_queued_reply",
        "end_connected",
        "drop_foreground",
        "drop_persistent",
        "drop_ending",
    ] {
        let root = tempfile::tempdir().unwrap();
        let helper = root.path().join("helper.py");
        fs::write(
            &helper,
            r#"#!/usr/bin/env python3
import os, sys, time
from pathlib import Path
if sys.argv[1:] == ['protocol'] and os.environ['RODE_LIFECYCLE_SCENARIO'] == 'end_before_reply':
    root = Path(os.environ['RODE_LIFECYCLE_ROOT'])
    (root / 'waiting').touch()
    deadline = time.monotonic() + 10
    while not (root / 'release').exists() and time.monotonic() < deadline:
        time.sleep(.01)
os.execv(os.environ['RODE_REAL_HELPER'], [os.environ['RODE_REAL_HELPER'], *sys.argv[1:]])
"#,
        )
        .unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime_lifetime_probe",
                "--ignored",
                "--nocapture",
            ])
            .env("RODE_SESSION_DIR", root.path().join("runtime"))
            .env("RODE_SESSION_HELPER", helper)
            .env("RODE_REAL_HELPER", env!("CARGO_BIN_EXE_rode-sessiond"))
            .env("RODE_LIFECYCLE_ROOT", root.path())
            .env("RODE_LIFECYCLE_SCENARIO", scenario)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{scenario}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

struct Host {
    plan: sandbox::Plan,
    id: String,
    policy: String,
}
impl Host {
    fn infos(&self) -> Vec<Info> {
        let reply = runtime::request(&self.plan, None, rode_session::Request::Runtimes).unwrap();
        let rode_session::Response::Runtimes(infos) = reply.response else {
            panic!()
        };
        infos
    }
    fn wait_absent(&self) {
        until(|| self.infos().iter().all(|info| info.id != self.id));
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        // Always release the startup gate before best-effort cleanup on failure.
        let _ = fs::write(PathBuf::from(&self.plan.root).join("release"), b"");
        let _ = runtime::request(
            &self.plan,
            None,
            rode_session::Request::EndRuntime {
                id: self.id.clone(),
                policy: self.policy.clone(),
            },
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let Ok(reply) = runtime::request(&self.plan, None, rode_session::Request::Runtimes)
            else {
                break;
            };
            if matches!(reply.response, rode_session::Response::Runtimes(infos) if infos.is_empty())
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = Command::new(env!("CARGO_BIN_EXE_rode-sessiond"))
            .arg("request")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|mut child| {
                use std::io::Write;
                let _ = child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(br#"{"action":"shutdown"}"#);
                let _ = child.wait();
            });
    }
}
fn until(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(
            Instant::now() < deadline,
            "Runtime lifetime transition timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "invoked by the isolated parent test"]
fn runtime_lifetime_probe() {
    let Ok(scenario) = std::env::var("RODE_LIFECYCLE_SCENARIO") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("RODE_LIFECYCLE_ROOT").unwrap());
    let mut plan = sandbox::Plan::default();
    plan.root = root.to_string_lossy().into();
    let host = Host {
        id: rode_session::new_id().unwrap(),
        policy: sessions::fingerprint(&plan),
        plan,
    };
    let bridge = include_str!("fixtures/notebook_bridge.py");
    let spec = Spec {
        sources: BTreeMap::new(),
        id: host.id.clone(),
        name: "Lifetime test".into(),
        root: host.plan.root.clone(),
        policy: host.policy.clone(),
        mode: host.plan.label().into(),
        document: root.join("demo.ipynb").to_string_lossy().into(),
        argv: vec!["python3".into(), "-u".into(), "-c".into(), bridge.into()],
        prefix: vec![],
        env: BTreeMap::new(),
        python: "off".into(),
        python_program: false,
        persistent: scenario != "drop_foreground",
        target: None,
        start: Start::Notebook {
            name: "python3".into(),
        },
    };
    let (ready, prepared) = mpsc::channel();
    let mut client = runtime::Client::start(
        host.plan.clone(),
        None,
        spec,
        Arc::new(move || {
            let _ = ready.send(());
        }),
    )
    .unwrap();
    if scenario == "end_before_reply" {
        until(|| root.join("waiting").exists());
        client.end();
        assert!(client.pending());
        assert!(client.cleanup().load(Ordering::Acquire));
        fs::write(root.join("release"), b"").unwrap();
    } else {
        // The callback runs after the creation reply has been published. Do not
        // poll the client: this is the exact window that used to orphan a job.
        prepared.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(client.info.is_none());
        if scenario == "end_connected" {
            until(|| {
                client.poll();
                client.connected()
            });
            assert!(client.error.is_none(), "{:?}", client.error);
        }
        match scenario.as_str() {
            "drop_foreground" => {
                drop(client);
                host.wait_absent();
                return;
            }
            "drop_persistent" => {
                drop(client);
                assert!(
                    host.infos()
                        .iter()
                        .any(|info| info.id == host.id && info.alive && info.persistent)
                );
                return;
            }
            "drop_ending" => {
                client.end();
                drop(client);
                host.wait_absent();
                return;
            }
            _ => client.end(),
        }
    }
    client.end(); // Duplicate Stop must not replace the shutdown owner.
    until(|| {
        client.poll();
        !client.pending()
    });
    assert!(client.error.is_none(), "{:?}", client.error);
    assert!(!client.cleanup().load(Ordering::Acquire));
    host.wait_absent();
}

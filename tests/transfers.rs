//! The same streaming helper used over SSH must retain the captured OS policy.
#[path = "support/native.rs"]
mod native_peer;
use rode_host_protocol::{
    framing,
    transfer::{Reply, Request},
};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

struct Peer {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
}
impl Peer {
    fn new(plan: &rode::sandbox::Plan) -> Self {
        let mut child = Command::new(native_peer::binary())
            .arg("--file-transfer")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        serde_json::to_writer(&mut input, plan).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
        let mut peer = Self {
            child,
            input: Some(input),
            output,
        };
        let reply: Result<Reply, String> = framing::read(&mut peer.output).unwrap().unwrap();
        reply.unwrap();
        peer
    }
    fn request(&mut self, request: Request) -> Result<Reply, String> {
        framing::write(self.input.as_mut().unwrap(), &request).unwrap();
        framing::read(&mut self.output).unwrap().unwrap()
    }
    fn finish(mut self) {
        drop(self.input.take());
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                break;
            }
            assert!(
                std::time::Instant::now() < until,
                "Transfer helper did not exit after EOF"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn native_stream_commits_complete_files_and_cleans_up_on_eof() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let root = tempfile::tempdir().unwrap();
    let plan = native_peer::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap();
    let target = root.path().join("complete.bin");
    let partial = root.path().join("partial.bin");
    let mut peer = Peer::new(&plan);
    for path in [&target, &partial] {
        peer.request(Request::OpenWrite {
            path: path.to_string_lossy().into(),
            expected: None,
        })
        .unwrap();
        peer.request(Request::Write {
            data: STANDARD.encode(b"binary\0bytes"),
        })
        .unwrap();
        if path == &target {
            peer.request(Request::Commit {
                size: 12,
                digest: rode::files::digest(b"binary\0bytes"),
                executable: true,
            })
            .unwrap();
        }
    }
    peer.finish();
    assert_eq!(fs::read(target).unwrap(), b"binary\0bytes");
    assert!(!partial.exists());
    assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rode-transfer-")
    }));
}

fn policy(backend: rode::sandbox::Backend, capability: &str, required: &str) {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let file = project.join("readable.txt");
    let outside = root.path().join("outside.txt");
    fs::write(&file, b"allowed").unwrap();
    fs::write(&outside, b"outside").unwrap();
    let plan = native_peer::prepare(
        project.to_str().unwrap(),
        &rode::sandbox::Options {
            enabled: true,
            backend,
            project_read_only: true,
            ..Default::default()
        },
    )
    .unwrap();
    if !plan.active {
        assert_ne!(
            std::env::var(required).as_deref(),
            Ok("1"),
            "Required transfer enforcement unavailable: {}",
            plan.reason
        );
        native_peer::capabilities::skip(capability, &plan.reason);
        return;
    }
    let mut peer = Peer::new(&plan);
    let entry = peer
        .request(Request::Stat {
            path: file.to_string_lossy().into(),
        })
        .unwrap()
        .entry
        .unwrap();
    peer.request(Request::OpenRead {
        path: file.to_string_lossy().into(),
        version: entry.version,
    })
    .unwrap();
    assert!(!peer.request(Request::Read).unwrap().data.is_empty());
    assert!(
        peer.request(Request::OpenWrite {
            path: project.join("blocked").to_string_lossy().into(),
            expected: None
        })
        .is_err()
    );
    let mut local = rode_host_core::transfer::Session::default();
    let entry = local
        .request(&Request::Stat {
            path: outside.to_string_lossy().into(),
        })
        .unwrap()
        .entry
        .unwrap();
    assert!(
        peer.request(Request::OpenRead {
            path: outside.to_string_lossy().into(),
            version: entry.version
        })
        .is_err()
    );
    assert!(!Path::new(&project.join("blocked")).exists());
    peer.finish();
}

#[test]
fn transfer_worker_enforces_bubblewrap_read_only_and_outside_grants() {
    policy(
        rode::sandbox::Backend::Bwrap,
        "bwrap",
        "RODE_TEST_REQUIRE_SANDBOX",
    );
}
#[test]
fn transfer_worker_enforces_landlock_read_only_and_outside_grants() {
    policy(
        rode::sandbox::Backend::Landlock,
        "landlock",
        "RODE_TEST_REQUIRE_LANDLOCK",
    );
}

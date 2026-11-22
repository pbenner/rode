//! Exercise the real lease/unmount implementation without gocryptfs, FUSE or
//! privileges. Only procfs observations and the unmount executable are fixtures.
use super::*;

struct Fixture {
    root: tempfile::TempDir,
    mount: Mount,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        let point = runtime.join("plain with spaces");
        let proc = root.path().join("proc");
        fs::create_dir_all(&point).unwrap();
        fs::create_dir_all(proc.join("self")).unwrap();
        let tool = root.path().join("unmount");
        // The tool must see the coordinator's directory handle already closed.
        // Inspect its parent rather than its own CLOEXEC-filtered descriptors.
        fs::write(
            &tool,
            format!(
                r#"#!/usr/bin/python3
import json
import os
import pathlib
import sys

root = pathlib.Path({})
point = str(root / 'runtime' / 'plain with spaces')
assert sys.argv[1:] == ['-u', '--', point], sys.argv
for fd in pathlib.Path('/proc/' + str(os.getppid()) + '/fd').iterdir():
    try:
        target = os.readlink(fd)
    except FileNotFoundError:
        continue
    assert target != point, 'Coordinator still holds the decrypted directory'
(root / 'called').write_text(json.dumps(sys.argv[1:]))
outcome = (root / 'outcome').read_text()
if outcome == 'busy':
    print('Device or resource busy', file=sys.stderr)
    sys.exit(1)
if outcome == 'unmount':
    (root / 'proc' / 'self' / 'mountinfo').write_text('')
"#,
                serde_json::to_string(&root.path()).unwrap(),
            ),
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let mut fixture = Self {
            mount: Mount {
                settings: Settings {
                    cipherdir: "/unused-cipher".into(),
                    // No encryption executable is ever needed by these tests.
                    program: "/nonexistent-gocryptfs".into(),
                    unmount_program: tool.to_string_lossy().into_owned(),
                },
                info: Info {
                    cipherdir: "/unused-cipher".into(),
                    mountpoint: point.to_string_lossy().into_owned(),
                },
                runtime,
                name: "rode-gocryptfs-fixture".into(),
                lease: None,
                directory: None,
                attempted: false,
                proc,
            },
            root,
        };
        fs::write(
            fixture.mount.proc.join("self/mountinfo"),
            format!(
                "10 9 0:42 / {} rw - fuse.gocryptfs {} rw\n",
                fixture.mount.info.mountpoint.replace(' ', "\\040"),
                fixture.mount.name,
            ),
        )
        .unwrap();
        fixture.outcome("unmount");
        fixture.mount.acquire().unwrap();
        fixture
    }

    fn outcome(&self, outcome: &str) {
        fs::write(self.root.path().join("outcome"), outcome).unwrap();
    }

    fn assert_retained(&self) {
        assert!(self.mount.directory.is_some());
        let lease = self.mount.lock_file("views.lock").unwrap();
        assert_ne!(
            unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0,
            "A refused lock must retain the view lease"
        );
    }
}

#[test]
fn normal_unmount_releases_handles_and_verifies_the_mount_disappeared() {
    let mut fixture = Fixture::new();
    assert!(fixture.mount.release(true, false).unwrap());
    assert!(fixture.root.path().join("called").is_file());
    assert!(fixture.mount.mounted().unwrap().is_none());
    assert!(fixture.mount.directory.is_none() && fixture.mount.lease.is_none());
}

#[test]
fn a_transient_inherited_view_descriptor_does_not_refuse_locking() {
    let mut fixture = Fixture::new();
    // dup retains the same flock description as fork, without forking Rust's
    // threaded test runner. Exec would release this CLOEXEC reference shortly.
    let inherited = fixture.mount.lease.as_ref().unwrap().try_clone().unwrap();
    let child = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        drop(inherited);
    });
    let result = fixture.mount.release(true, false);
    child.join().unwrap();
    assert!(result.unwrap());
}

#[test]
fn reacquiring_a_view_waits_for_transient_exclusive_descriptors() {
    let mut fixture = Fixture::new();
    fixture.mount.directory = None;
    fixture.mount.lease = None;
    let exclusive = fixture.mount.views(libc::LOCK_EX, "busy").unwrap();
    let child = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        drop(exclusive);
    });
    let result = fixture.mount.acquire();
    child.join().unwrap();
    result.unwrap();
    fixture.assert_retained();
}

#[test]
fn busy_unmount_retains_the_lease_and_can_be_retried() {
    let mut fixture = Fixture::new();
    fixture.outcome("busy");
    let error = fixture.mount.release(true, false).unwrap_err();
    assert!(
        error.to_string().contains("Device or resource busy"),
        "{error:#}"
    );
    fixture.assert_retained();
    fixture.outcome("unmount");
    assert!(fixture.mount.release(true, false).unwrap());
}

#[test]
fn successful_tool_exit_does_not_count_as_a_verified_unmount() {
    let mut fixture = Fixture::new();
    fixture.outcome("unchanged");
    let error = fixture.mount.release(true, false).unwrap_err();
    assert!(error.to_string().contains("still mounted"), "{error:#}");
    fixture.assert_retained();
}

#[test]
fn another_window_blocks_lock_but_allows_releasing_a_shared_view() {
    for close_shared in [false, true] {
        let mut fixture = Fixture::new();
        let other = fixture.mount.lock_file("views.lock").unwrap();
        assert_eq!(unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_SH) }, 0);
        let error = fixture.mount.release(true, close_shared).unwrap_err();
        assert!(error.to_string().starts_with("Another RODE window"));
        assert!(!fixture.root.path().join("called").exists());
        assert_eq!(fixture.mount.lease.is_none(), close_shared);
        assert_eq!(fixture.mount.directory.is_none(), close_shared);
        drop(other);
        assert!(fixture.mount.release(true, false).unwrap());
    }
}

#[test]
fn sandbox_mount_copies_block_unmount_until_released() {
    let mut fixture = Fixture::new();
    let process = fixture.mount.proc.join("123");
    fs::create_dir(&process).unwrap();
    fs::write(
        process.join("mountinfo"),
        "11 9 0:42 / /sandbox rw - fuse.gocryptfs rode-gocryptfs-fixture rw\n",
    )
    .unwrap();
    let error = fixture.mount.release(true, false).unwrap_err();
    assert!(
        error.to_string().contains("another process or sandbox"),
        "{error:#}"
    );
    assert!(!fixture.root.path().join("called").exists());
    fixture.assert_retained();
    fs::write(process.join("mountinfo"), "").unwrap();
    assert!(fixture.mount.release(true, false).unwrap());
}

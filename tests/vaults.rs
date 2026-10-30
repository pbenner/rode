//! Encrypted-mount identity, lease/locking safeguards and native/Python lifecycle parity, using
//! real FUSE only when available.

use rode::{files, vault};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};
struct Peer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Peer {
    fn new(request: &vault::Open, python: bool) -> Self {
        let mut command = if python {
            Command::new("python3")
        } else {
            Command::new(env!("CARGO_BIN_EXE_rode"))
        };
        if python {
            command.args(["-u", "-c", include_str!("../src/remote_vault.py")]);
        } else {
            command.arg("--vault-helper");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut peer = Self {
            child,
            input,
            output,
        };
        peer.input
            .write_all(&serde_json::to_vec(request).unwrap())
            .unwrap();
        peer.input.write_all(b"\n").unwrap();
        peer.input.flush().unwrap();
        peer
    }
    fn next(&mut self) -> vault::Reply {
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        assert!(
            !line.is_empty(),
            "Vault helper exited: {:?}",
            self.child.try_wait()
        );
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"))
    }
    fn action(&mut self, action: &str) -> vault::Reply {
        writeln!(self.input, "{}", serde_json::json!({"action":action})).unwrap();
        self.input.flush().unwrap();
        self.next()
    }
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.stop();
    }
}
fn request(path: &Path, program: &str, create: bool) -> vault::Open {
    vault::Open {
        settings: vault::Settings {
            cipherdir: path.to_string_lossy().into(),
            program: program.into(),
            ..Default::default()
        },
        password: "RODE-password-secret-9410".into(),
        create,
        lock_only: false,
    }
}
fn runtime(path: &Path) -> PathBuf {
    PathBuf::from(format!("/tmp/rode-vaults-{}", unsafe { libc::geteuid() })).join(files::digest(
        path.canonicalize().unwrap().to_string_lossy().as_bytes(),
    ))
}
fn cleanup_runtime(path: &Path) {
    let runtime = runtime(path);
    let point = runtime.join("plain");
    // The test's own empty mountpoint is inaccessible when locked. Never
    // recursively delete a mounted tree or an encrypted backing directory.
    if point.exists() {
        fs::set_permissions(&point, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir(&point).unwrap();
    }
    for file in ["operation.lock", "views.lock"] {
        let _ = fs::remove_file(runtime.join(file));
    }
    fs::remove_dir(runtime).unwrap();
}
#[test]
fn fast_python_peer_tools_preserve_exit_status_and_redacted_diagnostics() {
    let source = include_str!("../src/remote_vault.py");
    let script = format!(
        r#"
import subprocess, sys
namespace = {{"__name__": "vault_test"}}
exec({source:?}, namespace)
class AlreadyExited(subprocess.Popen):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.wait(timeout=5)
subprocess.Popen = AlreadyExited
assert namespace["run_tool"]("/usr/bin/true", [], "fixture-password") == (0, "")
code, message = namespace["run_tool"](sys.executable,
    ["-c", "import sys; sys.stderr.write('fixture-password rejected'); sys.exit(7)"],
    "fixture-password")
assert code == 7, (code, message)
assert message == "[redacted] rejected", message
"#
    );
    let output = Command::new("python3")
        .args(["-c", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn a_successful_tool_without_a_real_mount_never_opens_plaintext() {
    for python in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let cipher = root.path().join("storage with 'quotes'");
        fs::create_dir(&cipher).unwrap();
        fs::write(cipher.join("gocryptfs.conf"), "test fixture").unwrap();
        let mut peer = Peer::new(&request(&cipher, "/usr/bin/true", false), python);
        let vault::Reply::Error { message } = peer.next() else {
            panic!("A non-mount was accepted");
        };
        assert!(message.contains("verified encrypted mount"), "{message}");
        assert!(!message.contains("RODE-password-secret"));
        let point = runtime(&cipher).join("plain");
        assert_eq!(
            fs::metadata(&point).unwrap().permissions().mode() & 0o777,
            0
        );
        assert!(fs::write(point.join("never-plaintext"), "secret").is_err());
        drop(peer);
        cleanup_runtime(&cipher);
    }
}
#[test]
fn initialization_refuses_existing_plaintext_and_invalid_passwords() {
    for python in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let cipher = root.path().join("plain");
        fs::create_dir(&cipher).unwrap();
        fs::write(cipher.join("important.txt"), "keep source").unwrap();
        let mut peer = Peer::new(&request(&cipher, "/usr/bin/true", true), python);
        let vault::Reply::Error { message } = peer.next() else {
            panic!()
        };
        assert!(message.contains("must be empty"), "{message}");
        assert_eq!(
            fs::read_to_string(cipher.join("important.txt")).unwrap(),
            "keep source"
        );
        assert!(!cipher.join("gocryptfs.conf").exists());
        drop(peer);
        cleanup_runtime(&cipher);
        let mut invalid = request(root.path(), "/usr/bin/true", false);
        invalid.password = "bad\npassword".into();
        let mut peer = Peer::new(&invalid, python);
        let vault::Reply::Error { message } = peer.next() else {
            panic!()
        };
        assert!(message.contains("without line breaks"), "{message}");
        drop(peer);
        cleanup_runtime(root.path());
    }
}
#[test]
fn installed_gocryptfs_initializes_and_mounts_when_fuse_is_available() {
    if Command::new("gocryptfs").arg("-version").output().is_err() {
        eprintln!("Installed gocryptfs is needed for the real encryption test");
        return;
    }
    for python in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let cipher = root.path().join("encrypted");
        let open = request(&cipher, "gocryptfs", true);
        let mut first = Peer::new(&open, python);
        let reply = first.next();
        let config = fs::read_to_string(cipher.join("gocryptfs.conf")).unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&config).unwrap()["EncryptedKey"].is_string()
        );
        assert!(!config.contains(&open.password));
        if !Path::new("/dev/fuse").exists() {
            let vault::Reply::Error { message } = reply else {
                panic!("Expected a FUSE availability error");
            };
            assert!(message.to_lowercase().contains("fuse"), "{message}");
            assert!(!message.contains(&open.password));
            eprintln!(
                "Real gocryptfs initialization passed; mounting cannot run without /dev/fuse"
            );
        } else {
            let vault::Reply::Opened { info } = reply else {
                panic!("Real gocryptfs did not mount");
            };
            let file = Path::new(&info.mountpoint).join("confidential.txt");
            files::local(&files::Request::Write {
                path: file.to_string_lossy().into(),
                text: "RODE encrypted payload 4821".into(),
                expected: None,
            })
            .unwrap();
            assert_eq!(
                fs::read_to_string(&file).unwrap(),
                "RODE encrypted payload 4821"
            );
            assert!(!cipher.join("confidential.txt").exists());
            let mut second_request = request(&cipher, "gocryptfs", false);
            second_request.password.clear();
            let mut second = Peer::new(&second_request, !python);
            assert!(matches!(second.next(), vault::Reply::Opened { .. }));
            let vault::Reply::Closed {
                locked, message, ..
            } = first.action("lock")
            else {
                panic!()
            };
            assert!(
                !locked && message.contains("Another RODE window"),
                "{message}"
            );
            assert_eq!(
                fs::read_to_string(&file).unwrap(),
                "RODE encrypted payload 4821"
            );
            // Concurrent close releases one shared view. The final view must
            // verify unmount or retain its lease if namespace checks are denied.
            for peer in [&mut first, &mut second] {
                writeln!(peer.input, "{{\"action\":\"close\"}}").unwrap();
                peer.input.flush().unwrap();
            }
            let replies = [first.next(), second.next()];
            let details = serde_json::to_string(&replies).unwrap();
            let (mut shared, mut locked, mut restricted) = (0, 0, 0);
            for reply in &replies {
                match reply {
                    vault::Reply::Closed { locked: true, .. } => locked += 1,
                    vault::Reply::Closed {
                        locked: false,
                        message,
                        ..
                    } if message.starts_with(
                        "Another RODE window still has this encrypted project open.",
                    ) =>
                    {
                        shared += 1
                    }
                    vault::Reply::Closed {
                        locked: false,
                        message,
                        ..
                    } if message.starts_with(
                        "Cannot verify other mount namespaces; project remains unlocked",
                    ) =>
                    {
                        restricted += 1
                    }
                    _ => panic!(
                        "Unexpected concurrent close reply (first Python={python}): {details}"
                    ),
                }
            }
            assert_eq!(
                shared, 1,
                "Exactly one concurrent close must release a shared lease (first Python={python}): {details}"
            );
            assert_eq!(
                locked + restricted,
                1,
                "The final close must lock or explicitly refuse unverifiable namespaces (first Python={python}): {details}"
            );
            if restricted == 1 {
                // FUSE access does not imply permission to inspect every other
                // same-UID process namespace (e.g. on a hardened CI runner).
                // Refusing to claim a lock is the required production behavior.
                assert_eq!(
                    fs::read_to_string(&file).unwrap(),
                    "RODE encrypted payload 4821"
                );
                let lease = fs::File::open(runtime(&cipher).join("views.lock")).unwrap();
                let result =
                    unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                let error = std::io::Error::last_os_error();
                assert_eq!(result, -1, "A refused lock lost the final view lease");
                assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
                drop(lease);
                eprintln!("Host namespace verification prevents a verified lock: {details}");
                // Stop only our lease peers, then normally unmount only this
                // fixture. This cleanup is not a verified RODE lock and never
                // uses force/lazy unmount or relaxes the application guard.
                first.stop();
                second.stop();
                let program = rode::sandbox::find_program("fusermount3")
                    .or_else(|| rode::sandbox::find_program("fusermount"))
                    .expect("FUSE fixture cleanup requires fusermount3/fusermount");
                let mut command = Command::new(program);
                command
                    .args(["-u", "--", &info.mountpoint])
                    .current_dir("/");
                rode::project::run(command, vec![], std::time::Duration::from_secs(10))
                    .unwrap()
                    .checked()
                    .unwrap();
            }
            drop(second);

            assert!(fs::read_to_string(&file).is_err());
        }
        drop(first);
        // Password validation is independent of whether the kernel permits FUSE.
        let mut wrong = request(&cipher, "gocryptfs", false);
        wrong.password = "incorrect-password".into();
        let mut peer = Peer::new(&wrong, python);
        let vault::Reply::Error { message } = peer.next() else {
            panic!()
        };
        assert!(
            message.contains("12") || message.to_lowercase().contains("password"),
            "{message}"
        );
        drop(peer);
        cleanup_runtime(&cipher);
    }
}

#[test]
fn python_peer_releases_a_shared_close_lease_atomically_without_fuse() {
    let root = tempfile::tempdir().unwrap();
    let code = r#"
import fcntl, runpy, sys
scope = runpy.run_path(sys.argv[1], run_name='vault_unit_fixture')
class MockMount(scope['Mount']):
    inspection_error = None
    def mounted(self):
        return 'fixture'
    def unmount_locked(self):
        # Only the actual FUSE syscall is replaced: operation locks, view leases,
        # failure retention and shared close are the production implementation.
        with self.lock_file('views.lock') as lease:
            try:
                fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise ValueError('Another RODE window still has this encrypted project open.') from None
            if self.inspection_error:
                raise ValueError(self.inspection_error)
settings = dict(cipherdir=sys.argv[2], program='gocryptfs', unmount_program='')
first, second = MockMount(settings, False), MockMount(settings, False)
for mount in (first, second):
    with mount.operation():
        mount.acquire()
reply = scope['close'](first, retain_on_failure=True)
assert not reply['locked'] and first.lease is not None
reply = scope['close'](first, retain_on_failure=True, close_shared=True)
assert not reply['locked'] and first.lease is None
# Namespace inspection refusal must keep the final lease even on window close.
# Only the known shared-view refusal permits abandoning a lease without locking.
second.inspection_error = 'Cannot verify other mount namespaces; project remains unlocked.'
reply = scope['close'](second, retain_on_failure=True, close_shared=True)
assert not reply['locked'] and reply['message'] == second.inspection_error
assert second.lease is not None and second.directory is not None
with second.lock_file('views.lock') as exclusive:
    try:
        fcntl.flock(exclusive, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        pass
    else:
        raise AssertionError('A refused lock lost the final view lease')
second.inspection_error = None
reply = scope['close'](second, retain_on_failure=True, close_shared=True)
assert reply['locked'] and second.lease is None
"#;
    let output = Command::new("python3")
        .args(["-c", code])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/remote_vault.py"))
        .arg(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    cleanup_runtime(root.path());
}

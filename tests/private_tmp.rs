//! Exercise the real desktop/helper startup boundary without opening a GUI.
#[path = "support/capabilities.rs"]
mod capabilities;

use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
    process::{Command, Stdio},
};

fn private_home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

fn sandbox_temporary_storage(backend: rode::sandbox::Backend) {
    use rode::{files, sandbox};
    use std::time::Duration;

    let (capability, required) = match backend {
        sandbox::Backend::Bwrap => ("bwrap", "RODE_TEST_REQUIRE_SANDBOX"),
        sandbox::Backend::Landlock => ("landlock", "RODE_TEST_REQUIRE_LANDLOCK"),
    };
    for executable in [
        env!("CARGO_BIN_EXE_rode"),
        env!("CARGO_BIN_EXE_rode-sessiond"),
    ] {
        for read_only in [false, true] {
            let home = private_home();
            let temporary = rode_host_core::private_tmp::root(home.path()).unwrap();
            // Exercise the encrypted mount's directory layout without requiring
            // FUSE; this fixture contains only ordinary synthetic files.
            let vault = temporary.join("vaults-fixture/first");
            let project = vault.join("plain");
            let other = temporary.join("vaults-fixture/second/plain");
            for path in [&project, &other] {
                fs::create_dir_all(path).unwrap();
            }
            let source = project.join("source.txt");
            fs::write(&source, "project source").unwrap();
            let lock = vault.join("views.lock");
            let secret = other.join("secret.txt");
            for path in [&lock, &secret] {
                fs::write(path, "host only").unwrap();
            }
            let mut prepare = Command::new(executable);
            prepare.arg("--file-helper").env("HOME", home.path());
            let response = rode_host_core::command::run_bounded_limited(
                prepare,
                serde_json::to_vec(&files::Request::Prepare {
                    path: project.to_string_lossy().into_owned(),
                    sandbox: sandbox::Options {
                        enabled: true,
                        backend,
                        project_read_only: read_only,
                        ..Default::default()
                    },
                })
                .unwrap(),
                Duration::from_secs(20),
                64 * 1024,
            )
            .unwrap();
            let plan = serde_json::from_slice::<files::Response>(&response)
                .unwrap()
                .project
                .unwrap();
            if !plan.active {
                assert_ne!(
                    std::env::var(required).as_deref(),
                    Ok("1"),
                    "{}",
                    plan.reason
                );
                capabilities::skip(capability, &plan.reason);
                continue;
            }
            let scratch = rode_host_core::private_tmp::project(home.path(), &project)
                .unwrap()
                .0;
            let mut command = Command::new(&plan.prefix[0]);
            command
                .args(&plan.prefix[1..])
                .env("HOME", home.path())
                .args([
                    "/usr/bin/python3",
                    "-I",
                    "-c",
                    r#"
import errno, os, pathlib, subprocess, sys, tempfile
source, scratch, lock, secret = map(pathlib.Path, sys.argv[1:5])
assert pathlib.Path.cwd() == source.parent
assert source.read_text() == 'project source'
for name in ('TMPDIR', 'TMP', 'TEMP'):
    assert pathlib.Path(os.environ[name]) == scratch, (name, os.environ[name], scratch)
assert scratch.stat().st_mode & 0o777 == 0o700
with tempfile.TemporaryDirectory() as folder:
    assert pathlib.Path(folder).parent == scratch
    file = pathlib.Path(folder) / 'temporary.txt'
    file.write_text('scratch is writable')
    assert file.stat().st_mode & 0o777 == 0o600
subprocess.run([sys.executable, '-I', '-c',
    "import tempfile; f = tempfile.TemporaryFile(); f.write(b'child scratch')"], check=True)
for path in (lock, secret):
    try:
        path.read_text()
    except OSError as error:
        assert error.errno in (errno.EACCES, errno.EPERM, errno.ENOENT), error
    else:
        raise AssertionError('Unrelated private storage is exposed: ' + str(path))
try:
    source.write_text('edited')
except OSError as error:
    assert sys.argv[5] == 'read-only'
    assert error.errno in (errno.EACCES, errno.EPERM, errno.EROFS), error
else:
    assert sys.argv[5] == 'writable'
print('private scratch available with project access preserved')
"#,
                ]);
            command.args([
                source.as_path(),
                scratch.as_path(),
                lock.as_path(),
                secret.as_path(),
            ]);
            command.arg(if read_only { "read-only" } else { "writable" });
            let output = rode_host_core::command::run_bounded_limited(
                command,
                vec![],
                Duration::from_secs(15),
                64 * 1024,
            )
            .unwrap();
            assert!(
                String::from_utf8(output)
                    .unwrap()
                    .contains("private scratch available")
            );
            // Native helper startup must retain the sandbox's TMPDIR too.
            let mut helper = Command::new(&plan.prefix[0]);
            helper
                .args(&plan.prefix[1..])
                .env("HOME", home.path())
                .args([executable, "--file-helper"]);
            let response = rode_host_core::command::run_bounded_limited(
                helper,
                serde_json::to_vec(&files::Request::Read {
                    path: source.to_string_lossy().into_owned(),
                })
                .unwrap(),
                Duration::from_secs(15),
                64 * 1024,
            )
            .unwrap();
            let response = serde_json::from_slice::<files::Response>(&response).unwrap();
            assert_eq!(
                response.text,
                if read_only {
                    "project source"
                } else {
                    "edited"
                }
            );
            assert_eq!(fs::read_to_string(&lock).unwrap(), "host only");
            assert_eq!(fs::read_to_string(&secret).unwrap(), "host only");
        }
    }
}

#[test]
fn bubblewrap_provides_private_temporary_storage_for_nested_and_read_only_projects() {
    sandbox_temporary_storage(rode::sandbox::Backend::Bwrap);
}

#[test]
fn landlock_provides_private_temporary_storage_for_nested_and_read_only_projects() {
    sandbox_temporary_storage(rode::sandbox::Backend::Landlock);
}

#[test]
fn desktop_and_helper_children_use_private_home_storage_instead_of_inherited_tmp() {
    for executable in [
        env!("CARGO_BIN_EXE_rode"),
        env!("CARGO_BIN_EXE_rode-sessiond"),
    ] {
        for sandboxed in [false, true] {
            let fixture = tempfile::tempdir().unwrap();
            let home = fixture.path().join("home");
            let project = fixture.path().join("project");
            fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
            fs::create_dir(&project).unwrap();
            let expected = if sandboxed {
                rode_host_core::private_tmp::project(&home, &project)
                    .unwrap()
                    .0
            } else {
                home.join(".cache/rode/tmp")
            };
            let spec = serde_json::json!({
                "heartbeat": false, "environment": {}, "root": project,
                "prefix": [], "python": "python3", "python_program": false,
                "argv": ["/bin/sh", "-c", "printf '%s\\n' \"$TMPDIR\" \"$TMP\" \"$TEMP\"; umask; : > \"$TMPDIR/probe-file\"; mkdir \"$TMPDIR/probe-directory\""]
            });
            let mut child = Command::new(executable)
                .arg("--process")
                .env("HOME", &home)
                .env(
                    "TMPDIR",
                    if sandboxed {
                        expected.as_path()
                    } else {
                        Path::new("/tmp")
                    },
                )
                .env("TMP", "/tmp")
                .env("TEMP", "/tmp")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            // Keep the supervisor's lifetime channel open until its child exits.
            let mut input = child.stdin.take().unwrap();
            writeln!(input, "{spec}").unwrap();
            let result = child.wait_with_output().unwrap();
            assert!(
                result.status.success(),
                "{executable}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let output = String::from_utf8(result.stdout).unwrap();
            let lines = output.lines().collect::<Vec<_>>();
            assert_eq!(lines.len(), 4, "{output:?}");
            for line in &lines[..3] {
                assert_eq!(Path::new(line), expected);
            }
            assert_eq!(u32::from_str_radix(lines[3], 8).unwrap(), 0o077);
            assert_eq!(
                expected.metadata().unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                expected
                    .join("probe-file")
                    .metadata()
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                expected
                    .join("probe-directory")
                    .metadata()
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }
}

#[test]
fn startup_refuses_public_or_redirected_temporary_storage() {
    for executable in [
        env!("CARGO_BIN_EXE_rode"),
        env!("CARGO_BIN_EXE_rode-sessiond"),
    ] {
        let home = private_home();
        let path = rode_host_core::private_tmp::root(home.path()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let result = Command::new(executable)
            .arg("--host-info")
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("owner-only"));
        fs::remove_dir(&path).unwrap();
        std::os::unix::fs::symlink(home.path(), &path).unwrap();
        let result = Command::new(executable)
            .arg("--host-info")
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Unsafe temporary directory"));
    }
}

#[test]
fn native_service_state_and_home_less_terminal_requests_stay_in_the_host_home() {
    use rode_session::{Request, Response};
    use std::{
        collections::BTreeMap,
        time::{Duration, Instant},
    };

    let home = private_home();
    let rpc = |request: &Request| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rode-sessiond"))
            .arg("request")
            .env("HOME", home.path())
            .env_remove("RODE_SESSION_DIR")
            .env("TMPDIR", "/tmp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(request).unwrap())
            .unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice::<Response>(&result.stdout)
            .unwrap()
            .checked()
            .unwrap()
    };
    let id = rode_session::new_id().unwrap();
    let policy = "a".repeat(64);
    let script = concat!(
        "{ printf '%s\\n' \"$TMPDIR\" \"$HOME\"; umask; } ",
        "> \"$TMPDIR/service-probe\"; while read -r line; do :; done"
    );
    let request = Request::Create {
        id: id.clone(),
        policy: policy.clone(),
        name: "Private temporary storage".into(),
        root: home.path().to_string_lossy().into_owned(),
        mode: "NORMAL MODE".into(),
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env: BTreeMap::from([("PATH".into(), "/bin".into())]),
        initial_input: String::new(),
        persistent: true,
        allocation: None,
    };
    assert!(matches!(rpc(&request), Response::Session(_)));
    let temporary = home.path().join(".cache/rode/tmp");
    let probe = temporary.join("service-probe");
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        if let Ok(text) = fs::read_to_string(&probe)
            && text.lines().count() == 3
        {
            break Some(text);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // Stop retained work before asserting, including when the probe did not finish.
    rpc(&Request::End { id, policy });
    let text = result.expect("Native terminal did not write its private-home probe");
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(Path::new(lines[0]), temporary);
    assert_eq!(Path::new(lines[1]), home.path());
    assert_eq!(u32::from_str_radix(lines[2], 8).unwrap(), 0o077);
    assert_eq!(
        probe.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    let runtimes = fs::read_dir(home.path().join(".cache/rode"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("session-")
        })
        .collect::<Vec<_>>();
    assert_eq!(runtimes.len(), 1);
    assert_eq!(
        runtimes[0].metadata().unwrap().permissions().mode() & 0o777,
        0o700
    );
}

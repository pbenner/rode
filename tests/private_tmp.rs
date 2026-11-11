//! Exercise the real desktop/helper startup boundary without opening a GUI.
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

//! Regression checks for special files, canonical grants and inherited terminal access.
//! Every subprocess has a deadline so failures cannot hang the test runner.
use rode::{files, persistence, sandbox};
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

fn finish(mut child: Child) -> Output {
    let until = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("A special-file operation blocked instead of rejecting its input");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn request(mut command: Command, payload: &impl serde::Serialize) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(payload).unwrap())
        .unwrap();
    finish(child)
}

#[test]
fn native_and_python_reads_and_saves_reject_fifos_without_a_writer() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("pipe.txt");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let alias = root.path().join("alias.txt");
    symlink(&fifo, &alias).unwrap();
    for path in [fifo, alias] {
        for python in [false, true] {
            for op in [
                "read",
                "read_auto",
                "read_hex",
                "read_image",
                "read_binary",
                "write",
                "write_binary",
            ] {
                let mut command = if python {
                    let mut command = Command::new("python3");
                    command.args(["-c", files::REMOTE_HELPER]);
                    command
                } else {
                    let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
                    command.arg("--file-helper");
                    command
                };
                command.current_dir(root.path());
                let output = request(
                    command,
                    &json!({"op":op,"path":path,"text":"replacement","data":"AA==","expected":null}),
                );
                assert!(!output.status.success(), "{op} accepted a FIFO");
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("regular"),
                    "{op}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}

#[test]
#[ignore = "private subprocess entry point for bounded configuration checks"]
fn configuration_special_file_child() {
    let path = std::env::var_os("RODE_SECURITY_FIFO").unwrap();
    let path = Path::new(&path);
    let result = match std::env::var("RODE_SECURITY_OPERATION").unwrap().as_str() {
        "read" => persistence::read(path, persistence::LIMIT).map(|_| ()),
        "transaction" => persistence::transaction(path, |_| {
            panic!("A special file must not enter a configuration merge")
        }),
        _ => unreachable!(),
    };
    assert!(format!("{:#}", result.unwrap_err()).contains("regular"));
}

#[test]
fn preference_reads_and_transactions_reject_fifos_without_hanging() {
    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("app.ron");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    for operation in ["read", "transaction"] {
        let output = finish(
            Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "configuration_special_file_child"])
                .env("RODE_SECURITY_FIFO", &fifo)
                .env("RODE_SECURITY_OPERATION", operation)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        assert!(
            output.status.success(),
            "{operation}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn symlinked_home_cannot_be_exposed_by_project_or_additional_grants() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    let home = private.join("actual-home");
    let alias = root.path().join("login-home");
    let project = root.path().join("project");
    fs::create_dir_all(home.join("allowed-project")).unwrap();
    fs::create_dir(&project).unwrap();
    symlink(&home, &alias).unwrap();
    let peer = format!(
        "__name__='security_peer'\n{}\nr=json.load(sys.stdin);print(json.dumps(sandbox_arguments('/usr/bin/bwrap',r['root'],r['home'],r['options'])))",
        files::REMOTE_HELPER
    );
    let mut cases = vec![];
    for exposed in [&home, &private] {
        cases.push((exposed.clone(), sandbox::Options::default(), false));
        for writable in [false, true] {
            let mut options = sandbox::Options::default();
            if writable {
                options.read_write.push(exposed.to_string_lossy().into());
            } else {
                options.read_only.push(exposed.to_string_lossy().into());
            }
            cases.push((project.clone(), options, false));
        }
    }
    // Ordinary subprojects still work, retaining the user's HOME spelling.
    cases.push((
        home.join("allowed-project"),
        sandbox::Options::default(),
        true,
    ));
    for (project, mut options, allowed) in cases {
        options.enabled = true;
        let result = sandbox::arguments(Path::new("/usr/bin/bwrap"), &project, &alias, &options);
        assert_eq!(result.is_ok(), allowed, "{project:?}: {result:?}");
        if !allowed {
            assert!(result.unwrap_err().to_string().contains("home directory"));
        }
        let mut command = Command::new("python3");
        command.args(["-c", &peer]);
        let output = request(
            command,
            &json!({"root":project,"home":alias,"options":options}),
        );
        assert_eq!(output.status.success(), allowed);
        if !allowed {
            assert!(String::from_utf8_lossy(&output.stderr).contains("home directory"));
        }
    }
}

#[test]
#[ignore = "private subprocess entry point launched with a real controlling PTY"]
fn background_terminal_child() {
    use std::sync::Arc;
    // Ensure the regression starts from a terminal, rather than trivially
    // passing because the test runner itself already had no controlling tty.
    assert!(fs::File::open("/dev/tty").is_ok());
    let root = tempfile::tempdir().unwrap();
    let code = "import json,os\ntry:\n fd=os.open('/dev/tty',os.O_RDWR);os.close(fd);tty=True\nexcept OSError:\n tty=False\nprint(json.dumps({'tty':tty}))";
    for enabled in [false, true] {
        let plan = sandbox::prepare(
            root.path().to_str().unwrap(),
            &sandbox::Options {
                enabled,
                ..Default::default()
            },
        )
        .unwrap();
        if enabled && !plan.active {
            eprintln!("Namespace check unavailable: {}", plan.reason);
            continue;
        }
        let mut command = if plan.active {
            let mut command = Command::new(&plan.prefix[0]);
            command.args(&plan.prefix[1..]).arg("python3");
            command
        } else {
            Command::new("python3")
        };
        command.args(["-c", code]);
        let output = rode::project::run(command, vec![], Duration::from_secs(5))
            .unwrap()
            .checked()
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["tty"], false, "Background tools inherited /dev/tty");

        let session = rode::process::Session::start(
            rode::process::Launch {
                argv: vec!["python3".into(), "-c".into(), code.into()],
                plan,
                remote: None,
                python: "off".into(),
                python_program: false,
                line_limit: 8192,
                native_debug: false,
            },
            Arc::new(|| {}),
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match session.poll() {
                Some(rode::process::Event::Message(value)) => {
                    assert_eq!(
                        value["tty"], false,
                        "Agent/kernel transport inherited /dev/tty"
                    );
                    break;
                }
                Some(rode::process::Event::Error(error)) => panic!("{error}"),
                Some(rode::process::Event::Exited(code)) => panic!("Exited before reply: {code:?}"),
                _ => {}
            }
            assert!(
                Instant::now() < until,
                "Background terminal check timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn background_workers_detach_from_a_real_launching_terminal() {
    use std::io::Read;
    let pair = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize::default())
        .unwrap();
    let mut command = portable_pty::CommandBuilder::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        "background_terminal_child",
        "--nocapture",
    ]);
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = vec![];
        let _ = reader.by_ref().take(64 * 1024).read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let until = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("PTY worker test timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(pair.master);
    let log = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(status.success(), "{}", String::from_utf8_lossy(&log));
}

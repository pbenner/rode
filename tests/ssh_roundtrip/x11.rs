//! Real authenticated X11 traffic through the gateway and native PTY service.
use rode::{
    files::Request,
    ssh::{Profile, Remote},
    terminal::Terminal,
};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct DisplayClient(Child);
impl Drop for DisplayClient {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Authenticated protocol traffic without a desktop, Xvfb or GPU. The private
/// display accepts only the original cookie, so forwarding must replace the
/// SSH proxy cookie rather than merely advertising DISPLAY.
struct ProtocolDisplay {
    port: u16,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl ProtocolDisplay {
    fn new() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port >= 6000);
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        socket
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut setup = [0; 48];
                        socket.read_exact(&mut setup).unwrap();
                        assert_eq!(setup[0], b'l');
                        assert_eq!(&setup[12..30], b"MIT-MAGIC-COOKIE-1");
                        assert_eq!(&setup[32..], &[0x11; 16]);
                        socket.write_all(b"RODE-X11-OK").unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("Private X11 display: {error}"),
                }
            }
        });
        Self {
            port,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for ProtocolDisplay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let result = self.worker.take().unwrap().join();
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}

#[test]
#[ignore = "requires only the isolated sshd from scripts/test-ssh.sh; no X server"]
fn x11_forwarding_protocol_reaches_native_sessions_without_a_desktop() {
    use std::os::unix::fs::PermissionsExt;
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    if std::env::var("RODE_TEST_SSH_X11").as_deref() != Ok("yes") {
        return;
    }
    for backend in [rode::ssh::Backend::Russh, rode::ssh::Backend::OpenSsh] {
        for jump in ["", "rode-jump"] {
            for (trusted, timeout) in [(true, 1200), (false, 1200), (false, 0)] {
                let root = tempfile::tempdir().unwrap();
                let display = ProtocolDisplay::new();
                let xauth = root.path().join("xauth");
                let xauth_log = root.path().join("xauth-calls");
                fs::write(&xauth, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nprintf 'fixture/unix:0 MIT-MAGIC-COOKIE-1 11111111111111111111111111111111\\n'\n", rode::ssh::quote(&xauth_log.to_string_lossy()))).unwrap();
                fs::set_permissions(&xauth, fs::Permissions::from_mode(0o700)).unwrap();
                let profile = root.path().join("ssh_config");
                // The profile's explicit trust choice must override opposite
                // SSH configuration with either client backend.
                fs::write(&profile, format!("Host rode-test\n ForwardX11Trusted {}\n ForwardX11Timeout {timeout}\n XAuthLocation {}\nHost *\n Include \"{config}\"\n", if trusted { "no" } else { "yes" }, xauth.display())).unwrap();
                let remote = Remote::new(Profile {
                    backend,
                    x11_trust: if trusted {
                        rode::ssh::X11Trust::Trusted
                    } else {
                        rode::ssh::X11Trust::Untrusted
                    },
                    host: "rode-test".into(),
                    config: profile.to_string_lossy().into(),
                    jump: jump.into(),
                    ..Default::default()
                })
                .unwrap();
                let log = root.path().join("login.log");
                let mut connection = Connection {
                    master: Command::new(remote.client_program())
                        .args(remote.master_args().unwrap())
                        .env("DISPLAY", format!("127.0.0.1:{}", display.port - 6000))
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(fs::File::create(&log).unwrap())
                        .spawn()
                        .unwrap(),
                    remote,
                    terminal: None,
                };
                let until = Instant::now() + Duration::from_secs(15);
                while !connection.remote.connected() {
                    assert!(
                        connection.master.try_wait().unwrap().is_none() && Instant::now() < until,
                        "{}",
                        fs::read_to_string(&log).unwrap()
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                fs::write(root.path().join("x-probe.py"), r#"
import os, socket, struct, subprocess
display = os.environ['DISPLAY']
records = subprocess.check_output(['xauth', 'list', display], text=True).splitlines()
cookie = next(bytes.fromhex(line.split()[2]) for line in records if 'MIT-MAGIC-COOKIE-1' in line)
number = int(display.rsplit(':', 1)[1].split('.')[0])
with socket.create_connection(('localhost', 6000 + number), timeout=5) as client:
    client.sendall(struct.pack('<BBHHHHH', ord('l'), 0, 11, 0, 18, len(cookie), 0) + b'MIT-MAGIC-COOKIE-1\0\0' + cookie)
    result = bytearray()
    while len(result) < 11:
        chunk = client.recv(11 - len(result))
        assert chunk, 'Forwarded X11 channel closed before the display replied'
        result.extend(chunk)
    assert result == b'RODE-X11-OK', result
print('authenticated X11 roundtrip passed')
"#).unwrap();
                let remote = &connection.remote;
                let plan = remote
                    .request(&Request::Prepare {
                        path: root.path().to_string_lossy().into(),
                        sandbox: Default::default(),
                    })
                    .unwrap()
                    .project
                    .unwrap();
                // OpenSSH prepares authorization when its first display
                // channel opens, after master authentication has completed.
                let calls = fs::read_to_string(&xauth_log).unwrap();
                assert_eq!(
                    calls
                        .lines()
                        .any(|line| line.split_whitespace().any(|arg| arg == "generate")),
                    !trusted,
                    "{backend:?}: trust override did not reach cookie generation"
                );
                if timeout == 0 {
                    // Zero disables expiry; it must not become a one-second timer.
                    std::thread::sleep(Duration::from_secs(2));
                }
                let response = remote
                    .request(&Request::Tool {
                        request: rode::project::Request::Sessions {
                            root: plan.root.clone(),
                            operation: rode::sessions::Operation::Create {
                                id: rode_session::new_id().unwrap(),
                                name: "X11 protocol".into(),
                                policy: rode::sessions::fingerprint(&plan),
                                plan: Box::new(plan.clone()),
                                python: "off".into(),
                                environment: Default::default(),
                                persistent: true,
                                allocation: None,
                            },
                            directory: Some(format!("{}/runtime", plan.root)),
                        },
                    })
                    .unwrap();
                let Some(rode::project::Response::Sessions(reply)) = response.tool else {
                    panic!()
                };
                let rode_session::Response::Session(info) = &reply.response else {
                    panic!()
                };
                connection.terminal = Some(info.clone());
                for marker in ["first", "reattached"] {
                    let terminal = Terminal::native_session(
                        &eframe::egui::Context::default(),
                        Some(remote),
                        &reply,
                        info,
                    )
                    .unwrap();
                    terminal.send(format!("/usr/bin/python3 -I x-probe.py > result-{marker}.tmp 2>&1; mv result-{marker}.tmp result-{marker}\n").into_bytes()).unwrap();
                    let result = root.path().join(format!("result-{marker}"));
                    let until = Instant::now() + Duration::from_secs(15);
                    while !result.exists() {
                        assert!(Instant::now() < until, "{}", terminal.contents());
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let result = fs::read_to_string(result).unwrap();
                    assert!(
                        result.contains("authenticated X11 roundtrip passed"),
                        "Backend={backend:?}, jump={jump:?}, trusted={trusted}, timeout={timeout}: {result}\n{}",
                        fs::read_to_string(&log).unwrap()
                    );
                }
            }
        }
    }
}

/// Keep a real X client connected while OpenSSH creates untrusted cookies.
/// Otherwise an idle Xvfb resets when xauth exits, discarding the newly created
/// cookie before the forwarded application can use it.
fn keep_display_open() -> DisplayClient {
    let title = format!("rode-x11-fixture-{}", std::process::id());
    let mut client = DisplayClient(
        Command::new("xmessage")
            .args(["-title", &title, "X11 forwarding test"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("Install x11-utils for X11 integration coverage"),
    );
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let output = Command::new("xwininfo")
            .args(["-root", "-tree"])
            .output()
            .unwrap();
        if output.status.success() && String::from_utf8_lossy(&output.stdout).contains(&title) {
            return client;
        }
        assert!(
            client.0.try_wait().unwrap().is_none() && Instant::now() < until,
            "Local X11 fixture did not create its window"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct Connection {
    remote: Remote,
    master: Child,
    terminal: Option<rode_session::Info>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(info) = &self.terminal {
            let _ = self.remote.request(&Request::Tool {
                request: rode::project::Request::Sessions {
                    root: info.root.clone(),
                    operation: rode::sessions::Operation::End {
                        id: info.id.clone(),
                        policy: info.policy.clone(),
                    },
                    directory: Some(format!("{}/runtime", info.root)),
                },
            });
        }
        self.remote.cleanup();
        let _ = self.master.kill();
        let _ = self.master.wait();
    }
}

/// Run an actual X client inside the shell and publish completion after all
/// output is closed. Checking only DISPLAY would accept a dead/stale tunnel.
fn probe(terminal: &Terminal, root: &Path, marker: &str, expected: bool) {
    terminal.send(format!(
        "printf '%s' \"${{DISPLAY:-}}\" > display-{marker}; if [ -n \"${{DISPLAY:-}}\" ]; then xdpyinfo > x11-{marker} 2>&1; result=$?; else result=99; fi; printf '%s' \"$result\" > done-{marker}.tmp; mv done-{marker}.tmp done-{marker}\n"
    ).into_bytes()).unwrap();
    let status = root.join(format!("done-{marker}"));
    let until = Instant::now() + Duration::from_secs(15);
    while !status.exists() {
        assert!(
            Instant::now() < until,
            "X11 probe timed out: {}",
            terminal.contents()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let display = fs::read_to_string(root.join(format!("display-{marker}"))).unwrap();
    if expected {
        let output = fs::read_to_string(root.join(format!("x11-{marker}"))).unwrap();
        assert_eq!(
            fs::read_to_string(status).unwrap(),
            "0",
            "DISPLAY={display:?}: {output}\n{}",
            terminal.contents()
        );
        assert!(
            display.starts_with("localhost:"),
            "Not an SSH display: {display:?}"
        );
        assert!(output.contains("vendor string:"), "{output}");
    } else {
        assert!(display.is_empty(), "Unexpected display access: {display:?}");
        assert_eq!(fs::read_to_string(status).unwrap(), "99");
    }
}

#[test]
#[ignore = "requires Xvfb/X11 utilities and the isolated sshd from scripts/test-ssh.sh"]
fn x11_forwarding_reaches_native_terminals_and_survives_view_reattachment() {
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    if std::env::var("DISPLAY").is_ok_and(|display| !display.is_empty()) {
        assert!(
            Command::new("xdpyinfo").output().unwrap().status.success(),
            "Local X server is unavailable"
        );
    } else {
        assert!(
            std::env::var("RODE_TEST_REQUIRE_X11").as_deref() != Ok("1"),
            "Required X11 coverage needs DISPLAY; run through xvfb-run"
        );
        eprintln!("Set DISPLAY (e.g. use xvfb-run) for real X11 forwarding coverage");
        return;
    }
    let _display = keep_display_open();
    let server_x11 = std::env::var("RODE_TEST_SSH_X11").unwrap() == "yes";
    for jump in ["", "rode-jump"] {
        for (enabled, display_available, xauth_available) in [
            (true, true, true),
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            let root = tempfile::tempdir().unwrap();
            let profile = root.path().join("ssh_config");
            // A disabled profile must override even an explicit SSH-config opt-in.
            // Exercise untrusted forwarding, without relying on system defaults.
            let xauth = if xauth_available {
                String::new()
            } else {
                format!(
                    "    XAuthLocation {}\n",
                    root.path().join("missing-xauth").display()
                )
            };
            fs::write(&profile, format!("Host rode-test\n    ForwardX11 yes\n    ForwardX11Trusted no\n{xauth}Host *\n    Include \"{config}\"\n")).unwrap();
            let remote = Remote::new(Profile {
                host: "rode-test".into(),
                config: profile.to_string_lossy().into(),
                jump: jump.into(),
                x11_forwarding: enabled,
                ..Default::default()
            })
            .unwrap();
            let log = root.path().join("login.log");
            let mut command = Command::new(remote.client_program());
            command
                .args(remote.master_args().unwrap())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(fs::File::create(&log).unwrap());
            if !display_available {
                command.env_remove("DISPLAY");
            }
            let mut connection = Connection {
                remote,
                master: command.spawn().unwrap(),
                terminal: None,
            };
            let remote = &connection.remote;
            let until = Instant::now() + Duration::from_secs(15);
            while !remote.connected() {
                assert!(
                    connection.master.try_wait().unwrap().is_none() && Instant::now() < until,
                    "{}",
                    fs::read_to_string(&log).unwrap()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            // Probes and other short-lived work must not capture an X listener.
            let output = Command::new(remote.client_program())
                .args(remote.channel_args(false))
                .arg("printf '%s' \"${DISPLAY:-}\"")
                .output()
                .unwrap();
            assert!(
                output.status.success() && output.stdout.is_empty(),
                "{output:?}"
            );
            let plan = remote
                .request(&Request::Prepare {
                    path: root.path().to_string_lossy().into(),
                    sandbox: Default::default(),
                })
                .unwrap()
                .project
                .unwrap();
            let response = remote
                .request(&Request::Tool {
                    request: rode::project::Request::Sessions {
                        root: plan.root.clone(),
                        operation: rode::sessions::Operation::Create {
                            id: rode_session::new_id().unwrap(),
                            name: "X11 test".into(),
                            plan: Box::new(plan.clone()),
                            policy: rode::sessions::fingerprint(&plan),
                            python: "off".into(),
                            environment: Default::default(),
                            persistent: true,
                            allocation: None,
                        },
                        directory: Some(format!("{}/runtime", plan.root)),
                    },
                })
                .unwrap();
            let Some(rode::project::Response::Sessions(reply)) = response.tool else {
                panic!("Missing session reply")
            };
            let rode_session::Response::Session(info) = &reply.response else {
                panic!("Missing native terminal")
            };
            connection.terminal = Some(info.clone());
            let expected = enabled && display_available && xauth_available && server_x11;
            let ctx = eframe::egui::Context::default();
            let terminal = Terminal::native_session(&ctx, Some(remote), &reply, info).unwrap();
            probe(&terminal, root.path(), "first", expected);
            drop(terminal);
            let terminal = Terminal::native_session(&ctx, Some(remote), &reply, info).unwrap();
            probe(&terminal, root.path(), "reattached", expected);
            drop(terminal);
        }
    }
}

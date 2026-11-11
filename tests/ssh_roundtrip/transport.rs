//! Native and OpenSSH connection lifecycle and full transport roundtrips.
use super::*;

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn openssh_mux_roundtrip_and_disconnect() {
    roundtrip("");
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn idle_remote_gateways_processes_and_terminals_renew_their_leases() {
    use std::sync::Arc;
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    std::thread::scope(|scope| {
        for backend in [rode::ssh::Backend::Russh, rode::ssh::Backend::OpenSsh] {
            for jump in ["", "rode-jump"] {
                let config = config.clone();
                scope.spawn(move || {
                    let base = tempfile::tempdir().unwrap();
                    let root = base.path().join("project");
                    std::fs::create_dir(&root).unwrap();
                    std::fs::write(root.join("source"), "idle gateway survived").unwrap();
                    let remote = Remote::new(Profile {
                        host: "rode-test".into(), backend, config, jump: jump.into(),
                        x11_forwarding: false, ..Default::default()
                    }).unwrap();
                    let ctx = eframe::egui::Context::default();
                    let login = Terminal::remote(&ctx, &remote).unwrap();
                    let until = Instant::now() + Duration::from_secs(15);
                    while !remote.connected() {
                        assert!(login.is_alive() && Instant::now() < until, "{}", login.contents());
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let plan = remote.request(&Request::Prepare {
                        path: root.to_string_lossy().into(), sandbox: Default::default()
                    }).unwrap().project.unwrap();
                    let response = remote.request(&Request::Tool {
                        request: rode::project::Request::Sessions {
                            root: plan.root.clone(),
                            operation: rode::sessions::Operation::Create {
                                id: rode_session::new_id().unwrap(), name: "Idle lease test".into(),
                                policy: rode::sessions::fingerprint(&plan),
                                environment: Box::new(plan.environment.clone()), plan: Box::new(plan.clone()),
                                python: "off".into(), persistent: false, allocation: None,
                            },
                            directory: Some(base.path().join("sessions").to_string_lossy().into()),
                        }
                    }).unwrap();
                    let Some(rode::project::Response::Sessions(reply)) = response.tool else { panic!() };
                    let rode_session::Response::Session(info) = &reply.response else { panic!() };
                    let terminal = Terminal::native_session(&ctx, Some(&remote), &reply, info).unwrap();
                    let pipe = rode::process::Session::start(rode::process::Launch {
                            argv: policy_python("import json, sys; print('{\"ready\":true}', flush=True)\nfor line in sys.stdin: print(json.dumps(json.loads(line)), flush=True)").to_vec(),
                        plan: plan.clone(), remote: Some(remote.clone()), python: "off".into(),
                        python_program: false, native_debug: false, line_limit: 8192,
                    }, Arc::new(|| {})).unwrap();
                    let until = Instant::now() + Duration::from_secs(15);
                    loop {
                        if let Some(rode::process::Event::Message(value)) = pipe.poll() {
                            assert_eq!(value["ready"], true);
                            break;
                        }
                        assert!(Instant::now() < until, "Remote process did not start");
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    // No GUI repaint or user input renews these leases.
                    std::thread::sleep(rode_host_protocol::framing::CONNECTION_TIMEOUT + Duration::from_secs(5));
                    assert!(terminal.is_alive(), "{}", terminal.contents());
                    remote.request_in(&Request::Read { path: root.join("source").to_string_lossy().into() }, Some(&plan)).unwrap();
                    pipe.send(&serde_json::json!({"after_idle":true})).unwrap();
                    let until = Instant::now() + Duration::from_secs(10);
                    loop {
                        match pipe.poll() {
                            Some(rode::process::Event::Message(value)) => { assert_eq!(value["after_idle"], true); break; }
                            Some(rode::process::Event::Error(error)) => panic!("{error}"),
                            Some(rode::process::Event::Exited(code)) => panic!("Idle process exited: {code:?}"),
                            _ => {}
                        }
                        assert!(Instant::now() < until);
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    terminal.send(b"printf 'IDLE_%s\\n' READY\n".to_vec()).unwrap();
                    while !terminal.contents().contains("IDLE_READY") {
                        assert!(terminal.is_alive() && Instant::now() < until, "{}", terminal.contents());
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    drop(pipe);
                    drop(terminal);
                    drop(login);
                    remote.cleanup();
                });
            }
        }
    });
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn openssh_proxyjump_roundtrip_and_disconnect() {
    roundtrip("rode-jump");
}

pub(super) fn roundtrip(jump: &str) {
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    let root = tempfile::tempdir().unwrap();
    let path = root
        .path()
        .join("remote 'quoted' $file 🦀.rs")
        .to_str()
        .unwrap()
        .to_string();
    let remote = Remote::new(Profile {
        host: "rode-test".into(),
        config,
        jump: jump.into(),
        root: root.path().to_str().unwrap().into(),
        ..Default::default()
    })
    .unwrap();
    let terminal = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
    let start = Instant::now();
    while !remote.connected() && terminal.is_alive() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(remote.connected(), "{}", terminal.contents());
    // Separate RODE instances use separate masters even for the same SSH alias.
    let other = Remote::new(remote.profile.clone()).unwrap();
    assert_ne!(remote.master_args().unwrap(), other.master_args().unwrap());
    let other_terminal = Terminal::remote(&eframe::egui::Context::default(), &other).unwrap();
    let start = Instant::now();
    while !other.connected()
        && other_terminal.is_alive()
        && start.elapsed() < Duration::from_secs(10)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(other.connected(), "{}", other_terminal.contents());
    drop(other_terminal);
    other.cleanup();
    assert!(
        remote.connected(),
        "Closing another instance's SSH master disconnected this one"
    );
    let normal = remote
        .request(&Request::Prepare {
            path: root.path().to_string_lossy().into_owned(),
            sandbox: Default::default(),
        })
        .unwrap()
        .project
        .unwrap();
    let request = |request: &Request| remote.request_in(request, Some(&normal));
    let write = request(&Request::Write {
        path: path.clone(),
        text: "fn main() { println!(\"hello remote host 🦀\"); }\n".into(),
        expected: None,
    })
    .unwrap();
    let read = request(&Request::Read { path: path.clone() }).unwrap();
    assert_eq!(write.revision, read.revision);
    assert!(read.text.contains("hello remote host 🦀"));
    // A multi-megabyte save exercises simultaneous pipe draining; it must not deadlock.
    let large = "αβγ Rust remote host\n".repeat(100_000);
    request(&Request::Write {
        path: path.clone(),
        text: large.clone(),
        expected: Some(read.revision),
    })
    .unwrap();
    assert_eq!(
        request(&Request::Read { path: path.clone() }).unwrap().text,
        large
    );
    assert!(
        request(&Request::Write {
            path: path.clone(),
            text: "stale".into(),
            expected: Some(write.revision)
        })
        .unwrap_err()
        .to_string()
        .contains("conflict")
    );
    let list = request(&Request::List {
        path: root.path().to_str().unwrap().into(),
    })
    .unwrap();
    assert!(list.entries.iter().any(|e| e.path == path));
    // Open two project folders through the same authenticated connection. A
    // non-root test sshd uses non-TTY channels; the local PTY remains real.
    let second = root.path().join("project 'two' 🦀");
    std::fs::create_dir(&second).unwrap();
    for (index, directory) in [root.path(), second.as_path()].iter().enumerate() {
        let response = remote
            .request(&Request::Prepare {
                path: directory.to_str().unwrap().into(),
                sandbox: Default::default(),
            })
            .unwrap();
        assert_eq!(response.path, directory.to_str().unwrap());
        let plan = response.project.unwrap();
        // Exercise the real project-shell command without account login files
        // reading terminal input or changing the working directory.
        let fixture_shell = directory.join("fixture-shell");
        std::fs::write(
            &fixture_shell,
            "#!/bin/sh\nunset ENV BASH_ENV\nexec /bin/sh -i\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fixture_shell, std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let mut command = portable_pty::CommandBuilder::new(remote.client_program());
        command.args(remote.channel_args(false));
        command.arg(format!(
            "SHELL={} {}",
            rode::ssh::quote(&fixture_shell.to_string_lossy()),
            remote.shell_command(&plan)
        ));
        let shell = Terminal::spawn(
            &eframe::egui::Context::default(),
            command,
            "project shell".into(),
        )
        .unwrap();
        let marker = directory.join("terminal-marker.txt");
        shell
            .send(format!("printf 'project-{index}' > terminal-marker.txt\n").into_bytes())
            .unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        let expected = format!("project-{index}");
        loop {
            let actual = std::fs::read_to_string(&marker);
            if actual.as_deref().is_ok_and(|value| value == expected) {
                break;
            }
            assert!(
                shell.is_alive() && Instant::now() < until,
                "Project shell did not write its complete marker: {actual:?}\n{}",
                shell.contents()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(shell);
        assert!(
            remote.connected(),
            "Closing a project shell must not disconnect SSH"
        );
    }
    let response = remote
        .request(&Request::Prepare {
            path: second.to_str().unwrap().into(),
            sandbox: rode::sandbox::Options {
                enabled: true,
                ..Default::default()
            },
        })
        .unwrap();
    let plan = response.project.unwrap();
    let project_root = second.to_str().unwrap().to_owned();
    let image_path = second.join("image 'quoted' 🦀.png");
    let image_bytes = include_bytes!("../../assets/rode-tour.png");
    std::fs::write(&image_path, image_bytes).unwrap();
    let response = remote
        .request_in(
            &Request::ReadImage {
                path: image_path.to_string_lossy().into(),
            },
            Some(&plan),
        )
        .unwrap();
    let decoded = rode::images::decode(
        response.image.as_deref().unwrap(),
        image_path.to_str().unwrap(),
        &std::sync::Arc::new(rode::images::Budget::default()),
        4096,
    )
    .unwrap();
    assert_eq!(decoded.format, "PNG");
    assert_eq!(response.revision, rode::files::digest(image_bytes));
    if plan.active {
        assert!(
            remote
                .request_in(&Request::ReadImage { path: path.clone() }, Some(&plan))
                .is_err(),
            "Image transfers must obey the same sandbox grants as text reads"
        );
    }
    let git = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::Git {
                    root: project_root.clone(),
                    args: vec!["init".into()],
                    input: None,
                },
            },
            Some(&plan),
        )
        .unwrap()
        .tool
        .unwrap();
    assert!(matches!(git,rode::project::Response::Command(output) if output.success()));
    let run_git = |args: &[&str]| {
        let response = remote
            .request_in(
                &Request::Tool {
                    request: rode::project::Request::Git {
                        root: project_root.clone(),
                        args: args.iter().map(|s| (*s).into()).collect(),
                        input: None,
                    },
                },
                Some(&plan),
            )
            .unwrap()
            .tool
            .unwrap();
        let rode::project::Response::Command(output) = response else {
            panic!("Missing Git reply")
        };
        output.checked().unwrap()
    };
    let stash_file = second.join("stash-test.txt");
    std::fs::write(&stash_file, "baseline\n").unwrap();
    run_git(&["add", "--", "stash-test.txt"]);
    run_git(&[
        "-c",
        "user.name=RODE Test",
        "-c",
        "user.email=rode@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-m",
        "Stash baseline",
    ]);
    let stash = |value: &str| {
        std::fs::write(&stash_file, value).unwrap();
        run_git(&[
            "-c",
            "user.name=RODE Test",
            "-c",
            "user.email=rode@example.invalid",
            "stash",
            "push",
            "-m",
            value.trim(),
            "--",
            "stash-test.txt",
        ]);
        run_git(&["rev-parse", "refs/stash"]).trim().to_owned()
    };
    let selected = stash("selected A\n");
    let newer = stash("newer B\n");
    let drop_stash = |reference: &str| {
        remote.request_in(
            &Request::Tool {
                request: rode::project::Request::GitStash {
                    root: project_root.clone(),
                    action: rode::project::StashAction::Drop,
                    reference: reference.into(),
                    oid: selected.clone(),
                },
            },
            Some(&plan),
        )
    };
    assert!(
        drop_stash("stash@{0}")
            .unwrap_err()
            .to_string()
            .contains("Stash list changed")
    );
    let reply = drop_stash("stash@{1}").unwrap().tool.unwrap();
    assert!(matches!(reply, rode::project::Response::Command(output) if output.success()));
    assert_eq!(run_git(&["stash", "list", "--format=%H"]).trim(), newer);
    let environment = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::Environments {
                    root: project_root.clone(),
                    selected: String::new(),
                },
            },
            Some(&plan),
        )
        .unwrap()
        .tool
        .unwrap();
    assert!(
        matches!(environment,rode::project::Response::Environments(env) if !env.version.is_empty())
    );
    let monitor = remote
        .request(&Request::Tool {
            request: rode::project::Request::Monitor {
                root: project_root.clone(),
            },
        })
        .unwrap()
        .tool
        .unwrap();
    assert!(matches!(monitor,rode::project::Response::Monitor(sample) if !sample.cpus.is_empty()));
    if let Ok(ruff) = std::env::var("RODE_TEST_RUFF") {
        let copied = second.join("ruff-test");
        std::fs::copy(ruff, &copied).unwrap();
        let analysis = remote
            .request_in(
                &Request::Tool {
                    request: rode::project::Request::Analyze {
                        root: project_root.clone(),
                        path: second.join("main.py").to_str().unwrap().into(),
                        text: "import os\nvalue= [1,2]\n".into(),
                        python: String::new(),
                        ruff: copied.to_str().unwrap().into(),
                        format: true,
                        check: true,
                        fix: false,
                    },
                },
                Some(&plan),
            )
            .unwrap()
            .tool
            .unwrap();
        assert!(
            matches!(analysis,rode::project::Response::Analysis(result) if result.text.contains("value = [1, 2]") && result.diagnostics.iter().any(|d|d.code == "F401"))
        );
    }
    // New language tool routing uses the same authenticated host/sandbox.
    let tools = second.join("node_modules/.bin");
    std::fs::create_dir_all(&tools).unwrap();
    let formatter = tools.join("prettier");
    std::fs::write(&formatter, "#!/usr/bin/python3\nimport sys\nsource=sys.stdin.read()\nassert '--stdin-filepath' in sys.argv\nif 'broken' in source:\n print('[error] main.ts: SyntaxError (2:3)',file=sys.stderr);sys.exit(2)\nprint(source.replace('x=1','x = 1'),end='')\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&formatter, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for source in ["const x=1;\n", "// comment\nbroken\n"] {
        let response = remote
            .request_in(
                &Request::Tool {
                    request: rode::project::Request::Analyze {
                        root: project_root.clone(),
                        path: second.join("main.ts").to_string_lossy().into(),
                        text: source.into(),
                        python: String::new(),
                        ruff: String::new(),
                        format: true,
                        check: true,
                        fix: false,
                    },
                },
                Some(&plan),
            )
            .unwrap();
        let Some(rode::project::Response::Analysis(result)) = response.tool else {
            panic!()
        };
        if source.contains("broken") {
            assert_eq!(result.text, source);
            assert_eq!(result.diagnostics[0].row, 2);
        } else {
            assert_eq!(result.text, "const x = 1;\n");
        }
        assert!(!second.join("main.ts").exists());
    }
    let pdf_bytes = include_bytes!("../fixtures/pages.pdf");
    let pdf_path = second.join("quoted ' page 🦀.PDF");
    std::fs::write(&pdf_path, pdf_bytes).unwrap();
    let private_pdf = root.path().join("outside.pdf");
    std::fs::write(&private_pdf, pdf_bytes).unwrap();
    let escape_pdf = second.join("escape.pdf");
    std::os::unix::fs::symlink(&private_pdf, &escape_pdf).unwrap();
    if plan.active {
        for path in [&private_pdf, &escape_pdf] {
            assert!(
                remote
                    .request_in(
                        &Request::ReadBinary {
                            path: path.to_string_lossy().into()
                        },
                        Some(&plan)
                    )
                    .is_err()
            );
        }
    }
    let binary = remote
        .request_in(
            &Request::ReadBinary {
                path: pdf_path.to_string_lossy().into(),
            },
            Some(&plan),
        )
        .unwrap();
    {
        use base64::{Engine, engine::general_purpose::STANDARD};
        assert_eq!(STANDARD.decode(binary.binary.unwrap()).unwrap(), pdf_bytes);
    }
    let inside = second.join("sandbox.txt").to_str().unwrap().to_owned();
    remote
        .request_in(
            &Request::Write {
                path: inside.clone(),
                text: "remote sandbox".into(),
                expected: None,
            },
            Some(&plan),
        )
        .unwrap();
    assert_eq!(
        remote
            .request_in(&Request::Read { path: inside }, Some(&plan))
            .unwrap()
            .text,
        "remote sandbox"
    );
    if plan.active {
        assert!(
            remote
                .request_in(&Request::Read { path: path.clone() }, Some(&plan))
                .is_err(),
            "Parent project files must not be exposed to the sandbox of its child project"
        );
    } else {
        assert!(plan.requested && !plan.reason.is_empty());
        eprintln!("Remote sandbox unavailable: {}", plan.reason);
    }
    debugger_roundtrip(&remote, &plan);
    native_debugger_roundtrip(&remote, &plan.root);
    bundled_runtime_roundtrip(&remote, root.path());
    notebook_roundtrip(&remote, &plan.root, &path);
    marp_roundtrip(&remote, &plan);
    latex_roundtrip(&remote, &plan, &path);
    encrypted_roundtrip(&remote, root.path());
    binary_extension_roundtrip(&remote, &plan, &path);
    agent_roundtrip(&remote, &plan);
    interactive_allocation_roundtrip(&remote, &plan);
    let (native_info, native_reply, native_runtime) = native_terminal_start(&remote, &plan);
    drop(terminal);
    let start = Instant::now();
    while remote.connected() && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        remote.request(&Request::Read { path }).is_err(),
        "File operations must fail after disconnect instead of opening another SSH login"
    );
    remote.cleanup();
    // Reconnect with a new editor-owned SSH master: the native PTY owner must
    // survive destruction of the first connection and all of its channels.
    let next = Remote::new(remote.profile.clone()).unwrap();
    let login = Terminal::remote(&eframe::egui::Context::default(), &next).unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    while !next.connected() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(next.connected());
    let native = native_attach(&next, &native_reply, &native_info);
    let until = Instant::now() + Duration::from_secs(8);
    while !native.contents().contains("BACKGROUND_DONE") && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        native.contents().contains("BACKGROUND_DONE"),
        "{}",
        native.contents()
    );
    let changed = next
        .request(&Request::Tool {
            request: rode::project::Request::Sessions {
                root: plan.root.clone(),
                operation: rode::sessions::Operation::SetPersistence {
                    id: native_info.id.clone(),
                    policy: native_info.policy.clone(),
                    persistent: false,
                },
                directory: Some(native_runtime.clone()),
            },
        })
        .unwrap();
    assert!(matches!(
        changed.tool,
        Some(rode::project::Response::Sessions(_))
    ));
    let until = Instant::now() + Duration::from_secs(5);
    while native.persistent() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !native.persistent(),
        "SSH persistence update did not reach the attached view"
    );
    drop(native);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let response = next
            .request(&Request::Tool {
                request: rode::project::Request::Sessions {
                    root: plan.root.clone(),
                    operation: rode::sessions::Operation::List,
                    directory: Some(native_runtime.clone()),
                },
            })
            .unwrap();
        if let Some(rode::project::Response::Sessions(reply)) = response.tool
            && let rode_session::Response::Sessions(infos) = reply.response
            && !infos.iter().any(|info| info.id == native_info.id)
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "SSH nonpersistent terminal survived its last view"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(login);
    next.cleanup();
    let until = Instant::now() + Duration::from_secs(3);
    while std::path::Path::new(&native_runtime)
        .join("socket")
        .exists()
        && Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn native_generic_operations_need_no_remote_python() {
    if std::env::var("RODE_SSH_TEST_NO_PYTHON").as_deref() != Ok("1") {
        eprintln!("Set RODE_SSH_TEST_NO_PYTHON=1 for the native host without Python fixture");
        return;
    }
    let config = std::env::var("RODE_TEST_SSH_CONFIG").unwrap();
    for jump in ["", "rode-jump"] {
        let remote = Remote::new(Profile {
            host: "rode-test".into(),
            config: config.clone(),
            jump: jump.into(),
            ..Default::default()
        })
        .unwrap();
        let login = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while !remote.connected() {
            assert!(
                login.is_alive() && Instant::now() < until,
                "{}",
                login.contents()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            Command::new(remote.client_program())
                .args(remote.channel_args(false))
                .arg("if command -v python3 >/dev/null 2>&1; then exit 99; fi")
                .status()
                .unwrap()
                .success(),
            "Python must be absent from the fixture PATH"
        );
        let base = tempfile::tempdir().unwrap();
        if std::env::var("RODE_SSH_TEST_NO_BWRAP").as_deref() == Ok("1") {
            assert!(
                Command::new(remote.client_program())
                    .args(remote.channel_args(false))
                    .arg("if command -v bwrap >/dev/null 2>&1; then exit 99; fi")
                    .status()
                    .unwrap()
                    .success(),
                "bwrap must be absent from the remote fixture PATH"
            );
            let root = base.path().join("bundled-bwrap");
            std::fs::create_dir(&root).unwrap();
            let plan = remote
                .request(&Request::Prepare {
                    path: root.to_string_lossy().into(),
                    sandbox: rode::sandbox::Options {
                        enabled: true,
                        ..Default::default()
                    },
                })
                .unwrap()
                .project
                .unwrap();
            assert!(
                plan.reason.contains("bundled bubblewrap"),
                "{}",
                plan.reason
            );
            if std::env::var("RODE_TEST_REQUIRE_SANDBOX").as_deref() == Ok("1") {
                assert!(
                    plan.active,
                    "Required remote bundled sandbox unavailable: {}",
                    plan.reason
                );
            }
            if plan.active {
                assert_eq!(plan.prefix[1], "--bwrap-launch");
                let path = root
                    .join("remote-bundled.txt")
                    .to_string_lossy()
                    .into_owned();
                let saved = remote
                    .request_in(
                        &Request::Write {
                            path: path.clone(),
                            text: "bundled over SSH".into(),
                            expected: None,
                        },
                        Some(&plan),
                    )
                    .unwrap();
                let read = remote
                    .request_in(&Request::Read { path }, Some(&plan))
                    .unwrap();
                assert_eq!(read.text, "bundled over SSH");
                assert_eq!(read.revision, saved.revision);
                let secret = base.path().join("private");
                std::fs::write(&secret, "private").unwrap();
                assert!(
                    remote
                        .request_in(
                            &Request::Read {
                                path: secret.to_string_lossy().into()
                            },
                            Some(&plan)
                        )
                        .is_err()
                );
            } else {
                assert!(plan.prefix.is_empty());
                eprintln!("Remote bundled bubblewrap unavailable: {}", plan.reason);
            }
        }
        for sandboxed in [false, true] {
            let root = base
                .path()
                .join(if sandboxed { "isolated" } else { "normal" });
            std::fs::create_dir(&root).unwrap();
            let plan = remote
                .request(&Request::Prepare {
                    path: root.to_string_lossy().into(),
                    sandbox: rode::sandbox::Options {
                        enabled: sandboxed,
                        backend: rode::sandbox::Backend::Landlock,
                        read_only: std::env::var("RODE_TEST_PROJECT_SHELL")
                            .into_iter()
                            .collect(),
                        ..Default::default()
                    },
                })
                .unwrap()
                .project
                .unwrap();
            if sandboxed && !plan.active {
                assert_ne!(
                    std::env::var("RODE_TEST_REQUIRE_LANDLOCK").as_deref(),
                    Ok("1"),
                    "{}",
                    plan.reason
                );
                eprintln!(
                    "Native Landlock unavailable on this kernel: {}",
                    plan.reason
                );
                continue;
            }
            let path = root.join("native ' 🦀.rs").to_string_lossy().into_owned();
            let saved = remote
                .request_in(
                    &Request::Write {
                        path: path.clone(),
                        text: "native file service".into(),
                        expected: None,
                    },
                    Some(&plan),
                )
                .unwrap();
            let read = remote
                .request_in(&Request::Read { path }, Some(&plan))
                .unwrap();
            assert_eq!(read.text, "native file service");
            assert_eq!(read.revision, saved.revision);
            let response = remote
                .request_in(
                    &Request::Tool {
                        request: rode::project::Request::Git {
                            root: root.to_string_lossy().into(),
                            args: vec!["init".into(), "--quiet".into()],
                            input: None,
                        },
                    },
                    Some(&plan),
                )
                .unwrap();
            assert!(
                matches!(response.tool, Some(rode::project::Response::Command(output)) if output.success())
            );
            native_provider_and_terminal_without_python(&remote, plan);
        }
        drop(login);
        remote.cleanup();
    }
}

pub(super) fn native_provider_and_terminal_without_python(
    remote: &Remote,
    mut plan: rode::sandbox::Plan,
) {
    use std::{fs, os::unix::fs::PermissionsExt, path::Path, sync::Arc};
    let root = Path::new(&plan.root);
    let mise = root.join("fixture-mise");
    fs::write(&mise, "#!/bin/sh\ncase \"$*\" in\n'ls --current --json') printf '%s\\n' '{}' ;;\n'env --json') printf '%s\\n' '{\"RODE_PROVIDER\":\"native\"}' ;;\n*) printf '%s\\n' 'export RODE_PROVIDER=native' ;;\nesac\n").unwrap();
    fs::set_permissions(&mise, fs::Permissions::from_mode(0o700)).unwrap();
    plan.environment.mise = true;
    plan.environment
        .runtimes
        .insert("mise".into(), mise.to_string_lossy().into());
    let process = rode::process::Session::start(
        rode::process::Launch {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf '{\"provider\":\"%s\"}\\n' \"$RODE_PROVIDER\"".into(),
            ],
            plan: plan.clone(),
            remote: Some(remote.clone()),
            python: "off".into(),
            python_program: false,
            native_debug: false,
            line_limit: 8192,
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        match process.poll() {
            Some(rode::process::Event::Message(value)) => {
                assert_eq!(value["provider"], "native");
                break;
            }
            Some(rode::process::Event::Error(error)) => panic!("{error}"),
            Some(rode::process::Event::Exited(code)) => {
                panic!("Provider exited before reply: {code:?}")
            }
            _ => {}
        }
        assert!(Instant::now() < until, "Native process did not reply");
        std::thread::sleep(Duration::from_millis(10));
    }
    let directory = root
        .parent()
        .unwrap()
        .join("sessions")
        .to_string_lossy()
        .into_owned();
    let request = |operation| {
        remote
            .request(&Request::Tool {
                request: rode::project::Request::Sessions {
                    root: plan.root.clone(),
                    operation,
                    directory: Some(directory.clone()),
                },
            })
            .unwrap()
    };
    let reply = request(rode::sessions::Operation::Create {
        id: rode_session::new_id().unwrap(),
        name: "Native provider".into(),
        plan: Box::new(plan.clone()),
        policy: rode::sessions::fingerprint(&plan),
        python: "off".into(),
        environment: Box::new(plan.environment.clone()),
        persistent: false,
        allocation: None,
    });
    let Some(rode::project::Response::Sessions(reply)) = reply.tool else {
        panic!()
    };
    let rode_session::Response::Session(info) = &reply.response else {
        panic!()
    };
    let view = native_attach(remote, &reply, info);
    view.send(b"printf 'PROVIDER_%s\\n' \"$RODE_PROVIDER\"\n".to_vec())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !view.contents().contains("PROVIDER_native") {
        assert!(Instant::now() < until, "{}", view.contents());
        std::thread::sleep(Duration::from_millis(20));
    }
    request(rode::sessions::Operation::End {
        id: info.id.clone(),
        policy: info.policy.clone(),
    });
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn rust_client_works_without_client_path_and_preserves_channel_status() {
    if std::env::var_os("RODE_NO_SSH_CLIENT_CHILD").is_none() {
        let empty_path = tempfile::tempdir().unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "rust_client_works_without_client_path_and_preserves_channel_status",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env("RODE_NO_SSH_CLIENT_CHILD", "1")
            .env("PATH", empty_path.path())
            .env_remove("DISPLAY")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    assert!(Command::new("ssh").arg("-V").output().is_err());
    let config = std::env::var("RODE_TEST_SSH_CONFIG").unwrap();
    for jump in ["", "rode-jump"] {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(Profile {
            host: "rode-test".into(),
            config: config.clone(),
            jump: jump.into(),
            x11_forwarding: false,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(remote.profile.backend, rode::ssh::Backend::Russh);
        let login = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
        let until = Instant::now() + Duration::from_secs(15);
        while !remote.connected() {
            assert!(
                login.is_alive() && Instant::now() < until,
                "{}",
                login.contents()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        let path = root
            .path()
            .join("rust 'client' 🦀.txt")
            .to_string_lossy()
            .into_owned();
        let plan = remote
            .request(&Request::Prepare {
                path: root.path().to_string_lossy().into_owned(),
                sandbox: Default::default(),
            })
            .unwrap()
            .project
            .unwrap();
        let saved = remote
            .request_in(
                &Request::Write {
                    path: path.clone(),
                    text: "no client ssh required\n".into(),
                    expected: None,
                },
                Some(&plan),
            )
            .unwrap();
        let read = remote
            .request_in(&Request::Read { path: path.clone() }, Some(&plan))
            .unwrap();
        assert_eq!(read.text, "no client ssh required\n");
        assert_eq!(read.revision, saved.revision);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    let output = remote
                        .command()
                        .arg("printf stdout; printf stderr >&2; exit 37")
                        .output()
                        .unwrap();
                    assert_eq!(output.status.code(), Some(37));
                    assert_eq!(output.stdout, b"stdout");
                    assert_eq!(output.stderr, b"stderr");
                });
            }
        });
        let mut echo = remote.command();
        echo.arg("cat");
        let bytes = vec![b'x'; 2 * 1024 * 1024];
        let output = rode::project::run(echo, bytes.clone(), Duration::from_secs(20)).unwrap();
        assert!(output.success(), "{output:?}");
        assert_eq!(output.stdout.as_bytes(), bytes);
        assert!(remote.connected());
        drop(login);
        let until = Instant::now() + Duration::from_secs(5);
        while remote.connected() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!remote.connected());
        assert!(
            !remote
                .command()
                .arg("true")
                .output()
                .unwrap()
                .status
                .success()
        );
        remote.cleanup();
    }
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn openssh_remains_an_explicit_compatibility_backend() {
    let remote = Remote::new(Profile {
        host: "rode-test".into(),
        config: std::env::var("RODE_TEST_SSH_CONFIG").unwrap(),
        backend: rode::ssh::Backend::OpenSsh,
        ..Default::default()
    })
    .unwrap();
    let login = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !remote.connected() {
        assert!(
            login.is_alive() && Instant::now() < until,
            "{}",
            login.contents()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        remote
            .command()
            .arg("printf compatibility")
            .output()
            .unwrap()
            .stdout,
        b"compatibility"
    );
    drop(login);
    remote.cleanup();
}

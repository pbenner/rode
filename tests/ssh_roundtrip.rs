//! Run through scripts/test-ssh.sh, which creates an isolated localhost SSH server.
use rode::{
    files::Request,
    ssh::{Profile, Remote},
    terminal::Terminal,
};
use std::{
    process::Command,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn openssh_mux_roundtrip_and_disconnect() {
    roundtrip("");
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn openssh_proxyjump_roundtrip_and_disconnect() {
    roundtrip("rode-jump");
}

fn roundtrip(jump: &str) {
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
    assert_ne!(remote.master_args(), other.master_args());
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
    let write = remote
        .request(&Request::Write {
            path: path.clone(),
            text: "fn main() { println!(\"hello remote host 🦀\"); }\n".into(),
            expected: None,
        })
        .unwrap();
    let read = remote
        .request(&Request::Read { path: path.clone() })
        .unwrap();
    assert_eq!(write.revision, read.revision);
    assert!(read.text.contains("hello remote host 🦀"));
    // A multi-megabyte save exercises simultaneous pipe draining; it must not deadlock.
    let large = "αβγ Rust remote host\n".repeat(100_000);
    remote
        .request(&Request::Write {
            path: path.clone(),
            text: large.clone(),
            expected: Some(read.revision),
        })
        .unwrap();
    assert_eq!(
        remote
            .request(&Request::Read { path: path.clone() })
            .unwrap()
            .text,
        large
    );
    assert!(
        remote
            .request(&Request::Write {
                path: path.clone(),
                text: "stale".into(),
                expected: Some(write.revision)
            })
            .unwrap_err()
            .to_string()
            .contains("conflict")
    );
    let list = remote
        .request(&Request::List {
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
        let mut command = portable_pty::CommandBuilder::new("ssh");
        command.args(remote.channel_args(false));
        command.arg(remote.shell_command(&plan));
        let shell = Terminal::spawn(
            &eframe::egui::Context::default(),
            command,
            "project shell".into(),
        )
        .unwrap();
        let marker = directory.join("terminal-marker.txt");
        shell
            .send(format!("printf 'project-{index}' > terminal-marker.txt\r").into_bytes())
            .unwrap();
        let start = Instant::now();
        while !marker.exists() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(marker).unwrap(),
            format!("project-{index}"),
            "{}",
            shell.contents()
        );
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
    let image_bytes = include_bytes!("../assets/carbon.png");
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
    let pdf_bytes = include_bytes!("fixtures/pages.pdf");
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
    notebook_roundtrip(&remote, &plan.root, &path);
    marp_roundtrip(&remote, &plan);
    latex_roundtrip(&remote, &plan, &path);
    encrypted_roundtrip(&remote, root.path());
    binary_extension_roundtrip(&remote, &plan, &path);
    agent_roundtrip(&remote, &plan);
    native_upgrade_roundtrip(&remote, &plan);
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
                helper: Some(native_test_helper(false)),
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
                    helper: Some(native_test_helper(false)),
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

fn native_test_helper(data: bool) -> rode::sessions::Helper {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/release/rode-sessiond-static");
    let bytes = std::fs::read(path).expect("Run scripts/build-sessiond.sh before the SSH test");
    rode::sessions::Helper {
        digest: rode::files::digest(&bytes),
        architecture: std::env::consts::ARCH.into(),
        data: data.then(|| STANDARD.encode(bytes)),
    }
}

fn marp_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    use rode::marp::{Destination, Format, Invocation, Settings};
    use std::{
        os::unix::fs::PermissionsExt,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let root = std::path::Path::new(&plan.root);
    let program = root.join("marp.py");
    std::fs::write(&program, include_bytes!("fixtures/marp.py")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.join("pages.pdf"), include_bytes!("fixtures/pages.pdf")).unwrap();
    let source = root.join("deck.marp.md");
    std::fs::write(&source, "saved source").unwrap();
    let mut request = Invocation {
        root: plan.root.clone(),
        path: source.to_string_lossy().into(),
        text: "---\nmarp: true\n---\n# SSH unsaved slide\n".into(),
        settings: Settings {
            program: program.to_string_lossy().into(),
            ..Default::default()
        },
        format: Format::Pdf,
        destination: None,
    };
    let reply = remote
        .marp_request(&request, Some(plan), &AtomicBool::new(false))
        .unwrap();
    let loaded = rode::pdf::load(
        reply.data.as_deref().unwrap(),
        1,
        &Arc::new(rode::images::Budget::default()),
        128,
    )
    .unwrap();
    assert_eq!(loaded.source.pages, 2);
    request.format = Format::Html;
    request.destination = Some(Destination {
        path: root.join("slides.html").to_string_lossy().into(),
        overwrite: false,
    });
    remote
        .marp_request(&request, Some(plan), &AtomicBool::new(false))
        .unwrap();
    assert!(
        std::fs::read_to_string(root.join("slides.html"))
            .unwrap()
            .contains("SSH unsaved slide")
    );
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "saved source");
    assert!(
        remote
            .marp_request(&request, Some(plan), &AtomicBool::new(false))
            .is_err()
    );
    request.destination = None;
    request.text = "RODE_TEST_SLEEP".into();
    let _ = std::fs::remove_file(root.join("marp-pids.json"));
    let cancel = Arc::new(AtomicBool::new(false));
    let control = cancel.clone();
    let connection = remote.clone();
    let policy = plan.clone();
    let worker =
        std::thread::spawn(move || connection.marp_request(&request, Some(&policy), &cancel));
    let deadline = Instant::now() + Duration::from_secs(8);
    while !root.join("marp-pids.json").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    control.store(true, Ordering::Release);
    assert!(worker.join().unwrap().is_err());
    assert!(
        remote.connected(),
        "Cancelling Marp must preserve the authenticated SSH connection"
    );
}
fn latex_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan, outside: &str) {
    use rode::latex::{self, Settings};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let root = std::path::Path::new(&plan.root);
    let tool = root.join("latex-fixture");
    fs::write(&tool, include_bytes!("fixtures/latex.py")).unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(root.join("pages.pdf"), include_bytes!("fixtures/pages.pdf")).unwrap();
    let source = root.join("latex-main.tex");
    let text = if plan.active {
        format!("ASSERT_HIDDEN={outside}\n")
    } else {
        "normal mode\n".into()
    };
    fs::write(&source, &text).unwrap();
    let settings = Settings {
        program: tool.to_string_lossy().into(),
        formatter: tool.to_string_lossy().into(),
        checker: tool.to_string_lossy().into(),
        arguments: vec!["{file}".into(), "{outdir}".into()],
        output_directory: "latex-output".into(),
        ..Default::default()
    };
    let invocation = latex::Invocation {
        root: plan.root.clone(),
        path: source.to_string_lossy().into(),
        settings: settings.clone(),
    };
    let result = remote
        .latex_request(&invocation, plan, &AtomicBool::new(false))
        .unwrap();
    assert!(result.output.success(), "{}", result.output.stderr);
    let response = remote
        .request_in(
            &Request::ReadBinary {
                path: result.pdf.unwrap(),
            },
            Some(plan),
        )
        .unwrap();
    {
        use base64::{Engine, engine::general_purpose::STANDARD};
        assert_eq!(
            STANDARD.decode(response.binary.unwrap()).unwrap(),
            include_bytes!("fixtures/pages.pdf")
        );
    }
    let analysis = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::LatexAnalyze {
                    root: plan.root.clone(),
                    path: source.to_string_lossy().into(),
                    text: "UNFORMATTED\n🦀 ISSUE\n".into(),
                    settings,
                    format: true,
                    check: true,
                },
            },
            Some(plan),
        )
        .unwrap();
    let Some(rode::project::Response::Analysis(analysis)) = analysis.tool else {
        panic!()
    };
    assert_eq!(analysis.text, "formatted\n🦀 ISSUE\n");
    assert_eq!(analysis.diagnostics[0].column, 3);
    assert_eq!(fs::read_to_string(&source).unwrap(), text);
    fs::write(&source, "ERROR").unwrap();
    let result = remote
        .latex_request(&invocation, plan, &AtomicBool::new(false))
        .unwrap();
    assert!(!result.output.success() && result.pdf.is_none());
    assert_eq!(result.diagnostics[0].diagnostic.row, 2);
    fs::write(&source, "SLOW").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let connection = remote.clone();
    let policy = plan.clone();
    let worker = std::thread::spawn(move || connection.latex_request(&invocation, &policy, &flag));
    let until = Instant::now() + Duration::from_secs(8);
    while !root.join("latex-output/pids.json").exists() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    cancel.store(true, Ordering::Release);
    assert!(worker.join().unwrap().is_err());
    assert!(
        remote.connected(),
        "Cancelling LaTeX must preserve the authenticated SSH connection"
    );
    assert!(
        remote
            .request_in(
                &Request::Read {
                    path: source.to_string_lossy().into()
                },
                Some(plan)
            )
            .is_ok()
    );
}

fn native_attach(
    remote: &Remote,
    reply: &rode::sessions::Reply,
    info: &rode_session::Info,
) -> Terminal {
    let mut command = portable_pty::CommandBuilder::new("ssh");
    command.args(remote.channel_args(false));
    let mut argv = vec![reply.executable.clone()];
    argv.extend(reply.prefix.clone());
    argv.extend([
        "attach".into(),
        "--framed".into(),
        info.id.clone(),
        info.policy.clone(),
    ]);
    command.arg(
        argv.iter()
            .map(|arg| rode::ssh::quote(arg))
            .collect::<Vec<_>>()
            .join(" "),
    );
    Terminal::spawn_native(
        &eframe::egui::Context::default(),
        command,
        "Native SSH test".into(),
    )
    .unwrap()
}
fn native_upgrade_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    fn wait_output(terminal: &Terminal, token: &str, runtime: &std::path::Path) {
        let until = Instant::now() + Duration::from_secs(8);
        while !terminal.contents().contains(token) {
            assert!(
                terminal.is_alive() && Instant::now() < until,
                "Missing {token}: {}
Inputs: {:?}",
                terminal.contents(),
                std::fs::read_to_string(runtime.join("input"))
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    let runtime = std::path::Path::new(&plan.root)
        .parent()
        .unwrap()
        .join("legacy-runtime");
    let id = rode_session::new_id().unwrap();
    let policy = rode::sessions::fingerprint(plan);
    let mut legacy = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/session_v1.py"
        ))
        .arg(&runtime)
        .arg(&plan.root)
        .arg(&id)
        .arg(&policy)
        .spawn()
        .unwrap();
    struct StopLegacy<'a>(&'a mut std::process::Child);
    impl Drop for StopLegacy<'_> {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.0.id() as i32, libc::SIGTERM);
            }
            let _ = self.0.wait();
        }
    }
    let _stop = StopLegacy(&mut legacy);
    let wait = |label: &str, check: &dyn Fn() -> bool| {
        let until = Instant::now() + Duration::from_secs(8);
        while !check() {
            assert!(
                Instant::now() < until,
                "SSH terminal upgrade timed out: {label}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    wait("legacy startup", &|| runtime.join("socket").exists());
    let request = |operation| {
        remote.request(&Request::Tool {
            request: rode::project::Request::Sessions {
                root: plan.root.clone(),
                operation,
                helper: Some(native_test_helper(true)),
                directory: Some(runtime.to_string_lossy().into()),
            },
        })
    };
    {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let stale = b"#!/bin/sh\nexit 2\n";
        let error = remote
            .request(&Request::Tool {
                request: rode::project::Request::Sessions {
                    root: plan.root.clone(),
                    operation: rode::sessions::Operation::List,
                    helper: Some(rode::sessions::Helper {
                        digest: rode::files::digest(stale),
                        architecture: std::env::consts::ARCH.into(),
                        data: Some(STANDARD.encode(stale)),
                    }),
                    directory: Some(runtime.to_string_lossy().into()),
                },
            })
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("older protocol") && error.contains("build-sessiond.sh"),
            "{error}"
        );
    }
    let response = request(rode::sessions::Operation::List).unwrap();
    let Some(rode::project::Response::Sessions(reply)) = response.tool else {
        panic!()
    };
    let rode_session::Response::Sessions(infos) = &reply.response else {
        panic!()
    };
    let old = infos[0].clone();
    assert!(old.persistent && !old.persistence_supported);
    let old_view = native_attach(remote, &reply, &old);
    old_view
        .send(b"TOKEN=ssh_kept; printf 'LEGACY_%s\\n' READY\n".to_vec())
        .unwrap();
    wait_output(&old_view, "LEGACY_READY", &runtime);
    drop(old_view);
    let response = request(rode::sessions::Operation::Create {
        id: rode_session::new_id().unwrap(),
        name: "Updated terminal".into(),
        plan: Box::new(plan.clone()),
        policy: policy.clone(),
        python: "off".into(),
        persistent: false,
    })
    .unwrap();
    let Some(rode::project::Response::Sessions(reply)) = response.tool else {
        panic!()
    };
    let rode_session::Response::Session(info) = &reply.response else {
        panic!()
    };
    assert!(info.persistence_supported && !info.persistent);
    let current = native_attach(remote, &reply, info);
    current
        .send(b"printf 'UPDATED_%s\\n' READY\n".to_vec())
        .unwrap();
    wait_output(&current, "UPDATED_READY", &runtime);
    let error = request(rode::sessions::Operation::SetPersistence {
        id: id.clone(),
        policy: policy.clone(),
        persistent: false,
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("Start a new terminal"), "{error}");
    assert!(!error.contains("unknown variant"), "{error}");
    let old_view = native_attach(remote, &reply, &old);
    old_view
        .send(b"printf 'TOKEN_%s\\n' \"$TOKEN\"\n".to_vec())
        .unwrap();
    wait_output(&old_view, "TOKEN_ssh_kept", &runtime);
    assert!(
        !std::fs::read_to_string(runtime.join("input"))
            .unwrap()
            .contains("\\x04"),
        "Detaching a native view must not forward portable-pty's EOF sentinel to the shell"
    );
    let response = request(rode::sessions::Operation::List).unwrap();
    assert!(
        matches!(response.tool, Some(rode::project::Response::Sessions(rode::sessions::Reply { response: rode_session::Response::Sessions(infos), .. })) if infos.len() == 2)
    );
    drop(old_view);
    request(rode::sessions::Operation::End { id, policy }).unwrap();
    wait("legacy cleanup", &|| !runtime.join("socket").exists());
    current
        .send(b"printf 'UPDATED_%s\\n' SURVIVED\n".to_vec())
        .unwrap();
    wait_output(&current, "UPDATED_SURVIVED", &runtime);
    request(rode::sessions::Operation::SetPersistence {
        id: info.id.clone(),
        policy: info.policy.clone(),
        persistent: true,
    })
    .unwrap();
    wait("persistence update", &|| current.persistent());
    current.send(b"exit\n".to_vec()).unwrap();
    wait("natural shell exit", &|| !current.is_alive());
    // The exited session must disappear before the service's idle timeout,
    // while this already-open view retains its final output.
    let response = request(rode::sessions::Operation::List).unwrap();
    assert!(
        matches!(response.tool, Some(rode::project::Response::Sessions(rode::sessions::Reply { response: rode_session::Response::Sessions(infos), .. })) if infos.is_empty())
    );
    assert!(current.contents().contains("UPDATED_SURVIVED"));
    wait("new cleanup", &|| {
        !runtime.join("protocol-2/socket").exists()
    });
    let response = request(rode::sessions::Operation::List).unwrap();
    assert!(
        matches!(response.tool, Some(rode::project::Response::Sessions(rode::sessions::Reply { response: rode_session::Response::Sessions(infos), .. })) if infos.is_empty())
    );
}

fn native_terminal_start(
    remote: &Remote,
    plan: &rode::sandbox::Plan,
) -> (rode_session::Info, rode::sessions::Reply, String) {
    let env_response = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::CreateEnvironment {
                    root: plan.root.clone(),
                    path: ".venv".into(),
                    python: "python3".into(),
                    with_pip: false,
                },
            },
            Some(plan),
        )
        .unwrap();
    let Some(rode::project::Response::Command(output)) = env_response.tool else {
        panic!()
    };
    assert!(output.success(), "{}", output.stderr);
    let runtime = std::path::Path::new(&plan.root)
        .parent()
        .unwrap()
        .join("native-runtime")
        .to_string_lossy()
        .into_owned();
    let response = remote
        .request(&Request::Tool {
            request: rode::project::Request::Sessions {
                root: plan.root.clone(),
                operation: rode::sessions::Operation::Create {
                    persistent: true,
                    id: rode_session::new_id().unwrap(),
                    name: "Persistent SSH test".into(),
                    plan: Box::new(plan.clone()),
                    policy: rode::sessions::fingerprint(plan),
                    python: "auto".into(),
                },
                helper: Some(native_test_helper(true)),
                directory: Some(runtime.clone()),
            },
        })
        .unwrap();
    let Some(rode::project::Response::Sessions(reply)) = response.tool else {
        panic!()
    };
    let rode_session::Response::Session(info) = reply.response.clone() else {
        panic!()
    };
    let native = native_attach(remote, &reply, &info);
    native
        .send(
            b"python -c 'import sys;open(\"native-venv-prefix\",\"w\").write(sys.prefix)'\n"
                .to_vec(),
        )
        .unwrap();
    let prefix_file = std::path::Path::new(&plan.root).join("native-venv-prefix");
    let expected = format!("{}/.venv", plan.root);
    let deadline = Instant::now() + Duration::from_secs(8);
    while std::fs::read_to_string(&prefix_file).ok().as_deref() != Some(&expected)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        std::fs::read_to_string(prefix_file).unwrap(),
        expected,
        "{}",
        native.contents()
    );
    native
        .send(
            b"printf 'BACKGROUND_%s\\n' STARTED; sleep 2; printf 'BACKGROUND_%s\\n' DONE\n"
                .to_vec(),
        )
        .unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    while !native.contents().contains("BACKGROUND_STARTED") && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        native.contents().contains("BACKGROUND_STARTED"),
        "{}",
        native.contents()
    );
    drop(native);
    (info, reply, runtime)
}

fn agent_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    use rode::agent::{Config, Launch, Provider, protocol::Chat};
    use std::{os::unix::fs::PermissionsExt, sync::Arc};
    let root = std::path::Path::new(&plan.root);
    let program = root.join("agent 'quoted'.py");
    std::fs::write(&program, include_str!("fixtures/agent.py")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.parent().unwrap().join("private.txt"), "hidden").unwrap();
    for provider in [Provider::Codex, Provider::Acp] {
        let launch = Launch {
            config: Config {
                provider,
                codex: program.to_string_lossy().into(),
                acp_program: program.to_string_lossy().into(),
                ..Default::default()
            },
            plan: plan.clone(),
            remote: Some(remote.clone()),
            python: "auto".into(),
        };
        let mut chat = Chat::default();
        chat.submit(launch.clone(), "remote".into(), Arc::new(|| {}))
            .unwrap();
        wait_agent(&mut chat, |c| c.completed == 1);
        let scope: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("agent-scope.json")).unwrap())
                .unwrap();
        assert_eq!(scope["cwd"], plan.root);
        assert_eq!(scope["private_visible"], !plan.active);
        assert_eq!(
            std::fs::read_to_string(root.join("agent-created.txt")).unwrap(),
            "remote"
        );
        let _ = std::fs::remove_file(root.join("approved.txt"));
        chat.submit(launch.clone(), "approve".into(), Arc::new(|| {}))
            .unwrap();
        wait_agent(&mut chat, |c| !c.prompts.is_empty());
        assert!(!root.join("approved.txt").exists());
        chat.answer(chat.prompts[0].choices[0].result.clone())
            .unwrap();
        wait_agent(&mut chat, |c| c.completed == 2);
        assert!(root.join("approved.txt").exists());
        chat.submit(launch.clone(), "wait".into(), Arc::new(|| {}))
            .unwrap();
        wait_agent(&mut chat, |c| {
            c.messages.iter().any(|m| m.text.contains("long task"))
        });
        chat.cancel();
        wait_agent(&mut chat, |c| !c.busy);
        assert!(chat.connected());
        std::fs::remove_file(root.join("agent-child.pid")).unwrap();
        chat.submit(launch, "wait".into(), Arc::new(|| {})).unwrap();
        wait_agent(&mut chat, |_| {
            std::fs::read_to_string(root.join("agent-child.pid"))
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .is_some()
        });
        let pid: u32 = std::fs::read_to_string(root.join("agent-child.pid"))
            .unwrap()
            .parse()
            .unwrap();
        drop(chat);
        let until = Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
            !s.rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
        }) {
            assert!(
                Instant::now() < until,
                "Closing SSH agent left its tool process alive"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            remote.connected(),
            "Closing agent chat must preserve the authenticated SSH master"
        );
        assert!(
            remote
                .request(&Request::List {
                    path: plan.root.clone()
                })
                .is_ok()
        );
    }
}

fn wait_agent(
    chat: &mut rode::agent::protocol::Chat,
    ready: impl Fn(&rode::agent::protocol::Chat) -> bool,
) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        chat.poll();
        if ready(chat) {
            return;
        }
        assert!(
            Instant::now() < until,
            "{}: {}",
            chat.status,
            chat.messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn binary_extension_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan, outside: &str) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let root = std::path::Path::new(&plan.root);
    let path = root
        .join("binary 'quoted' 🦀.bin")
        .to_string_lossy()
        .into_owned();
    let bytes = [0, 0xff, 0x80, 0x90, 0xc3];
    std::fs::write(&path, bytes).unwrap();
    let response = remote
        .request_in(&Request::ReadHex { path: path.clone() }, Some(plan))
        .unwrap();
    assert_eq!(STANDARD.decode(response.binary.unwrap()).unwrap(), bytes);
    remote
        .request_in(
            &Request::WriteBinary {
                path: path.clone(),
                data: STANDARD.encode([0, 0x90, 0x90, 0x90, 0xc3]),
                expected: Some(response.revision),
            },
            Some(plan),
        )
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), [0, 0x90, 0x90, 0x90, 0xc3]);
    if plan.active {
        assert!(
            remote
                .request_in(
                    &Request::ReadHex {
                        path: outside.into()
                    },
                    Some(plan)
                )
                .is_err()
        );
    }
    let adapter = root.join("extension.py");
    std::fs::write(&adapter, include_str!("fixtures/extension.py")).unwrap();
    let invocation = rode::extensions::Invocation {
        api: 1,
        extension: "test.adapter".into(),
        program: "python3".into(),
        args: vec![adapter.to_string_lossy().into()],
        operation: "format".into(),
        path: Some(path),
        text: "unsaved".into(),
        settings: Default::default(),
    };
    let reply = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::Extension {
                    root: plan.root.clone(),
                    invocation: invocation.clone(),
                },
            },
            Some(plan),
        )
        .unwrap()
        .tool
        .unwrap();
    let rode::project::Response::Extension(reply) = reply else {
        panic!()
    };
    reply.validate("format").unwrap();
    assert_eq!(reply.text.as_deref(), Some("UNSAVED"));
    let mut invocation = invocation;
    invocation.operation = "test.inspect".into();
    invocation
        .settings
        .insert("outside".into(), serde_json::json!(outside));
    let reply = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::Extension {
                    root: plan.root.clone(),
                    invocation,
                },
            },
            Some(plan),
        )
        .unwrap()
        .tool
        .unwrap();
    let rode::project::Response::Extension(reply) = reply else {
        panic!()
    };
    assert_eq!(reply.output, if plan.active { "False" } else { "True" });
}

fn notebook_roundtrip(remote: &Remote, root: &str, outside: &str) {
    use rode::notebook::kernel::{Event, Kernel, Run};
    let Ok(python) = std::env::var("RODE_TEST_JUPYTER") else {
        return;
    };
    let env = std::path::Path::new(&python)
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let plan = remote
        .request(&Request::Prepare {
            path: root.into(),
            sandbox: rode::sandbox::Options {
                enabled: true,
                share_network: false,
                read_only: vec![env.to_string_lossy().into()],
                ..Default::default()
            },
        })
        .unwrap()
        .project
        .unwrap();
    let file = std::path::Path::new(root)
        .join("notebook 'quoted' 🦀.ipynb")
        .to_string_lossy()
        .into_owned();
    let wrote = remote
        .request_in(
            &Request::Write {
                path: file.clone(),
                text: include_str!("fixtures/notebook.ipynb").into(),
                expected: None,
            },
            Some(&plan),
        )
        .unwrap();
    let read = remote
        .request_in(&Request::Read { path: file.clone() }, Some(&plan))
        .unwrap();
    assert_eq!(read.revision, wrote.revision);
    let book = rode::notebook::Notebook::parse(&read.text).unwrap();
    assert_eq!(book.cells.len(), 4);
    let mut kernel = Kernel::start(
        plan.clone(),
        Some(remote.clone()),
        python,
        "python3".into(),
        std::sync::Arc::new(|| {}),
    )
    .unwrap();
    kernel.run(vec![Run{uid:1,version:0,code:"value=40".into()},Run{uid:2,version:0,code:format!("import os\nprint(value+2)\nprint(os.getcwd())\nprint(os.path.exists({outside:?}))")}]).unwrap();
    let mut done = 0;
    let mut text = String::new();
    let start = Instant::now();
    while done < 2 {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{}",
            kernel.status
        );
        for event in kernel.poll() {
            match event {
                Event::Done(_, status) => {
                    assert_eq!(status, "ok");
                    done += 1;
                }
                Event::Output { output, .. } => {
                    if let Some(value) = output["text"].as_str() {
                        text.push_str(value);
                    }
                }
                Event::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(text.contains("42\n"));
    assert!(text.contains(root));
    assert!(text.contains(if plan.active { "False\n" } else { "True\n" }));
    kernel.stop();
    assert!(
        remote.connected(),
        "Notebook shutdown ended the shared SSH connection"
    );
}

fn encrypted_roundtrip(remote: &Remote, root: &std::path::Path) {
    let cipher = root.join("encrypted quoted 'folder'");
    std::fs::create_dir(&cipher).unwrap();
    std::fs::write(cipher.join("gocryptfs.conf"), "fixture").unwrap();
    let request = rode::vault::Open {
        settings: rode::vault::Settings {
            cipherdir: cipher.to_string_lossy().into(),
            program: "/usr/bin/true".into(),
            ..Default::default()
        },
        password: "ssh-test-password".into(),
        create: false,
        lock_only: false,
    };
    let client =
        rode::vault::Client::start(request, Some(remote.clone()), std::sync::Arc::new(|| {}))
            .unwrap();
    let until = Instant::now() + Duration::from_secs(15);
    let mut failed = false;
    while Instant::now() < until {
        if let Some(rode::process::Event::Message(value)) = client.session.poll()
            && let Ok(rode::vault::Reply::Error { message }) = serde_json::from_value(value)
        {
            assert!(message.contains("verified encrypted mount"), "{message}");
            assert!(!message.contains("ssh-test-password"));
            failed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        failed,
        "SSH encrypted mount verification did not fail closed"
    );
    drop(client);
    assert!(
        remote.connected(),
        "Encrypted-project cleanup closed the SSH master"
    );
}

fn debugger_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rode::debugger::{
        Backend, Breakpoint, Event, Settings,
        protocol::{Client, Phase},
    };
    use std::{
        fs,
        sync::{Arc, atomic::Ordering},
    };
    let root = std::path::Path::new(&plan.root);
    let adapter = root.join("debug-adapter.py");
    fs::write(&adapter, include_str!("fixtures/debug_adapter.py")).unwrap();
    let source = root.join("main.py");
    fs::write(&source,"answer = 40\nanswer += 2\nprint(answer, flush=True)\nname=input('Name: ')\nprint('HELLO '+name, flush=True)\n").unwrap();
    let wait = |client: &mut Client, check: &dyn Fn(&Client) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            client.poll();
            if check(client) {
                break;
            }
            assert!(
                client.error.is_empty(),
                "{}\n{}",
                client.error,
                client.output
            );
            assert!(
                Instant::now() < deadline,
                "{}\n{}",
                client.status,
                client.output
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let mut settings = Settings::default();
    settings.config.backend = Backend::Custom;
    settings.config.adapter = "python3".into();
    settings.config.adapter_arguments = vec![adapter.to_string_lossy().into()];
    settings.breakpoints.insert(
        source.to_string_lossy().into(),
        vec![Breakpoint {
            line: 3,
            ..Default::default()
        }],
    );
    settings.watches = vec!["answer".into()];
    let mut client = Client::start(
        plan.clone(),
        Some(remote.clone()),
        source.to_string_lossy().into(),
        "auto".into(),
        settings,
        Arc::new(|| {}),
    )
    .unwrap();
    wait(&mut client, &|c| {
        c.phase == Phase::Paused && c.watch_values[0] == "42 🦀"
    });
    let mut data = String::new();
    let mut terminal = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !data.contains("Name: ") {
        client.poll();
        while let Some(event) = client.terminal_events.pop_front() {
            match event {
                Event::Terminal { id, .. } => terminal = Some(id),
                Event::Data { data: bytes, .. } => {
                    data.push_str(&String::from_utf8_lossy(&STANDARD.decode(bytes).unwrap()))
                }
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "{data}");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(data.contains("TTY=True"));
    client.writer().unwrap().send(&serde_json::json!({"op":"input","id":terminal.unwrap(),"data":STANDARD.encode("SSH\n")})).unwrap();
    wait(&mut client, &|_| root.join("input-result").exists());
    assert_eq!(
        fs::read_to_string(root.join("input-result")).unwrap(),
        "SSH"
    );
    client.control("next").unwrap();
    wait(&mut client, &|c| {
        c.phase == Phase::Paused && c.frames.first().is_some_and(|f| f["line"] == 4)
    });
    client.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    while client.cleanup().load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        remote.connected(),
        "Debug cleanup closed authenticated master"
    );
    if let Ok(python) =
        std::env::var("RODE_TEST_DEBUGPY").or_else(|_| std::env::var("RODE_TEST_JUPYTER"))
    {
        let env = std::path::Path::new(&python)
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let policy = remote
            .request(&Request::Prepare {
                path: plan.root.clone(),
                sandbox: rode::sandbox::Options {
                    enabled: true,
                    read_only: vec![env.to_string_lossy().into()],
                    ..Default::default()
                },
            })
            .unwrap()
            .project
            .unwrap();
        let mut settings = Settings::default();
        settings.breakpoints.insert(
            source.to_string_lossy().into(),
            vec![Breakpoint {
                line: 2,
                ..Default::default()
            }],
        );
        settings.watches = vec!["answer".into()];
        let mut client = Client::start(
            policy,
            Some(remote.clone()),
            source.to_string_lossy().into(),
            python,
            settings,
            Arc::new(|| {}),
        )
        .unwrap();
        wait(&mut client, &|c| {
            c.phase == Phase::Paused && c.watch_values[0] == "40"
        });
        client.control("next").unwrap();
        wait(&mut client, &|c| {
            c.phase == Phase::Paused && c.watch_values[0] == "42"
        });
        client.stop();
        let deadline = Instant::now() + Duration::from_secs(8);
        while client.cleanup().load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(remote.connected());
    }
}

fn native_debugger_roundtrip(remote: &Remote, root: &str) {
    use rode::debugger::{
        Backend, Breakpoint, Settings,
        protocol::{Client, Phase},
    };
    use std::{
        fs,
        sync::{Arc, atomic::Ordering},
    };
    let Ok(adapter) = std::env::var("RODE_TEST_LLDB") else {
        return;
    };
    let source = std::path::Path::new(root).join("native-debug.c");
    let binary = std::path::Path::new(root).join("native-debug");
    fs::write(&source,"#include <stdio.h>\nint main(void){\n int answer=40;\n answer+=2;\n printf(\"%d\\n\",answer);\n return 0;\n}\n").unwrap();
    let status = Command::new("cc")
        .arg("-g")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .status()
        .unwrap();
    assert!(status.success());
    let plan = remote
        .request(&Request::Prepare {
            path: root.into(),
            sandbox: rode::sandbox::Options {
                enabled: true,
                read_only: vec![
                    std::path::Path::new(&adapter)
                        .parent()
                        .unwrap()
                        .to_string_lossy()
                        .into(),
                ],
                ..Default::default()
            },
        })
        .unwrap()
        .project
        .unwrap();
    let mut settings = Settings::default();
    settings.config.backend = Backend::Lldb;
    settings.config.adapter = adapter;
    settings.config.program = binary.to_string_lossy().into();
    if let Ok(libraries) = std::env::var("LD_LIBRARY_PATH") {
        settings.config.extra = serde_json::json!({"env":{"LD_LIBRARY_PATH":libraries}});
    }
    settings.breakpoints.insert(
        source.to_string_lossy().into(),
        vec![Breakpoint {
            line: 4,
            ..Default::default()
        }],
    );
    settings.watches = vec!["answer".into()];
    let mut client = Client::start(
        plan,
        Some(remote.clone()),
        source.to_string_lossy().into(),
        "auto".into(),
        settings,
        Arc::new(|| {}),
    )
    .unwrap();
    let wait = |client: &mut Client, value: &str| {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            client.poll();
            if client.phase == Phase::Paused && client.watch_values[0].contains(value) {
                break;
            }
            if !client.error.is_empty() {
                for event in &client.terminal_events {
                    if let rode::debugger::Event::Data { data, .. } = event {
                        use base64::{Engine, engine::general_purpose::STANDARD};
                        eprintln!(
                            "Debug terminal: {}",
                            String::from_utf8_lossy(&STANDARD.decode(data).unwrap_or_default())
                        );
                    }
                }
            }
            assert!(
                client.error.is_empty(),
                "{}\n{}",
                client.error,
                client.output
            );
            assert!(
                Instant::now() < deadline,
                "{}\n{}",
                client.status,
                client.output
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    wait(&mut client, "40");
    client.control("next").unwrap();
    wait(&mut client, "42");
    client.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    while client.cleanup().load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(remote.connected());
}

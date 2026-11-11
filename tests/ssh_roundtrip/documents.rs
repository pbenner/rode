//! Document tools and protected file operations over the shared SSH fixture.
use super::*;

pub(super) fn marp_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
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
    std::fs::write(&program, include_bytes!("../fixtures/marp.py")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        root.join("pages.pdf"),
        include_bytes!("../fixtures/pages.pdf"),
    )
    .unwrap();
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

pub(super) fn latex_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan, outside: &str) {
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
    fs::write(&tool, include_bytes!("../fixtures/latex.py")).unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        root.join("pages.pdf"),
        include_bytes!("../fixtures/pages.pdf"),
    )
    .unwrap();
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
            include_bytes!("../fixtures/pages.pdf")
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

pub(super) fn binary_extension_roundtrip(
    remote: &Remote,
    plan: &rode::sandbox::Plan,
    outside: &str,
) {
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
    std::fs::write(&adapter, include_str!("../fixtures/extension.py")).unwrap();
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

pub(super) fn notebook_roundtrip(remote: &Remote, root: &str, outside: &str) {
    use rode::notebook::kernel::{Event, Kernel, Run};
    let Ok(python) = std::env::var("RODE_TEST_JUPYTER") else {
        capabilities::skip(
            "jupyter",
            "Set RODE_TEST_JUPYTER for remote kernel coverage",
        );
        return;
    };
    let plan = remote
        .request(&Request::Prepare {
            path: root.into(),
            sandbox: rode::sandbox::Options {
                enabled: true,
                share_network: false,
                read_only: interpreter_grants(&python),
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
                text: include_str!("../fixtures/notebook.ipynb").into(),
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
    kernel.run(vec![Run{cell: String::new(),
uid:1,version:0,code:"value=40".into()},Run{cell: String::new(),
uid:2,version:0,code:format!("import os\nprint(value+2)\nprint(os.getcwd())\nprint(os.path.exists({outside:?}))")}]).unwrap();
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

pub(super) fn encrypted_roundtrip(remote: &Remote, root: &std::path::Path) {
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

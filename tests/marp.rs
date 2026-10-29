//! Snapshot rendering/export, atomic destination guards and native/Python renderer cleanup with
//! deterministic fixtures.

use rode::marp::{self, Destination, Format, Invocation, Settings};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn fixture(root: &Path) -> Invocation {
    use std::os::unix::fs::PermissionsExt;
    fs::write(root.join("marp.py"), include_bytes!("fixtures/marp.py")).unwrap();
    fs::set_permissions(root.join("marp.py"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(root.join("pages.pdf"), include_bytes!("fixtures/pages.pdf")).unwrap();
    let source = root.join("deck.md");
    fs::write(&source, "saved source").unwrap();
    Invocation {
        root: root.to_string_lossy().into(),
        path: source.to_string_lossy().into(),
        text: "---\nmarp: true\n---\n# Unsaved deck\n\n---\n\n# Slide 2\n".into(),
        settings: Settings {
            program: root.join("marp.py").to_string_lossy().into(),
            ..Default::default()
        },
        format: Format::Html,
        destination: None,
    }
}
fn wait(check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn marp_detection_and_unicode_source_urls_are_bounded_data() {
    assert!(marp::is_deck(Some("slides.marp.md"), ""));
    assert!(marp::is_deck(
        None,
        "---\nmarp: true # deck\ntheme: gaia\n---\n# Hello"
    ));
    assert!(!marp::is_deck(None, "---\nmarp: false\n---\nmarp: true"));
    assert!(!marp::is_deck(None, "# Document\nmarp: true"));
    assert_eq!(
        marp::file_url(Path::new("/tmp/a '雪#%.md")),
        "file:///tmp/a%20%27%E9%9B%AA%23%25.md"
    );
}

#[test]
fn native_and_python_peers_render_unsaved_snapshots_and_protect_exports() {
    for peer in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut invocation = fixture(root.path());
        invocation.text.push_str("\nRODE_TEST_RECORD");
        let run = |request: &Invocation| {
            if peer {
                let mut command = Command::new("python3");
                command.args(["-c", rode::files::REMOTE_HELPER, "--marp-helper"]);
                marp::execute(command, request, &AtomicBool::new(false))
            } else {
                marp::run(request, &AtomicBool::new(false))
            }
        };
        invocation.format = Format::Pdf;
        let reply = run(&invocation).unwrap();
        let budget = Arc::new(rode::images::Budget::default());
        let loaded = rode::pdf::load(reply.data.as_deref().unwrap(), 1, &budget, 128).unwrap();
        assert_eq!(loaded.source.pages, 2);
        let snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("marp-snapshot.json")).unwrap())
                .unwrap();
        assert_eq!(snapshot["text"], invocation.text);
        assert_eq!(
            snapshot["config"]["baseUrl"],
            marp::file_url(Path::new(&invocation.path))
        );
        invocation.format = Format::Html;
        invocation.destination = Some(Destination {
            path: "deck.html".into(),
            overwrite: false,
        });
        run(&invocation).unwrap();
        let output = fs::read(root.path().join("deck.html")).unwrap();
        assert!(String::from_utf8_lossy(&output).contains("Unsaved deck"));
        assert!(run(&invocation).is_err());
        invocation.destination.as_mut().unwrap().overwrite = true;
        invocation.text.push_str("\nNewer snapshot");
        run(&invocation).unwrap();
        assert!(
            fs::read_to_string(root.path().join("deck.html"))
                .unwrap()
                .contains("Newer snapshot")
        );
        invocation.text.push_str("\nRODE_TEST_CONFLICT");
        assert!(run(&invocation).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("deck.html")).unwrap(),
            "external change"
        );
        assert_eq!(
            fs::read_to_string(&invocation.path).unwrap(),
            "saved source"
        );
        std::os::unix::fs::symlink(&invocation.path, root.path().join("source-alias.html"))
            .unwrap();
        invocation.destination.as_mut().unwrap().path = "source-alias.html".into();
        assert!(
            run(&invocation).is_err(),
            "An export must never follow an alias over its source"
        );
        assert_eq!(
            fs::read_to_string(&invocation.path).unwrap(),
            "saved source"
        );
        invocation.destination.as_mut().unwrap().path = "too-large.html".into();
        invocation.text = "RODE_TEST_OVERSIZE".into();
        assert!(run(&invocation).is_err());
        assert!(!root.path().join("too-large.html").exists());
        invocation.text = "x".repeat(marp::MAX_SOURCE + 1);
        assert!(run(&invocation).is_err());
    }
}

#[test]
fn cancelling_native_and_python_coordinators_stops_the_cli_process_tree() {
    for peer in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut invocation = fixture(root.path());
        invocation.text.push_str("RODE_TEST_SLEEP");
        let cancel = Arc::new(AtomicBool::new(false));
        let control = cancel.clone();
        let worker = std::thread::spawn(move || {
            let mut command = if peer {
                let mut c = Command::new("python3");
                c.args(["-c", rode::files::REMOTE_HELPER, "--marp-helper"]);
                c
            } else {
                let mut c = Command::new(env!("CARGO_BIN_EXE_rode"));
                c.arg("--marp-helper");
                c
            };
            command.current_dir(&invocation.root);
            marp::execute(command, &invocation, &cancel)
        });
        let pids = root.path().join("marp-pids.json");
        wait(|| pids.exists());
        let pids: Vec<i32> = serde_json::from_slice(&fs::read(pids).unwrap()).unwrap();
        control.store(true, Ordering::Release);
        assert!(worker.join().unwrap().is_err());
        for pid in pids {
            wait(|| {
                let state = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
                state.is_empty()
                    || state
                        .split_once(") ")
                        .is_some_and(|(_, s)| s.starts_with('Z'))
            });
        }
    }
}

#[test]
fn installed_marp_converts_real_themes_and_all_export_formats_when_requested() {
    let Ok(program) = std::env::var("RODE_TEST_MARP") else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let mut invocation = fixture(root.path());
    invocation.settings.program = program;
    invocation.settings.browser = std::env::var("RODE_TEST_MARP_BROWSER").unwrap_or_default();
    invocation.text = "---\nmarp: true\ntheme: gaia\nclass: invert\npaginate: true\n---\n# A real deck\n\n---\n\n# Second slide\n\n$$E = mc^2$$\n".into();
    for format in [Format::Html, Format::Pdf, Format::Pptx] {
        invocation.format = format;
        invocation.destination = Some(Destination {
            path: format!("slides.{}", format.extension()),
            overwrite: false,
        });
        marp::run(&invocation, &AtomicBool::new(false))
            .unwrap_or_else(|error| panic!("{format:?}: {error:#}"));
        let bytes = fs::read(root.path().join(format!("slides.{}", format.extension()))).unwrap();
        assert!(!bytes.is_empty());
        if format == Format::Html {
            let html = String::from_utf8(bytes).unwrap();
            assert!(html.contains("A real deck") && html.contains("gaia"));
        } else if format == Format::Pdf {
            let loaded =
                rode::pdf::load_bytes(bytes, 1, &Arc::new(rode::images::Budget::default()), 256)
                    .unwrap();
            assert_eq!(loaded.source.pages, 2);
        } else {
            assert!(bytes.starts_with(b"PK"));
        }
    }
    assert_eq!(
        fs::read_to_string(&invocation.path).unwrap(),
        "saved source"
    );
}

#[test]
fn native_and_remote_style_previews_work_in_a_read_only_sandbox_without_project_writes() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("project");
    fs::create_dir(&root).unwrap();
    let mut invocation = fixture(&root);
    let hidden = base.path().join("outside.txt");
    fs::write(&hidden, "private").unwrap();
    invocation.text = format!("# Slides\nRODE_TEST_SCOPE={}\n", hidden.display());
    let options = rode::sandbox::Options {
        enabled: true,
        project_read_only: true,
        share_network: false,
        ..Default::default()
    };
    let plan = rode::sandbox::prepare(root.to_str().unwrap(), &options).unwrap();
    if !plan.active {
        eprintln!("Read-only Marp sandbox unavailable: {}", plan.reason);
        return;
    }
    for peer in [false, true] {
        let mut command = Command::new(&plan.prefix[0]);
        command.args(&plan.prefix[1..plan.prefix.len() - 1]);
        if peer {
            command.args([
                "--",
                "python3",
                "-c",
                rode::files::REMOTE_HELPER,
                "--marp-helper",
            ]);
        } else {
            command.args([
                "--ro-bind",
                env!("CARGO_BIN_EXE_rode"),
                "/rode-marp-helper",
                "--",
                "/rode-marp-helper",
                "--marp-helper",
            ]);
        }
        let reply = marp::execute(command, &invocation, &AtomicBool::new(false)).unwrap();
        use base64::{Engine, engine::general_purpose::STANDARD};
        let output = String::from_utf8(STANDARD.decode(reply.data.unwrap()).unwrap()).unwrap();
        assert!(output.contains("Outside visible: False"));
        assert_eq!(
            fs::read_to_string(&invocation.path).unwrap(),
            "saved source"
        );
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            3,
            "Rendering must not write temporary source files into the project"
        );
    }
}

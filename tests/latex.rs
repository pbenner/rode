//! Generic saved-source builds, compiler/checker diagnostics, cancellation, output locking and
//! sandbox enforcement.

use rode::{
    files,
    latex::{self, Invocation, Settings},
    project, sandbox,
};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

static TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn fixture(root: &Path) -> Settings {
    use std::os::unix::fs::PermissionsExt;
    let tools = root.join("tools");
    fs::create_dir_all(&tools).unwrap();
    fs::write(
        tools.join("latex-tool"),
        include_bytes!("fixtures/latex.py"),
    )
    .unwrap();
    fs::set_permissions(tools.join("latex-tool"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        tools.join("pages.pdf"),
        include_bytes!("fixtures/pages.pdf"),
    )
    .unwrap();
    Settings {
        program: tools.join("latex-tool").to_string_lossy().into(),
        formatter: tools.join("latex-tool").to_string_lossy().into(),
        checker: tools.join("latex-tool").to_string_lossy().into(),
        arguments: vec!["{file}".into(), "{outdir}".into(), "{root}".into()],
        output_directory: "build/latex".into(),
        ..Default::default()
    }
}
fn build(invocation: &Invocation, peer: bool, cancel: &AtomicBool) -> anyhow::Result<latex::Reply> {
    let mut command = if peer {
        Command::new("python3")
    } else {
        Command::new(env!("CARGO_BIN_EXE_rode"))
    };
    if peer {
        command.args(["-c", files::REMOTE_HELPER, "--latex-helper"]);
    } else {
        command.arg("--latex-helper");
    }
    for (name, test_name) in [
        ("TEXMF", "RODE_TEST_TEXMF"),
        ("TEXFORMATS", "RODE_TEST_TEXFORMATS"),
    ] {
        if let Some(value) = std::env::var_os(test_name) {
            command.env(name, value);
        }
    }
    let home = Path::new(&invocation.root).join("test-home");
    fs::create_dir_all(&home).unwrap();
    command
        .env("HOME", &home)
        .env("TEXMFVAR", home.join("texmf-var"))
        .env("TEXMFCONFIG", home.join("texmf-config"));
    latex::execute(command, invocation, cancel)
}
fn peer_analyze(request: &project::Request) -> project::Analysis {
    let mut child = Command::new("python3")
        .args(["-c", files::REMOTE_HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            &serde_json::to_vec(&files::Request::Tool {
                request: request.clone(),
            })
            .unwrap(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: files::Response = serde_json::from_slice(&output.stdout).unwrap();
    let Some(project::Response::Analysis(analysis)) = response.tool else {
        panic!()
    };
    analysis
}
fn wait(check: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < until, "LaTeX fixture timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn generic_builds_preserve_source_and_resolve_roots_and_pdf_names_on_both_peers() {
    let _guard = TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for peer in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let settings = fixture(root.path());
        fs::create_dir(root.path().join("chapters")).unwrap();
        let source = root.path().join("paper.v1 '🦀{dir}.tex");
        fs::write(&source, "saved source").unwrap();
        let chapter = root.path().join("chapters/intro.tex");
        fs::write(
            &chapter,
            "% !TeX root = ../paper.v1 '🦀{dir}.tex\nChapter text\n",
        )
        .unwrap();
        let invocation = Invocation {
            root: root.path().to_string_lossy().into(),
            path: chapter.to_string_lossy().into(),
            settings,
        };
        let result = build(&invocation, peer, &AtomicBool::new(false)).unwrap();
        assert!(result.output.success());
        assert_eq!(result.source, source.to_string_lossy());
        let pdf = root.path().join("build/latex/paper.v1 '🦀{dir}.pdf");
        assert_eq!(result.pdf.as_deref(), Some(pdf.to_str().unwrap()));
        assert_eq!(
            fs::read(&pdf).unwrap(),
            include_bytes!("fixtures/pages.pdf")
        );
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("build/latex/invocation.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record["arguments"][0], source.to_str().unwrap());
        assert_eq!(record["arguments"][2], root.path().to_str().unwrap());
        assert_eq!(record["cwd"], root.path().to_str().unwrap());
        assert_eq!(fs::read_to_string(&source).unwrap(), "saved source");
        fs::write(&source, "ERROR").unwrap();
        let result = build(&invocation, peer, &AtomicBool::new(false)).unwrap();
        assert!(!result.output.success() && result.pdf.is_none());
        assert_eq!(result.diagnostics[0].diagnostic.row, 2);
        assert_eq!(result.diagnostics[0].diagnostic.column, 5);
        fs::write(&source, "% !TeX root = chapters/intro.tex").unwrap();
        assert!(build(&invocation, peer, &AtomicBool::new(false)).is_err());
    }
}

#[test]
fn latex_buffer_formatting_and_linting_match_without_writing_source() {
    let _guard = TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = tempfile::tempdir().unwrap();
    let settings = fixture(root.path());
    let path = root.path().join("main.tex");
    fs::write(&path, "saved source").unwrap();
    let request = project::Request::LatexAnalyze {
        root: root.path().to_string_lossy().into(),
        path: path.to_string_lossy().into(),
        text: "UNFORMATTED\n🦀 ISSUE\n".into(),
        settings,
        format: true,
        check: true,
    };
    let project::Response::Analysis(local) = project::local(&request).unwrap() else {
        panic!()
    };
    let remote = peer_analyze(&request);
    assert_eq!(local.text, "formatted\n🦀 ISSUE\n");
    assert_eq!(
        serde_json::to_value(&local).unwrap(),
        serde_json::to_value(remote).unwrap()
    );
    assert_eq!(local.diagnostics[0].column, 3);
    assert_eq!(local.diagnostics[0].severity, "warning");
    assert_eq!(fs::read_to_string(path).unwrap(), "saved source");
    assert!(!root.path().join("indent.log").exists());
}

#[test]
fn builds_cancel_owned_children_and_coordinate_shared_output_directories() {
    let _guard = TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for peer in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let settings = fixture(root.path());
        let path = root.path().join("main.tex");
        fs::write(&path, "SLOW").unwrap();
        let invocation = Invocation {
            root: root.path().to_string_lossy().into(),
            path: path.to_string_lossy().into(),
            settings,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let request = invocation.clone();
        let worker = std::thread::spawn(move || build(&request, peer, &flag));
        let pids = root.path().join("build/latex/pids.json");
        wait(|| pids.exists());
        let ids: Vec<u32> = serde_json::from_slice(&fs::read(&pids).unwrap()).unwrap();
        let error = build(&invocation, !peer, &AtomicBool::new(false))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Another RODE build"), "{error}");
        cancel.store(true, Ordering::Release);
        assert!(worker.join().unwrap().is_err());
        wait(|| {
            ids.iter().all(|pid| {
                fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |text| {
                    text.split_once(") ").unwrap().1.starts_with('Z')
                })
            })
        });
        fs::write(&path, "after cancellation").unwrap();
        assert!(
            build(&invocation, peer, &AtomicBool::new(false))
                .unwrap()
                .output
                .success()
        );
    }
}

#[test]
fn real_latexmk_produces_pdf_and_located_compiler_errors_when_installed() {
    let _guard = TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if Command::new("latexmk").arg("-v").output().is_err()
        || Command::new("pdflatex").arg("--version").output().is_err()
    {
        eprintln!("Real TeX checks need installed latexmk and pdfLaTeX");
        return;
    }
    let mut probe = Command::new("kpsewhich");
    probe.arg("pdflatex.fmt");
    if let Some(value) = std::env::var_os("RODE_TEST_TEXFORMATS") {
        probe.env("TEXFORMATS", value);
    }
    if !probe
        .output()
        .is_ok_and(|out| out.status.success() && !out.stdout.is_empty())
    {
        eprintln!(
            "Real TeX checks need a usable pdflatex.fmt; set RODE_TEST_TEXFORMATS for an isolated development format directory."
        );
        return;
    }
    for peer in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("paper space.tex");
        let source = "\\documentclass{article}\n\\begin{document}\nHello, \\LaTeX! $E=mc^2$.\n\\end{document}\n";
        fs::write(&path, source).unwrap();
        let request = Invocation {
            root: root.path().to_string_lossy().into(),
            path: path.to_string_lossy().into(),
            settings: Default::default(),
        };
        let result = build(&request, peer, &AtomicBool::new(false)).unwrap();
        assert!(
            result.output.success(),
            "{}\n{}",
            result.output.stdout,
            result.output.stderr
        );
        assert!(result.pdf.is_some(), "{}", result.note);
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
        fs::write(&path, source.replace("Hello,", "\\undefinedRODEcommand")).unwrap();
        let result = build(&request, peer, &AtomicBool::new(false)).unwrap();
        assert!(!result.output.success() && result.pdf.is_none());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|issue| issue.path == path.to_string_lossy() && issue.diagnostic.row == 3)
        );
    }
}

#[test]
fn actual_sandbox_builds_obey_project_grants() {
    let _guard = TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let settings = fixture(&project);
    let hidden = root.path().join("outside-secret");
    fs::write(&hidden, "hidden").unwrap();
    let path = project.join("main.tex");
    fs::write(&path, format!("ASSERT_HIDDEN={}\n", hidden.display())).unwrap();
    let plan = sandbox::prepare(
        project.to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    if !plan.active {
        eprintln!("Sandbox unavailable: {}", plan.reason);
        return;
    }
    let invocation = Invocation {
        root: project.to_string_lossy().into(),
        path: path.to_string_lossy().into(),
        settings,
    };
    for peer in [false, true] {
        let mut command = Command::new(&plan.prefix[0]);
        command.args(&plan.prefix[1..plan.prefix.len() - 1]);
        if peer {
            command.args([
                "--",
                "python3",
                "-c",
                files::REMOTE_HELPER,
                "--latex-helper",
            ]);
        } else {
            command.args([
                "--ro-bind",
                env!("CARGO_BIN_EXE_rode"),
                "/rode-latex-helper",
                "--",
                "/rode-latex-helper",
                "--latex-helper",
            ]);
        }
        let result = latex::execute(command, &invocation, &AtomicBool::new(false)).unwrap();
        assert!(result.pdf.is_some());
    }
}

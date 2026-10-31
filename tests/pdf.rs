//! Desktop Poppler page rendering, source/budget lifetime and bounded native/Python PDF transfer.

use base64::{Engine, engine::general_purpose::STANDARD};
use rode::{
    files::{self, Request},
    images::{self, Budget},
    pdf, sandbox,
};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
    sync::{Arc, atomic::AtomicBool},
};
const FIXTURE: &[u8] = include_bytes!("fixtures/pages.pdf");
fn peer(request: &Request) -> anyhow::Result<files::Response> {
    let mut child = Command::new("python3")
        .args(["-c", files::REMOTE_HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(request)?)?;
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
fn available() -> bool {
    if sandbox::find_program("pdfinfo").is_none() || sandbox::find_program("pdftoppm").is_none() {
        eprintln!("Install poppler-utils to test actual PDF rendering.");
        return false;
    }
    true
}
#[test]
fn pdf_pages_render_with_distinct_content_and_release_the_shared_budget() {
    if !available() {
        return;
    }
    let budget = Arc::new(Budget::default());
    let loaded = pdf::load_bytes(FIXTURE.to_vec(), 1, &budget, 1024).unwrap();
    assert_eq!(loaded.source.pages, 2);
    assert_eq!(loaded.page, 1);
    assert_eq!(loaded.pixels.pixels.size, [820, 1024]);
    let center = loaded.pixels.pixels.pixels[512 * 820 + 410];
    assert!(center.r() > 200 && center.b() < 80);
    let second = loaded
        .source
        .render(2, &budget, 1024, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(second.pixels.size, [1024, 820]);
    let center = second.pixels.pixels[410 * 1024 + 512];
    assert!(center.b() > 200 && center.r() < 80);
    assert_eq!(budget.used(), FIXTURE.len() + 2 * 820 * 1024 * 4);
    assert!(
        loaded
            .source
            .render(0, &budget, 1024, &AtomicBool::new(false))
            .is_err()
    );
    assert!(
        loaded
            .source
            .render(3, &budget, 1024, &AtomicBool::new(false))
            .is_err()
    );
    assert!(
        loaded
            .source
            .render(2, &budget, 1024, &AtomicBool::new(true))
            .is_err()
    );
    drop(second);
    drop(loaded);
    assert_eq!(budget.used(), 0);
    let last = pdf::load_bytes(FIXTURE.to_vec(), 999, &budget, 256).unwrap();
    assert_eq!(last.page, 2);
}
#[test]
fn binary_pdf_transports_match_and_reject_oversized_or_nonregular_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("quoted ' PDF 🦀.PDF");
    fs::write(&path, FIXTURE).unwrap();
    let request = Request::ReadBinary {
        path: path.to_string_lossy().into(),
    };
    for response in [files::local(&request).unwrap(), peer(&request).unwrap()] {
        assert_eq!(response.path, path.to_string_lossy());
        assert_eq!(STANDARD.decode(response.binary.unwrap()).unwrap(), FIXTURE);
        assert_eq!(response.revision, files::digest(FIXTURE));
        assert!(response.pdf.is_none());
    }
    fs::File::create(&path)
        .unwrap()
        .set_len(images::MAX_ENCODED as u64 + 1)
        .unwrap();
    assert!(files::local(&request).is_err());
    assert!(peer(&request).is_err());
    let dir = Request::ReadBinary {
        path: root.path().to_string_lossy().into(),
    };
    assert!(files::local(&dir).is_err());
    assert!(peer(&dir).is_err());
    assert!(pdf::is_path("report.PDF"));
    assert!(!pdf::is_path("report.pdf.rs"));
}
#[test]
fn invalid_pdf_data_does_not_retain_resources() {
    let budget = Arc::new(Budget::default());
    for bytes in [
        b"not a PDF".to_vec(),
        b"%PDF-1.4\ncorrupt\n".to_vec(),
        vec![0; images::MAX_ENCODED + 1],
    ] {
        assert!(pdf::load_bytes(bytes, 1, &budget, 512).is_err());
        assert_eq!(budget.used(), 0);
    }
    assert!(pdf::load("invalid base64", 1, &budget, 512).is_err());
    assert!(
        pdf::load_cancellable(
            &STANDARD.encode(FIXTURE),
            1,
            &budget,
            512,
            &AtomicBool::new(true)
        )
        .unwrap_err()
        .to_string()
        .contains("cancelled")
    );
}
#[test]
fn sandbox_pdf_reads_cannot_escape_through_paths_or_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let outside = root.path().join("private.pdf");
    fs::write(&outside, FIXTURE).unwrap();
    fs::write(project.join("inside.pdf"), FIXTURE).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("escape.pdf")).unwrap();
    let plan = sandbox::prepare(
        project.to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let read = |path: &std::path::Path| {
        plan.local_request_with_helper(
            &Request::ReadBinary {
                path: path.to_string_lossy().into(),
            },
            std::path::Path::new(env!("CARGO_BIN_EXE_rode")),
        )
    };
    assert_eq!(
        STANDARD
            .decode(read(&project.join("inside.pdf")).unwrap().binary.unwrap())
            .unwrap(),
        FIXTURE
    );
    if plan.active {
        assert!(read(&outside).is_err());
        assert!(read(&project.join("escape.pdf")).is_err());
    } else {
        eprintln!("Sandbox unavailable: {}", plan.reason);
    }
}

#[test]
fn document_attachments_extract_pdf_text_and_reject_cancelled_conversion() {
    if sandbox::find_program("pdftotext").is_none() {
        return;
    }
    let text = pdf::extract_text(FIXTURE.to_vec(), &AtomicBool::new(false)).unwrap();
    assert!(!text.trim().is_empty());
    assert!(!text.contains("%PDF-"));
    assert!(pdf::extract_text(FIXTURE.to_vec(), &AtomicBool::new(true)).is_err());
    assert!(pdf::extract_text(b"not a PDF".to_vec(), &AtomicBool::new(false)).is_err());
}

//! Bundled Rust page rendering, source/budget lifetime and bounded desktop/headless PDF transfer.

use rode::sandbox::PlanExt as _;
#[path = "support/native.rs"]
mod native_peer;

use base64::{Engine, engine::general_purpose::STANDARD};
use rode::{
    files::{self, Request},
    images::Budget,
    pdf, sandbox,
};
use std::{
    fs,
    io::Write,
    process::Stdio,
    sync::{Arc, atomic::AtomicBool},
};
const FIXTURE: &[u8] = include_bytes!("fixtures/pages.pdf");
const PPTX: &[u8] = include_bytes!("fixtures/slides.pptx");
const DOCX: &[u8] = include_bytes!("fixtures/word.docx");

#[test]
fn word_pages_render_text_tables_images_and_related_stories_without_changing_source() {
    let budget = Arc::new(Budget::default());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("quoted ' document.DOCX");
    fs::write(&path, DOCX).unwrap();
    let reply = files::local(&Request::ReadBinary {
        path: path.to_string_lossy().into(),
    })
    .unwrap();
    let loaded = pdf::load_format_cancellable(
        &reply.binary.unwrap(),
        pdf::Format::Docx,
        1,
        &budget,
        768,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(loaded.source.format, pdf::Format::Docx);
    assert_eq!(loaded.source.pages, 3);
    assert_eq!(loaded.source.sizes, vec![[612., 792.]; 3]);
    let pixels = &loaded.pixels.pixels.pixels;
    assert!(
        pixels.iter().filter(|p| p.b() > 150 && p.r() < 100).count() > 100,
        "Blue heading must render"
    );
    assert!(
        pixels
            .iter()
            .filter(|p| p.g() > 150 && p.r() < 100 && p.b() < 120)
            .count()
            > 100,
        "Embedded image must render"
    );
    let second = loaded
        .source
        .render(2, &budget, 768, &AtomicBool::new(false))
        .unwrap();
    assert!(
        second
            .pixels
            .pixels
            .iter()
            .filter(|p| p.r() > 150 && p.b() < 100)
            .count()
            > 100,
        "Second page must contain its red heading"
    );
    let third = loaded
        .source
        .render(3, &budget, 768, &AtomicBool::new(false))
        .unwrap();
    assert!(
        third
            .pixels
            .pixels
            .iter()
            .filter(|p| p.g() > 150 && p.r() < 100 && p.b() < 120)
            .count()
            > 100,
        "Third page must contain its green heading"
    );
    let cached = loaded
        .source
        .render(1, &budget, 768, &AtomicBool::new(false))
        .unwrap();
    assert!(Arc::ptr_eq(&loaded.pixels.pixels, &cached.pixels));
    let snapshot = rdocx::Document::from_bytes(DOCX)
        .unwrap()
        .to_pdf_deterministic()
        .unwrap();
    // Text runs may be separate PDF operators. Normalize their whitespace
    // while still checking that body, table, header/footer and note text survived.
    let text = pdf::extract_text(snapshot, &AtomicBool::new(false)).unwrap();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for expected in [
        "RODE header",
        "RODE footer",
        "Word footnote content",
        "Native rendering",
        "Final Word page",
    ] {
        assert!(
            text.contains(expected),
            "Missing Word content {expected:?}: {text}"
        );
    }
    assert_eq!(fs::read(path).unwrap(), DOCX);
    drop((cached, second, third, loaded));
    assert_eq!(budget.used(), 0);
}

#[test]
fn word_previews_reject_other_formats_and_cancelled_loads_without_leaking_leases() {
    let budget = Arc::new(Budget::default());
    for bytes in [b"not DOCX".as_slice(), FIXTURE, PPTX] {
        assert!(
            pdf::load_format_cancellable(
                &STANDARD.encode(bytes),
                pdf::Format::Docx,
                1,
                &budget,
                256,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert_eq!(budget.used(), 0);
    }
    assert!(
        pdf::load_format_cancellable(
            &STANDARD.encode(DOCX),
            pdf::Format::Docx,
            1,
            &budget,
            256,
            &AtomicBool::new(true)
        )
        .is_err()
    );
    assert_eq!(budget.used(), 0);
    assert_eq!(
        pdf::Format::for_path("remote/document.DOCX"),
        Some(pdf::Format::Docx)
    );
    assert_eq!(pdf::Format::for_path("document.doc"), None);
}

#[test]
fn custom_powerpoint_page_sizes_fit_the_admitted_raster_at_every_resolution() {
    // The width is 787.795 points: the engine snaps the page box up to 788.
    // Scaling from the raw EMU size used to exceed the admitted raster edge.
    let encoded = STANDARD.encode(include_bytes!("fixtures/custom-slides.pptx"));
    let budget = Arc::new(Budget::default());
    for side in [128, 512, 2048] {
        let loaded = pdf::load_format_cancellable(
            &encoded,
            pdf::Format::Pptx,
            1,
            &budget,
            side,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(loaded.source.sizes[0], [788., 405.]);
        assert_eq!(loaded.pixels.pixels.size[0], side);
        assert!(loaded.pixels.pixels.size[1] > 0 && loaded.pixels.pixels.size[1] <= side);
        drop(loaded);
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn native_powerpoint_slides_preserve_source_cache_pixels_and_report_missing_images() {
    let budget = Arc::new(Budget::default());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("quoted ' deck.PPTX");
    fs::write(&path, PPTX).unwrap();
    let reply = files::local(&Request::ReadBinary {
        path: path.to_string_lossy().into(),
    })
    .unwrap();
    let loaded = pdf::load_format_cancellable(
        &reply.binary.unwrap(),
        pdf::Format::Pptx,
        1,
        &budget,
        512,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(loaded.source.pages, 3);
    assert_eq!(loaded.source.format, pdf::Format::Pptx);
    assert_eq!(loaded.pixels.pixels.size, [512, 288]);
    let first = &loaded.pixels.pixels;
    let center = first.pixels[144 * 512 + 256];
    assert!(center.r() > 180 && center.b() < 90);
    assert!(
        first
            .pixels
            .iter()
            .filter(|p| p.r() > 230 && p.g() > 230 && p.b() > 230)
            .count()
            > 100,
        "Title text must render with the bundled font"
    );
    let second = loaded
        .source
        .render(2, &budget, 512, &AtomicBool::new(false))
        .unwrap();
    let center = second.pixels.pixels[144 * 512 + 256];
    assert!(center.b() > 180 && center.r() < 90);
    let picture = second.pixels.pixels[200 * 512 + 400];
    assert!(
        picture.g() > 150 && picture.r() < 80,
        "Embedded images must be drawn"
    );
    assert_eq!(loaded.source.skipped_images(2), 0);
    let third = loaded
        .source
        .render(3, &budget, 512, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        loaded.source.skipped_images(3),
        1,
        "Missing image content must be reported"
    );
    let cached = loaded
        .source
        .render(1, &budget, 512, &AtomicBool::new(false))
        .unwrap();
    assert!(Arc::ptr_eq(&loaded.pixels.pixels, &cached.pixels));
    assert_eq!(
        fs::read(path).unwrap(),
        PPTX,
        "A preview must never save the source"
    );
    drop((cached, second, third, loaded));
    assert_eq!(budget.used(), 0);
}

#[test]
fn powerpoint_rejects_bad_packages_and_cancellation_without_retaining_resources() {
    let budget = Arc::new(Budget::default());
    for bytes in [
        b"not PPTX".as_slice(),
        b"PK\x03\x04bad zip".as_slice(),
        FIXTURE,
    ] {
        assert!(
            pdf::load_format_cancellable(
                &STANDARD.encode(bytes),
                pdf::Format::Pptx,
                1,
                &budget,
                256,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert_eq!(budget.used(), 0);
    }
    assert!(
        pdf::load_format_cancellable(
            &STANDARD.encode(PPTX),
            pdf::Format::Pptx,
            1,
            &budget,
            256,
            &AtomicBool::new(true)
        )
        .is_err()
    );
    assert_eq!(budget.used(), 0);
    assert_eq!(
        pdf::Format::for_path("remote/deck.PPTX"),
        Some(pdf::Format::Pptx)
    );
    assert_eq!(pdf::Format::for_path("deck.pptx.rs"), None);
}
fn peer(request: &Request) -> anyhow::Result<files::Response> {
    let mut child = native_peer::command()
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
#[test]
fn pdf_pages_render_with_distinct_content_and_release_the_shared_budget() {
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
fn binary_pdf_transports_match_without_a_source_size_cap_and_reject_nonregular_files() {
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
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    for response in [files::local(&request).unwrap(), peer(&request).unwrap()] {
        assert_eq!(
            STANDARD.decode(response.binary.unwrap()).unwrap().len(),
            32 * 1024 * 1024 + 1
        );
    }
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
        vec![0; 32 * 1024 * 1024 + 1],
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
    let plan = native_peer::prepare(
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
    let text = pdf::extract_text(FIXTURE.to_vec(), &AtomicBool::new(false)).unwrap();
    assert!(!text.trim().is_empty());
    assert!(!text.contains("%PDF-"));
    assert!(pdf::extract_text(FIXTURE.to_vec(), &AtomicBool::new(true)).is_err());
    assert!(pdf::extract_text(b"not a PDF".to_vec(), &AtomicBool::new(false)).is_err());
}

#[test]
fn renderer_survives_its_opening_thread_and_reuses_cached_pixels() {
    let budget = Arc::new(Budget::default());
    let worker_budget = budget.clone();
    let loaded = std::thread::spawn(move || {
        pdf::load_bytes(FIXTURE.to_vec(), 1, &worker_budget, 512).unwrap()
    })
    .join()
    .unwrap();
    let source = loaded.source.clone();
    let second = source
        .render(2, &budget, 512, &AtomicBool::new(false))
        .unwrap();
    let used = budget.used();
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        let back = source
            .render(1, &budget, 512, &AtomicBool::new(false))
            .unwrap();
        assert!(Arc::ptr_eq(&loaded.pixels.pixels, &back.pixels));
        let forward = source
            .render(2, &budget, 512, &AtomicBool::new(false))
            .unwrap();
        assert!(Arc::ptr_eq(&second.pixels, &forward.pixels));
    }
    eprintln!("2,000 cached page accesses: {:?}", start.elapsed());
    assert_eq!(budget.used(), used);
    drop((loaded, source, second));
    assert_eq!(budget.used(), 0);
}

#[test]
fn bundled_renderer_and_text_extraction_work_with_an_empty_path() {
    use std::io::Read;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("document.pdf");
    fs::write(&path, FIXTURE).unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_rode"))
        .args([std::ffi::OsStr::new("--pdf-helper"), path.as_os_str()])
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    let header = |output: &mut std::process::ChildStdout| {
        let mut length = [0; 4];
        output.read_exact(&mut length).unwrap();
        let length = u32::from_be_bytes(length) as usize;
        assert!(length <= 512 * 1024);
        let mut bytes = vec![0; length];
        output.read_exact(&mut bytes).unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
    };
    let document = header(&mut output);
    assert_eq!(document["kind"], "document");
    assert_eq!(document["sizes"].as_array().unwrap().len(), 2);
    input
        .write_all(b"{\"action\":\"render\",\"page\":2,\"side\":256}\n")
        .unwrap();
    let raster = header(&mut output);
    assert_eq!(raster["kind"], "page");
    let length = raster["width"].as_u64().unwrap() * raster["height"].as_u64().unwrap() * 4;
    assert!(length > 0 && length <= 256 * 256 * 4);
    let mut pixels = vec![0; length as usize];
    output.read_exact(&mut pixels).unwrap();
    input.write_all(b"{\"action\":\"text\"}\n").unwrap();
    let text = header(&mut output);
    assert_eq!(text["kind"], "text");
    let mut bytes = vec![0; text["bytes"].as_u64().unwrap() as usize];
    output.read_exact(&mut bytes).unwrap();
    assert!(String::from_utf8(bytes).unwrap().contains("RODE"));
    drop(input);
    drop(output);
    assert!(child.wait().unwrap().success());
}

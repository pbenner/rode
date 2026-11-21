//! Compare checkpoint reuse to fresh parser output, including edits that change
//! multiline scope and shift the checkpoint grid. Timing is never asserted.
use super::*;
use crate::theme::Themes;

fn settings() -> Settings {
    Themes::load(tempfile::tempdir().unwrap().path(), "studio-neutral")
        .0
        .source_settings()
}
fn fresh(text: &str, settings: &Settings, previous: Option<&Parsed>) -> Parsed {
    parse(text.into(), "rs", settings, previous, || false).unwrap()
}
fn equivalent(a: &Parsed, b: &Parsed) {
    assert_eq!(a.source, b.source);
    assert_eq!(a.ranges, b.ranges);
    for (a, b) in a.tokens.iter().zip(&b.tokens) {
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b.iter()) {
            assert_eq!(a.range, b.range);
            assert_eq!(a.style, b.style);
        }
    }
    assert_eq!(a.tokens.len(), b.tokens.len());
}
#[test]
fn checkpoint_edits_match_fresh_parse_across_multiline_and_shifted_suffixes() {
    let settings = settings();
    let original: String = (0..220)
        .map(|i| format!("let value_{i} = \"日本語\"; // line\r\n"))
        .collect();
    let old = fresh(&original, &settings, None);
    for changed in [
        original.replacen("let value_10", "/*\nlet value_10", 1),
        original.replacen("let value_10", "\n// inserted\nlet value_10", 1),
        original.replacen("let value_70 = \"日本語\"; // line\r\n", "", 1),
        format!("{original}\nlet last = true;"),
        String::new(),
    ] {
        let incremental = fresh(&changed, &settings, Some(&old));
        equivalent(&incremental, &fresh(&changed, &settings, None));
        equivalent(&fresh(&original, &settings, Some(&incremental)), &old);
    }
    let changed = original.replacen("value_10", "renamed_10", 1);
    let incremental = fresh(&changed, &settings, Some(&old));
    assert!(
        Arc::ptr_eq(&incremental.tokens[210], &old.tokens[210]),
        "Unchanged suffix should reuse parsed tokens after state convergence"
    );
    assert!(parse(original.into(), "rs", &settings, Some(&old), || true).is_none());
}

#[test]
fn worker_publishes_only_latest_source_and_palette_and_highlights_large_files() {
    let ctx = egui::Context::default();
    let mut themes = Themes::load(tempfile::tempdir().unwrap().path(), "studio-neutral").0;
    let mut view = Source::default();
    let large = "fn name() { let value = true; } // source\n".repeat(14_000);
    assert!(large.len() > 512 * 1024);
    view.job(&ctx, &large, "rs", 16., themes.source_settings());
    themes.select_syntax("dracula", &ctx).unwrap();
    let expected = format!("/* prefix */\n{large}");
    let settings = themes.source_settings();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let job = loop {
        let job = view.job(&ctx, &expected, "rs", 18., settings.clone());
        assert_eq!(job.text.as_ref(), expected);
        if view.settled() {
            break job;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Latest parse never completed"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert!(
        job.chunks
            .iter()
            .flat_map(|c| &c.sections)
            .any(|s| s.format.color != job.chunks[0].sections[0].format.color)
    );
    assert!(
        job.chunks
            .iter()
            .flat_map(|c| &c.sections)
            .all(|s| s.format.font_id.size == 18.)
    );
    assert_eq!(
        job.chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        expected
    );
    for chunk in &job.chunks {
        let mut cursor = 0;
        for span in &chunk.sections {
            assert_eq!(span.byte_range.start, cursor);
            cursor = span.byte_range.end;
        }
        assert_eq!(cursor, chunk.text.len());
    }
    let old_revision = view.revision;
    assert_eq!(view.job(&ctx, "", "txt", 16., settings).text.as_ref(), "");
    assert!(view.revision > old_revision);
    // Empty input may complete before job() polls the mailbox. Readiness is
    // deliberately not tied to thread scheduling; its source identity is exact.
    if view.settled() {
        assert!(view.parsed.as_ref().unwrap().source.is_empty());
    }
    let shared = view.worker.as_ref().unwrap().shared.clone();
    let revision = view.revision;
    drop(view);
    assert!(shared.revision.load(Ordering::Relaxed) > revision);
    let mailbox = shared.mailbox.lock().unwrap();
    assert!(mailbox.stopped);
    assert!(mailbox.request.is_none());
    assert!(mailbox.result.is_none());
}

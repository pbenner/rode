//! Exact diff semantics, real Git plumbing, stale reply admission and paint bounds.
use super::*;
use diff::{Kind::*, Marker};
use std::process::Command;

fn changes(old: &str, new: &str) -> Vec<(usize, usize, diff::Kind)> {
    diff::compare(old, new)
        .unwrap()
        .markers
        .into_iter()
        .map(|m| (m.line, m.count, m.kind))
        .collect()
}

#[test]
fn line_changes_preserve_unchanged_context_and_unequal_replacements() {
    assert!(changes("same\n", "same\n").is_empty());
    assert_eq!(
        changes("one\ntwo\nthree\n", "one\nchanged\nthree\nnew\n"),
        [(1, 1, Modified), (3, 1, Added)]
    );
    assert_eq!(
        changes("a\nb\nc\nd\n", "a\nreplacement\n"),
        [(1, 1, Modified), (2, 2, Deleted)]
    );
    assert_eq!(
        changes("a\nb\n", "a\nλ\nextra\n"),
        [(1, 1, Modified), (2, 1, Added)]
    );
}

#[test]
fn deletions_empty_buffers_crlf_and_final_newlines_have_exact_anchors() {
    assert_eq!(
        changes("first\nkeep\nlast\n", "keep\n"),
        [(0, 1, Deleted), (1, 1, Deleted)]
    );
    assert_eq!(changes("all\nremoved\n", ""), [(0, 2, Deleted)]);
    assert_eq!(changes("", "λ\r\n"), [(0, 1, Added)]);
    assert_eq!(changes("a\r\nb\r\n", "a\r\nc\r\n"), [(1, 1, Modified)]);
    assert_eq!(changes("a\n", "a"), [(0, 1, Modified)]);
    assert!(changes("", "").is_empty());
    assert!(changes("a\nb\n", "a\r\nb\r\n").is_empty());
    assert!(diff::compare("binary\0", "text").is_none());
    assert!(diff::compare("", &"x".repeat(MAX_BYTES + 1)).is_none());
    assert!(diff::compare("", &"\n".repeat(MAX_LINES)).is_none());
}

#[test]
fn previews_keep_exact_old_new_ranges_and_shared_replacement_markers() {
    let changes = diff::compare("a\nλ old\nremoved\n", "a\nnew\n").unwrap();
    assert_eq!(changes.hunks.len(), 1);
    let hunk = &changes.hunks[0];
    assert_eq!(hunk.old, 1..3);
    assert_eq!(hunk.new, 1..2);
    assert_eq!(hunk.markers, 0..2);
    assert_eq!(hunk.patch, "@@ -2,2 +2,1 @@\n-λ old\n-removed\n+new\n");
    assert_eq!(changes.markers[0].kind, Modified);
    assert_eq!(changes.markers[1].kind, Deleted);

    let changes = diff::compare("first\nkeep\nlast\n", "keep\n").unwrap();
    assert_eq!(changes.hunks.len(), 2);
    assert_eq!(changes.hunks[0].patch, "@@ -1,1 +0,0 @@\n-first\n");
    assert_eq!(changes.hunks[1].patch, "@@ -3,1 +1,0 @@\n-last\n");
    assert_eq!(changes.hunks[0].markers, 0..1);
    assert_eq!(changes.hunks[1].markers, 1..2);
}

#[test]
fn previews_preserve_empty_new_unicode_and_missing_newline_changes() {
    assert_eq!(
        diff::compare("", "λ\r\n").unwrap().hunks[0].patch,
        "@@ -0,0 +1,1 @@\n+λ\n"
    );
    assert_eq!(
        diff::compare("all\nremoved\n", "").unwrap().hunks[0].patch,
        "@@ -1,2 +0,0 @@\n-all\n-removed\n"
    );
    assert_eq!(
        diff::compare("λ\n", "λ").unwrap().hunks[0].patch,
        "@@ -1,1 +1,1 @@\n-λ\n+λ\n\\ No newline at end of file\n"
    );
    assert_eq!(
        diff::compare("λ", "λ\n").unwrap().hunks[0].patch,
        "@@ -1,1 +1,1 @@\n-λ\n\\ No newline at end of file\n+λ\n"
    );
    assert!(diff::compare("a\r\n", "a\n").unwrap().hunks.is_empty());
}

#[test]
fn open_previews_survive_identical_refreshes_and_close_when_the_comparison_changes() {
    let mut documents = documents::Documents::default();
    documents.new_file();
    let doc = documents.active_mut().unwrap();
    doc.path = Some("/project/source.rs".into());
    let key = Key {
        id: doc.id,
        path: doc.path.clone().unwrap(),
    };
    let mut state = State::default();
    state.select(key.clone(), doc.version(), Instant::now());
    let comparison = diff::compare("old\n", "current\n").unwrap();
    state.rendered = Some((doc.version(), comparison.clone()));
    assert!(matches!(
        state.toggle_peek(doc, 0),
        Some(PeekAction::Navigate(1))
    ));
    assert_eq!(state.peek, Some(0));
    assert!(state.peek_height(doc, 500.) > 0.);
    assert_eq!(state.peek_height(doc, 50.), 0.);
    state.pending = Some(1);
    state.complete(Completed {
        token: 1,
        version: doc.version(),
        refreshed: true,
        baseline: None,
        changes: comparison.clone(),
    });
    assert_eq!(
        state.peek,
        Some(0),
        "Polling an unchanged HEAD must not dismiss the preview"
    );
    assert!(matches!(state.toggle_peek(doc, 0), Some(PeekAction::Close)));
    assert_eq!(state.peek, None);
    let _ = state.toggle_peek(doc, 0);
    state.pending = Some(2);
    state.complete(Completed {
        token: 1,
        version: doc.version(),
        refreshed: true,
        baseline: None,
        changes: Changes::default(),
    });
    assert_eq!(
        state.peek,
        Some(0),
        "A stale worker cannot dismiss current content"
    );
    state.complete(Completed {
        token: 2,
        version: doc.version(),
        refreshed: true,
        baseline: None,
        changes: Changes::default(),
    });
    assert_eq!(state.peek, None);
    state.rendered = Some((doc.version(), comparison));
    let _ = state.toggle_peek(doc, 0);
    doc.advance_version_fixture();
    assert_eq!(state.peek_height(doc, 500.), 0.);
    state.select(key, doc.version(), Instant::now());
    assert_eq!(state.peek, None);
    state.rendered.as_mut().unwrap().0 = doc.version();
    let _ = state.toggle_peek(doc, 0);
    doc.path = None;
    assert_eq!(state.peek_height(doc, 500.), 0.);
    state.reset();
    assert_eq!(state.peek, None);
}

pub(super) struct Repo {
    directory: tempfile::TempDir,
}
impl Repo {
    pub(super) fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let repo = Self { directory };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Gutter fixture"]);
        repo.git(&["config", "user.email", "fixture@example.invalid"]);
        repo
    }
    pub(super) fn root(&self) -> &str {
        self.directory.path().to_str().unwrap()
    }
    pub(super) fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.root())
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn baseline(&self, name: &str) -> Option<Arc<str>> {
        self.snapshot(name, None).0.map(|baseline| baseline.text)
    }
    pub(super) fn snapshot(
        &self,
        name: &str,
        previous: Option<&repository::Baseline>,
    ) -> (Option<repository::Baseline>, usize) {
        let path = self.directory.path().join(name);
        let mut calls = 0;
        let result = repository::baseline(
            self.root(),
            path.to_str().unwrap(),
            previous,
            |root, args| {
                calls += 1;
                // Exercise the same host service used by the local background worker.
                match crate::project::local(&crate::project::Request::Git {
                    root: root.into(),
                    args,
                    input: None,
                })
                .ok()?
                {
                    crate::project::Response::Command(output) => Some(output),
                    _ => None,
                }
            },
        );
        (result, calls)
    }
    pub(super) fn write(&self, name: &str, text: &str) {
        let path = self.directory.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

#[test]
fn head_baselines_include_staged_unsaved_and_untracked_changes_and_follow_commits() {
    let repo = Repo::new();
    repo.write("source.rs", "original\n");
    assert_eq!(repo.baseline("source.rs").as_deref(), Some("")); // unborn HEAD
    repo.git(&["add", "--", "source.rs"]);
    assert_eq!(repo.baseline("source.rs").as_deref(), Some(""));
    repo.git(&["commit", "-qm", "Initial"]);
    repo.write("source.rs", "staged\n");
    repo.git(&["add", "--", "source.rs"]);
    assert_eq!(repo.baseline("source.rs").as_deref(), Some("original\n"));
    assert_eq!(
        changes(&repo.baseline("source.rs").unwrap(), "unsaved\n"),
        [(0, 1, Modified)]
    );
    repo.git(&["commit", "-qm", "Changed"]);
    assert_eq!(repo.baseline("source.rs").as_deref(), Some("staged\n"));
    repo.write("new.rs", "untracked\n");
    assert_eq!(repo.baseline("new.rs").as_deref(), Some(""));
    repo.write(".gitignore", "ignored.rs\n");
    repo.write("ignored.rs", "secret\n");
    assert!(repo.baseline("ignored.rs").is_none());
    assert!(repo.baseline("missing.rs").is_none());
    repo.git(&["rm", "-q", "source.rs"]);
    assert_eq!(repo.baseline("source.rs").as_deref(), Some("staged\n"));
}

#[test]
fn literal_paths_nested_repositories_and_non_text_entries_are_safe() {
    let repo = Repo::new();
    for name in ["sub/-option ' λ.rs", "tab\tname\n.rs", "binary.rs"] {
        repo.write(
            name,
            if name == "binary.rs" {
                "a\0b"
            } else {
                "original\r\n"
            },
        );
    }
    std::os::unix::fs::symlink("binary.rs", repo.directory.path().join("link.rs")).unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "Unusual paths"]);
    for name in ["sub/-option ' λ.rs", "tab\tname\n.rs"] {
        assert_eq!(repo.baseline(name).as_deref(), Some("original\r\n"));
    }
    assert!(repo.baseline("link.rs").is_none());
    assert!(repo.baseline("binary.rs").is_none());
    let nested = repo.directory.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    repo.git(&["-C", nested.to_str().unwrap(), "init", "-q"]);
    repo.write("nested/new.txt", "nested\n");
    assert_eq!(repo.baseline("nested/new.txt").as_deref(), Some(""));
    assert!(repository::location(repo.root(), "/outside.rs").is_none());
    assert!(repository::location(repo.root(), &format!("{}/../outside.rs", repo.root())).is_none());
    assert!(repository::location("/project", "/project-other/file").is_none());
    assert!(repository::location("/project", "relative/file").is_none());
    let mut calls = 0;
    assert!(
        repository::baseline(
            repo.root(),
            &format!("{}/source.rs", repo.root()),
            None,
            |_, _| {
                calls += 1;
                None // denied/missing Git never becomes a whole-file addition
            }
        )
        .is_none()
    );
    assert_eq!(calls, 1);
}

#[test]
fn debounce_reply_admission_and_resets_preserve_the_single_worker_bound() {
    let mut state = State::default();
    let key = Key {
        id: 1,
        path: "/project/file.rs".into(),
    };
    let now = Instant::now();
    state.select(key.clone(), 0, now);
    assert_eq!(state.due(now), Some(true));
    state.token += 1;
    state.pending = Some(state.token);
    let token = state.token;
    state.busy.store(true, Ordering::Release);
    let running = Running(state.busy.clone());
    state.select(key.clone(), 1, now);
    state.complete(Completed {
        token,
        version: 0,
        refreshed: true,
        baseline: None,
        changes: Changes::default(),
    });
    assert_eq!(state.rendered.as_ref().unwrap().0, 0); // paint rejects version 1
    assert_eq!(state.due(now + DEBOUNCE), None); // worker still owns its slot
    drop(running);
    assert_eq!(state.due(now), None);
    assert_eq!(state.due(now + DEBOUNCE), Some(false));
    state.busy.store(true, Ordering::Release);
    let running = Running(state.busy.clone());
    state.reset();
    state.select(Key { id: 2, ..key }, 0, now);
    state.complete(Completed {
        token,
        version: 0,
        refreshed: true,
        baseline: None,
        changes: Changes::default(),
    });
    assert!(state.baseline.is_none());
    assert_eq!(state.due(now), None);
    drop(running);
    assert_eq!(state.due(now), Some(true));
}

#[test]
fn markers_stay_in_their_own_strip_and_deletions_remain_visible_at_both_ends() {
    let ctx = egui::Context::default();
    let palette = crate::theme::Theme::builtins()[0].palette;
    for scale in [1., 1.5, 2.] {
        ctx.set_pixels_per_point(scale);
        let mut gutter = egui::Rect::NOTHING;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                gutter = ui
                    .allocate_exact_size(egui::vec2(72., 60.), Sense::hover())
                    .0;
                let mut job = egui::text::LayoutJob::simple(
                    "a\nb\n".into(),
                    FontId::monospace(14.),
                    palette.text,
                    f32::INFINITY,
                );
                job.sections[0].format.line_height = Some(20.);
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                let rows = editor_view::gutter::Rows::new(&galley, gutter.left_top());
                let _ = diff::paint(
                    ui,
                    &[
                        Marker {
                            line: 0,
                            count: 1,
                            kind: Deleted,
                        },
                        Marker {
                            line: 1,
                            count: 1,
                            kind: Modified,
                        },
                        Marker {
                            line: 2,
                            count: 1,
                            kind: Added,
                        },
                        Marker {
                            line: 3,
                            count: 4,
                            kind: Deleted,
                        },
                    ],
                    1,
                    gutter,
                    rows,
                    palette,
                );
            });
        });
        let markers: Vec<_> = output
            .shapes
            .iter()
            .filter(|s| match &s.shape {
                egui::Shape::Rect(r) => {
                    r.fill == palette.success || r.fill == egui::Color32::from_rgb(60, 145, 235)
                }
                egui::Shape::Path(p) => p.fill == palette.error,
                _ => false,
            })
            .collect();
        assert_eq!(markers.len(), 4);
        for marker in markers {
            let bounds = marker.shape.visual_bounding_rect();
            assert!(gutter.contains_rect(bounds), "{scale}: {bounds:?}");
            assert!(bounds.left() >= gutter.right() - 12.); // clear of numbers/diagnostics
            assert!(bounds.right() < gutter.right() - 4.); // clear of separator
        }
    }
}

#[test]
fn unchanged_heads_reuse_blob_text_without_hiding_nested_repository_changes() {
    let repo = Repo::new();
    repo.write("source.txt", "root version\n");
    repo.write("sub/source.txt", "sub version\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "Both paths"]);
    let (first, calls) = repo.snapshot("sub/source.txt", None);
    assert_eq!(calls, 4);
    let first = first.unwrap();
    assert_eq!(&*first.text, "sub version\n");
    let (cached, calls) = repo.snapshot("sub/source.txt", Some(&first));
    assert_eq!(calls, 1);
    assert!(Arc::ptr_eq(&cached.unwrap().text, &first.text));
    // A .git file can turn the existing subdirectory into another checkout of
    // exactly the same HEAD. Its relative source path changes from sub/source.txt
    // to source.txt, so reusing just the commit ID would decorate the wrong text.
    repo.write(
        "sub/.git",
        &format!("gitdir: {}\n", repo.directory.path().join(".git").display()),
    );
    let (nested, calls) = repo.snapshot("sub/source.txt", Some(&first));
    assert_eq!(calls, 4);
    assert_eq!(&*nested.unwrap().text, "root version\n");
}

#[test]
fn painting_rejects_old_versions_other_paths_and_unbound_recovery_documents() {
    let ctx = egui::Context::default();
    let palette = crate::theme::Theme::builtins()[0].palette;
    let mut documents = documents::Documents::default();
    documents.new_file();
    let doc = documents.active_mut().unwrap();
    let path = "/project/source.rs".to_owned();
    doc.path = Some(path.clone());
    let mut state = State::default();
    state.select(
        Key {
            id: doc.id,
            path: path.clone(),
        },
        0,
        Instant::now(),
    );
    state.rendered = Some((
        0,
        Changes {
            markers: vec![Marker {
                line: 0,
                count: 1,
                kind: Added,
            }],
            ..Default::default()
        },
    ));
    let paint_count = |doc: &Document| {
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let gutter = ui
                    .allocate_exact_size(egui::vec2(72., 20.), Sense::hover())
                    .0;
                let mut job = egui::text::LayoutJob::simple(
                    "x".into(),
                    FontId::monospace(14.),
                    palette.text,
                    f32::INFINITY,
                );
                job.sections[0].format.line_height = Some(20.);
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                let _ = state.paint(
                    ui,
                    doc,
                    gutter,
                    editor_view::gutter::Rows::new(&galley, gutter.left_top()),
                    palette,
                );
            });
        });
        output
            .shapes
            .iter()
            .filter(|s| {
                matches!(&s.shape,
            egui::Shape::Rect(rect) if rect.fill == palette.success)
            })
            .count()
    };
    assert_eq!(paint_count(doc), 1);
    doc.set_version_fixture(1);
    assert_eq!(paint_count(doc), 0);
    doc.set_version_fixture(0);
    doc.path = Some("/project/renamed.rs".into());
    assert_eq!(paint_count(doc), 0);
    doc.path = None;
    assert_eq!(paint_count(doc), 0);
    doc.path = Some(path);
    doc.id += 1;
    assert_eq!(paint_count(doc), 0);
}

#[test]
fn clean_crlf_checkouts_do_not_show_every_line_as_modified() {
    let repo = Repo::new();
    repo.git(&["config", "core.autocrlf", "input"]);
    repo.write("source.txt", "one\r\ntwo\r\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "Normalize text"]);
    let baseline = repo.baseline("source.txt").unwrap();
    assert_eq!(&*baseline, "one\ntwo\n");
    assert!(changes(&baseline, "one\r\ntwo\r\n").is_empty());
    assert_eq!(changes(&baseline, "one\r\nchanged\r\n"), [(1, 1, Modified)]);
}

#[test]
fn editor_markers_align_with_rendered_rows_throughout_long_files() {
    for scale in [0.75, 1., 1.25, 1.5, 2.] {
        for font_size in [14., 15., 19., 26.] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.set_ui_scale(scale, &ctx);
            ctx.style_mut(|style| {
                style.scroll_animation.duration = egui::emath::Rangef::new(0., 0.)
            });
            app.font_size = font_size;
            app.new_file();
            let source: String = (0..240)
                .map(|n| {
                    format!(
                        "let line_{n} = {n}; // {}\n",
                        if n % 11 == 0 { "λ🧪" } else { "λ" }
                    )
                })
                .collect();
            let doc = app.documents.active_mut().unwrap();
            doc.text = source.clone();
            doc.source_changed();
            doc.path = Some("/project/source.rs".into());
            app.git_gutter.key = Some(Key {
                id: doc.id,
                path: doc.path.clone().unwrap(),
            });
            app.git_gutter.rendered = Some((
                doc.version(),
                Changes {
                    markers: [0, 64, 199]
                        .into_iter()
                        .map(|line| Marker {
                            line,
                            count: 1,
                            kind: Modified,
                        })
                        .collect(),
                    ..Default::default()
                },
            ));
            for scrolled in [false, true] {
                let height = if scrolled { 480. } else { 12000. };
                if scrolled {
                    let at = source[..source.find("let line_199 ").unwrap()]
                        .chars()
                        .count();
                    app.documents.active_mut().unwrap().selection = Some((at, at));
                }
                let frame = |app: &mut EditorApp| {
                    ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(1200., height),
                            )),
                            ..Default::default()
                        },
                        |ctx| app.editor(ctx),
                    )
                };
                // A requested caret jump changes the scroll offset for the next pass.
                frame(&mut app);
                frame(&mut app);
                let output = frame(&mut app);
                let (text, clip) = output
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Text(text) if text.galley.text() == source => {
                            Some((text, s.clip_rect))
                        }
                        _ => None,
                    })
                    .expect("Missing source text");
                if scrolled {
                    assert!(text.pos.y < clip.top(), "Source did not scroll");
                }
                let markers: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == egui::Color32::from_rgb(60, 145, 235) =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .collect();
                let expected: Vec<_> = [0, 64, 199]
                    .into_iter()
                    .filter_map(|line| {
                        let row = text.galley.rows[line].rect().translate(text.pos.to_vec2());
                        (row.bottom() > clip.top() && row.top() < clip.bottom())
                            .then_some((line, row))
                    })
                    .collect();
                assert!(
                    !expected.is_empty(),
                    "scale={scale}, font={font_size}, scrolled={scrolled}, text origin={:?}, clip={clip:?}",
                    text.pos
                );
                assert_eq!(markers.len(), expected.len());
                for ((line, row), marker) in expected.into_iter().zip(markers) {
                    let tolerance = 0.51 / scale;
                    assert!(
                        (marker.top() - row.top().max(clip.top())).abs() <= tolerance,
                        "scale={scale}, font={font_size}, scrolled={scrolled}, line={}: marker {marker:?}, row {row:?}",
                        line + 1
                    );
                    assert!(
                        (marker.bottom() - row.bottom().min(clip.bottom())).abs() <= tolerance,
                        "scale={scale}, font={font_size}, scrolled={scrolled}, line={}: marker {marker:?}, row {row:?}",
                        line + 1
                    );
                    let number = output
                        .shapes
                        .iter()
                        .find_map(|s| match &s.shape {
                            egui::Shape::Text(number)
                                if number.galley.text() == (line + 1).to_string() =>
                            {
                                Some(number)
                            }
                            _ => None,
                        })
                        .expect("Missing line number");
                    let center = number
                        .galley
                        .rect
                        .translate(number.pos.to_vec2())
                        .center()
                        .y;
                    assert!(
                        (center - row.center().y).abs() <= tolerance,
                        "Line number drift at {}",
                        line + 1
                    );
                }
            }
        }
    }
}

#[test]
fn empty_source_keeps_line_one_and_its_deletion_marker_visible() {
    for font_size in [14., 26.] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.font_size = font_size;
        app.new_file();
        let doc = app.documents.active_mut().unwrap();
        doc.path = Some("/project/empty.rs".into());
        app.git_gutter.key = Some(Key {
            id: doc.id,
            path: doc.path.clone().unwrap(),
        });
        app.git_gutter.rendered = Some((
            doc.version(),
            Changes {
                markers: vec![Marker {
                    line: 0,
                    count: 3,
                    kind: Deleted,
                }],
                ..Default::default()
            },
        ));
        let frame = |app: &mut EditorApp| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200., 600.),
                    )),
                    ..Default::default()
                },
                |ctx| app.editor(ctx),
            )
        };
        frame(&mut app);
        let output = frame(&mut app);
        let palette = app.themes.active().palette;
        let deletion = output
            .shapes
            .iter()
            .find(|s| {
                matches!(&s.shape,
            egui::Shape::Path(path) if path.fill == palette.error)
            })
            .expect("Empty-file deletion marker disappeared");
        let number = output
            .shapes
            .iter()
            .find(|s| {
                matches!(&s.shape,
            egui::Shape::Text(text) if text.galley.text() == "1")
            })
            .expect("Empty-file line number disappeared");
        assert!(
            deletion
                .clip_rect
                .contains_rect(deletion.shape.visual_bounding_rect())
        );
        assert!(
            number
                .clip_rect
                .contains_rect(number.shape.visual_bounding_rect())
        );
    }
}

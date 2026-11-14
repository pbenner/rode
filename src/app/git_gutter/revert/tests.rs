//! Restoration semantics, stale snapshot rejection and native undo/redo integration.
use super::*;
use crate::app::git_gutter::tests::Repo;

#[test]
fn restores_only_the_selected_hunk_with_unicode_and_unequal_replacements() {
    for (old, new, index, expected) in [
        (
            "α\nold\nkeep\nlast\n",
            "α\nnew\nkeep\nextra\n",
            0,
            "α\nold\nkeep\nextra\n",
        ),
        (
            "α\nold\nkeep\nlast\n",
            "α\nnew\nkeep\nextra\n",
            1,
            "α\nnew\nkeep\nlast\n",
        ),
        ("a\nb\nc\nd\n", "a\nλ🧪\n", 0, "a\nb\nc\nd\n"),
        ("a\nb\n", "a\nλ\nextra\n", 0, "a\nb\n"),
        ("keep\n", "new\nkeep\n", 0, "keep\n"),
        ("first\nkeep\nlast\n", "keep\n", 0, "first\nkeep\n"),
        ("first\nkeep\nlast\n", "keep\n", 1, "keep\nlast\n"),
        ("removed\n", "", 0, "removed\n"),
        ("", "untracked\n", 0, ""),
        ("λ\n", "λ", 0, "λ\n"),
        ("λ", "λ\n", 0, "λ"),
    ] {
        let changes = diff::compare(old, new).unwrap();
        let (range, text) = replacement(old, new, &changes.hunks[index]);
        let mut actual = new.to_owned();
        actual.replace_range(range, &text);
        assert_eq!(actual, expected, "old={old:?}, new={new:?}, index={index}");
    }
}

#[test]
fn restores_line_endings_without_rewriting_unchanged_context() {
    for (old, new, expected) in [
        (
            "one\nold\nend\n",
            "one\r\nnew\r\nend\r\n",
            "one\r\nold\r\nend\r\n",
        ),
        (
            "one\r\nold\r\nend\r\n",
            "one\nnew\nend\n",
            "one\nold\nend\n",
        ),
        ("α\nold\nend\n", "α\r\nnew\r\nend\n", "α\r\nold\r\nend\n"),
        ("old\r\n", "", "old\r\n"),
        ("one\nlast", "one\r\nchanged\r\n", "one\r\nlast"),
        ("one\n", "one", "one\n"),
    ] {
        let changes = diff::compare(old, new).unwrap();
        let (range, text) = replacement(old, new, &changes.hunks[0]);
        let mut actual = new.to_owned();
        actual.replace_range(range, &text);
        assert_eq!(actual, expected);
    }
}

fn editor(old: &str, current: &str) -> (Repo, EditorApp, egui::Context) {
    let repo = Repo::new();
    repo.write("source.rs", old);
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "Baseline"]);
    repo.write("source.rs", current);
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    let doc = app.documents.active_mut().unwrap();
    doc.fixture_content().text = current.into();
    doc.sync_source_fixture();
    doc.acknowledge_save(
        format!("{}/source.rs", repo.root()),
        current.into(),
        "revision".into(),
    );
    app.editor.git_gutter.select(
        Key {
            id: doc.id,
            path: doc.path.clone().unwrap(),
        },
        doc.version(),
        Instant::now(),
    );
    app.editor.git_gutter.baseline = repo.snapshot("source.rs", None).0;
    app.editor.git_gutter.rendered = Some((doc.version(), diff::compare(old, current).unwrap()));
    (repo, app, ctx)
}

fn frame(app: &mut EditorApp, ctx: &egui::Context, events: Vec<egui::Event>) {
    let _ = ctx.run(
        egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 700.),
            )),
            ..Default::default()
        },
        |ctx| {
            app.route_input(ctx);
            app.editor(ctx);
        },
    );
}

#[test]
fn revert_is_a_native_undo_step_and_preserves_disk_saved_baseline_and_other_hunks() {
    let before = "α\nnew\nkeep\nextra\n";
    let after = "α\nold\nkeep\nextra\n";
    let (repo, mut app, ctx) = editor("α\nold\nkeep\nlast\n", before);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let request = app
        .editor
        .git_gutter
        .revert_request(&app.documents[0], 0)
        .unwrap();
    let version = app.documents[0].version();
    app.revert_git_change(request, &ctx);
    assert_eq!(app.documents[0].text, after);
    assert_eq!(app.documents[0].saved_text, before);
    assert!(app.documents[0].dirty());
    assert_eq!(app.documents[0].version(), version + 1);
    assert_eq!(app.documents[0].line_count(), 5);
    assert_eq!(app.documents[0].cursor, (2, 1));
    assert_eq!(app.documents[0].selection, Some((2, 2)));
    assert_eq!(
        std::fs::read_to_string(format!("{}/source.rs", repo.root())).unwrap(),
        before
    );
    frame(&mut app, &ctx, vec![]);
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }],
    );
    assert_eq!(app.documents[0].text, before);
    assert!(!app.documents[0].dirty());
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::Y,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }],
    );
    assert_eq!(app.documents[0].text, after);
    assert!(app.documents[0].dirty());
}

#[test]
fn an_old_revert_cannot_modify_a_new_edit_path_document_or_baseline() {
    for invalidation in [
        "edit",
        "rename",
        "document",
        "baseline",
        "missing-baseline",
        "reset",
    ] {
        let (repo, mut app, ctx) = editor("old\n", "new\n");
        let request = app
            .editor
            .git_gutter
            .revert_request(&app.documents[0], 0)
            .unwrap();
        match invalidation {
            "edit" => {
                app.documents[0].fixture_content().text = "later edit\n".into();
                app.documents[0].advance_version_fixture();
            }
            "rename" => app.documents[0].fixture_content().path = Some("renamed.rs".into()),
            "document" => {
                app.new_file();
            }
            "baseline" => {
                repo.write("source.rs", "latest commit\n");
                repo.git(&["add", "."]);
                repo.git(&["commit", "-qm", "New baseline"]);
                app.editor.git_gutter.baseline = repo.snapshot("source.rs", None).0;
                app.editor.git_gutter.rendered = Some((
                    app.documents[0].version(),
                    diff::compare("latest commit\n", "new\n").unwrap(),
                ));
            }
            "missing-baseline" => app.editor.git_gutter.baseline = None,
            "reset" => app.editor.git_gutter.reset(),
            _ => unreachable!(),
        }
        let before = app.documents.active().unwrap().text.clone();
        app.revert_git_change(request, &ctx);
        assert_eq!(
            app.documents.active().unwrap().text,
            before,
            "{invalidation}"
        );
    }
}

//! Structural mutations preserve selected identities and pending-read ownership.
use super::*;

#[test]
fn refreshes_preserve_disabled_notebooks_and_skip_dirty_saving_and_unbound_documents() {
    use crate::extensions::Feature;
    let images = Arc::new(markdown::Images::default());
    let shutdowns = notebook_view::Shutdowns::default();
    let mut docs = Documents::default();
    let text = docs.open(
        Response {
            path: "source.txt".into(),
            revision: "text".into(),
            ..Default::default()
        },
        &images,
        &shutdowns,
    );
    let book = crate::notebook::Notebook::default();
    let notebook = docs.open(
        Response {
            path: "book.ipynb".into(),
            text: book.serialize().unwrap(),
            revision: "book".into(),
            notebook: Some(Ok(book)),
            ..Default::default()
        },
        &images,
        &shutdowns,
    );
    docs.new_file();

    let jobs = docs.refreshes(|feature| feature != Feature::Notebooks);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].0, text);
    assert!(
        matches!(&jobs[0].2, Refresh::Read(request) if matches!(request.as_ref(), Request::Read { path } if path == "source.txt"))
    );
    let jobs = docs.refreshes(|_| true);
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().any(|(id, _, refresh)| {
        *id == notebook
            && matches!(refresh, Refresh::Read(request)
            if matches!(request.as_ref(), Request::ReadAuto { .. }))
    }));

    docs.by_id_mut(text).unwrap().set_source("changed".into());
    docs.by_id_mut(notebook)
        .unwrap()
        .begin_text_save("book.ipynb", false)
        .unwrap();
    assert!(docs.refreshes(|_| true).is_empty());
    assert_eq!(
        docs.by_id(notebook).unwrap().feature(),
        Some(Feature::Notebooks)
    );
}

#[test]
fn reloads_change_representation_without_leaking_old_previews_or_losing_source() {
    use crate::extensions::Feature;
    use base64::{Engine, engine::general_purpose::STANDARD};

    let images = Arc::new(markdown::Images::default());
    let shutdowns = notebook_view::Shutdowns::default();
    let mut doc = Document::loaded(
        1,
        Response {
            path: "changing.md".into(),
            text: "# Original".into(),
            revision: "r1".into(),
            ..Default::default()
        },
        &images,
        &shutdowns,
    );
    let mut preview = markdown::Preview::new(images.clone());
    preview.showing = true;
    doc.source_views_mut().unwrap().markdown = Some(preview);
    assert!(doc.rendered_preview());

    let book = crate::notebook::Notebook::default();
    let source = book.serialize().unwrap();
    doc.reload(
        Response {
            text: source.clone(),
            revision: "r2".into(),
            notebook: Some(Ok(book)),
            ..Default::default()
        },
        &images,
        &shutdowns,
    );
    assert_eq!(doc.feature(), Some(Feature::Notebooks));
    assert!(doc.source_views_mut().is_none());
    assert!(doc.markdown().is_none());
    assert_eq!(doc.text, source);
    assert!(!doc.read_only());

    let budget = Arc::new(crate::images::Budget::default());
    let loaded = crate::binary::Loaded::decode(&STANDARD.encode([0, 1, 255]), &budget).unwrap();
    doc.reload(
        Response {
            hex: Some(loaded),
            revision: "r3".into(),
            ..Default::default()
        },
        &images,
        &shutdowns,
    );
    assert_eq!(doc.feature(), Some(Feature::Binary));
    assert!(doc.notebook().is_none());
    assert!(!doc.source_visible());
    assert_eq!(doc.hex().unwrap().buffer.bytes().as_slice(), &[0, 1, 255]);

    doc.reload(
        Response {
            text: "Plain text again".into(),
            revision: "r4".into(),
            ..Default::default()
        },
        &images,
        &shutdowns,
    );
    assert!(doc.feature().is_none());
    assert!(doc.hex().is_none());
    assert!(doc.source_visible());
    assert_eq!(doc.text, "Plain text again");
    assert_eq!(doc.saved_text, doc.text);
    assert!(!doc.dirty());
}

#[test]
fn feature_changes_cancel_source_previews_without_discarding_edits_or_pending_saves() {
    use crate::extensions::Feature;

    let mut docs = Documents::default();
    docs.new_file();
    let doc = docs.active_mut().unwrap();
    doc.set_source("# Unsaved".into());
    doc.begin_text_save("draft.md", true).unwrap();
    let mut preview = markdown::Preview::new(Arc::default());
    preview.showing = true;
    let cancel = preview.cancelled();
    doc.source_views_mut().unwrap().markdown = Some(preview);
    let version = doc.version();

    docs.disable_feature(Feature::Markdown.id());
    let doc = docs.active().unwrap();
    assert!(cancel.load(Ordering::Acquire));
    assert!(doc.source_visible());
    assert!(doc.dirty());
    assert!(doc.saving);
    assert_eq!(doc.text, "# Unsaved");
    assert_eq!(doc.version(), version);
}

#[test]
fn source_changes_keep_revisions_dirty_state_and_line_counts_consistent() {
    let mut docs = Documents::default();
    docs.new_file();
    let doc = docs.active_mut().unwrap();
    let original = "λ\r\nsecond\r\n";
    doc.fixture_content().text = original.into();
    doc.acknowledge_save("source.rs".into(), original.into(), "saved".into());
    doc.fixture_content().text = "λ\r\nchanged\r\nextra".into();
    doc.source_changed();
    assert_eq!(doc.version(), 1);
    assert_eq!(doc.line_count(), 3);
    assert!(doc.dirty());
    doc.fixture_content().text = original.into();
    doc.source_changed();
    assert_eq!(doc.version(), 2);
    assert_eq!(doc.line_count(), 3);
    assert!(
        !doc.dirty(),
        "Restoring the saved source must clear the dirty flag"
    );
    let version = doc.begin_preview_reload();
    assert_eq!(version, 3);
    assert!(
        !doc.dirty(),
        "Decoder invalidation must not make clean source dirty"
    );
    doc.renamed("renamed.rs".into());
    assert_eq!(doc.version(), 4);
    assert_eq!(doc.path.as_deref(), Some("renamed.rs"));
    assert_eq!(doc.saved_text, original);
    assert!(!doc.dirty());
}

#[test]
fn close_reorder_and_empty_transitions_preserve_selection_and_never_reuse_ids() {
    let mut docs = Documents::default();
    let a = docs.new_file();
    let b = docs.new_file();
    let c = docs.new_file();
    docs.select_index(1);
    assert_eq!(docs.close(a).unwrap().id, a);
    assert_eq!(docs.selected_id(), Some(b));
    assert!(docs.reorder(c, b, false));
    assert_eq!(docs.selected_id(), Some(b));
    assert!(!docs.reorder(c, u64::MAX, true));
    assert!(!docs.select_index(usize::MAX));
    assert_eq!(docs.selected_id(), Some(b));
    docs.close(b);
    assert_eq!(docs.selected_id(), Some(c));
    docs.close(c);
    assert!(docs.active().is_none());
    assert_eq!(docs.active_index(), 0);
    let next = docs.new_file();
    assert!(next > c);
    docs.clear();
    assert!(docs.new_file() > next);
}

#[test]
fn canonical_alias_replies_select_existing_edits_without_creating_another_identity() {
    let mut docs = Documents::default();
    let images = Arc::new(markdown::Images::default());
    let shutdowns = notebook_view::Shutdowns::default();
    assert!(docs.begin_open("alias.txt".into()));
    assert!(!docs.begin_open("alias.txt".into()));
    let response = Response {
        path: "canonical.txt".into(),
        text: "λ\nsecond\n".into(),
        revision: "r1".into(),
        ..Default::default()
    };
    let id = docs.open(response.clone(), &images, &shutdowns);
    assert_eq!(docs.active().unwrap().lines, 3);
    assert_eq!(docs.active().unwrap().cursor, (1, 1));
    assert!(!docs.active().unwrap().dirty());
    docs.active_mut().unwrap().fixture_content().text = "unsaved newer text".into();
    docs.active_mut().unwrap().set_dirty_fixture(true);
    let other = docs.new_file();
    assert_eq!(docs.open(response, &images, &shutdowns), id);
    assert_eq!(docs.len(), 2);
    assert_eq!(docs.active().unwrap().text, "unsaved newer text");
    assert!(docs.active().unwrap().dirty());
    docs.finish_open("alias.txt");
    assert!(!docs.has_pending_opens());
    docs.clear();
    assert!(docs.new_file() > other);
}

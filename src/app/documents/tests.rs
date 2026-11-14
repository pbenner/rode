//! Structural mutations preserve selected identities and pending-read ownership.
use super::*;

#[test]
fn source_changes_keep_revisions_dirty_state_and_line_counts_consistent() {
    let mut docs = Documents::default();
    docs.new_file();
    let doc = docs.active_mut().unwrap();
    let original = "λ\r\nsecond\r\n";
    doc.text = original.into();
    doc.acknowledge_save("source.rs".into(), original.into(), "saved".into());
    doc.text = "λ\r\nchanged\r\nextra".into();
    doc.source_changed();
    assert_eq!(doc.version(), 1);
    assert_eq!(doc.line_count(), 3);
    assert!(doc.dirty());
    doc.text = original.into();
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
    docs.active_mut().unwrap().text = "unsaved newer text".into();
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

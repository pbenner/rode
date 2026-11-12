//! Structural mutations preserve selected identities and pending-read ownership.
use super::*;

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
    docs.active_mut().unwrap().modified = true;
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

use super::*;
use crate::app::{crash_recovery::Entry, project_bookmarks::Project};
use std::os::unix::fs::PermissionsExt;

pub(in crate::app::workspace_sessions) fn snapshot() -> Snapshot {
    Snapshot {
        location: Location::Project {
            project: Project::Local {
                root: "/home/test/project".into(),
                sandbox: Default::default(),
            },
        },
        policy: "a".repeat(64),
        documents: vec![Document {
            key: 1,
            path: Some("/home/test/project/notes.txt".into()),
            name: "notes.txt".into(),
            relative: false,
            revision: Some("baseline".into()),
            cursor: (2, 3),
            group: 0,
            selected: true,
            private: false,
            notebook_source: false,
            draft: Some(Entry {
                name: "notes.txt".into(),
                text: "Unsaved 日本語".into(),
                notebook: false,
            }),
        }],
        layout: Default::default(),
        draft: "Unsent prompt".into(),
        agent: None,
        terminals: vec![],
        selected_terminal: None,
        runtimes: vec![],
    }
}

#[test]
fn ownership_transfer_rejects_old_writers_and_stale_confirmations() {
    let temp = tempfile::tempdir().unwrap();
    let first = Store::open(temp.path().to_owned()).unwrap();
    let second = Store::open(temp.path().to_owned()).unwrap();
    let (original, old) = first.create(snapshot()).unwrap();
    assert!(first.list().unwrap().rows[0].active);
    assert!(second.claim(&original.key(), false).is_err());
    assert!(first.owns(&old).unwrap()); // Populate the ownership cache before takeover.
    let (resumed, current) = second.claim(&original.key(), true).unwrap();
    assert_ne!(original.key(), resumed.key());
    assert!(!first.owns(&old).unwrap());
    assert!(first.save(&old, snapshot()).is_err());
    assert!(first.claim(&original.key(), true).is_err());
    drop(old);
    assert!(second.list().unwrap().rows[0].active);
    assert_eq!(
        second.record(&resumed.id).unwrap().snapshot.draft,
        "Unsent prompt"
    );
    drop(current);
    assert!(!first.list().unwrap().rows[0].active);
    let (_, reopened) = first.claim(&resumed.key(), false).unwrap();
    assert!(first.owns(&reopened).unwrap());
}

#[test]
fn invalid_or_oversized_checkpoints_preserve_the_previous_durable_record() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path().to_owned()).unwrap();
    let (record, owner) = store.create(snapshot()).unwrap();
    let mut changed = snapshot();
    changed.draft = "x".repeat(persistence::LIMIT);
    assert!(store.save(&owner, changed).is_err());
    let mut changed = snapshot();
    changed.location = Location::Project {
        project: Project::Local {
            root: "/another/project".into(),
            sandbox: Default::default(),
        },
    };
    assert!(store.save(&owner, changed).is_err());
    assert_eq!(store.record(&record.id).unwrap().revision, 1);
    assert_eq!(
        store.record(&record.id).unwrap().snapshot.documents[0]
            .draft
            .as_ref()
            .unwrap()
            .text,
        "Unsaved 日本語"
    );
    assert_eq!(
        fs::metadata(store.path(&record.id).unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn rename_merges_with_saves_and_remove_requires_an_inactive_unchanged_record() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path().to_owned()).unwrap();
    let (record, owner) = store.create(snapshot()).unwrap();
    store
        .rename(&record.key(), "Analysis 日本語".into())
        .unwrap();
    let saved = store.save(&owner, snapshot()).unwrap();
    assert_eq!(saved.name, "Analysis 日本語");
    assert!(store.remove(&saved.key(), saved.revision).is_err());
    drop(owner);
    assert!(store.remove(&record.key(), record.revision).is_err());
    store.remove(&saved.key(), saved.revision).unwrap();
    assert!(store.list().unwrap().rows.is_empty());
}

#[test]
fn encrypted_workspaces_and_tainted_buffers_never_accept_plaintext_drafts() {
    let mut saved = snapshot();
    saved.location = Location::Encrypted {
        cipherdir: "/home/test/encrypted".into(),
        remote: None,
        sandbox: Default::default(),
    };
    assert!(saved.validate().is_err());
    saved.documents[0].draft = None;
    assert!(saved.validate().is_err());
    saved.draft.clear();
    saved.documents[0].relative = true;
    saved.documents[0].path = Some("notes.txt".into());
    assert!(saved.validate().is_ok());
    saved.documents[0].path = Some("../outside".into());
    assert!(saved.validate().is_err());
    let mut saved = snapshot();
    saved.documents[0].private = true;
    assert!(saved.validate().is_err());
}

#[test]
fn corrupt_entries_are_preserved_without_hiding_healthy_workspaces() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path().to_owned()).unwrap();
    let (record, _owner) = store.create(snapshot()).unwrap();
    let bad = store.path(&rode_session::new_id().unwrap()).unwrap();
    persistence::replace(&bad, b"bad JSON", true).unwrap();
    let catalogue = store.list().unwrap();
    assert_eq!(catalogue.rows.len(), 1);
    assert_eq!(catalogue.rows[0].summary.id, record.id);
    assert_eq!(catalogue.warnings.len(), 1);
    assert_eq!(fs::read(bad).unwrap(), b"bad JSON");
}

#[test]
fn focus_requests_are_scoped_to_the_current_generation_and_desktop() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path().to_owned()).unwrap();
    let (record, old) = store.create(snapshot()).unwrap();
    store.focus(&record.key()).unwrap();
    let (next, owner) = store.claim(&record.key(), true).unwrap();
    assert!(!store.requested_focus(&owner).unwrap());
    assert!(store.focus(&record.key()).is_err());
    store.focus(&next.key()).unwrap();
    assert!(store.requested_focus(&owner).unwrap());
    assert!(!store.requested_focus(&owner).unwrap());
    drop(old);
}

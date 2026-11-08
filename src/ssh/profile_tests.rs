//! Saved-list edits operate on current records, preserving order, unknown fields
//! and concurrent settings while refusing stale destructive actions.
use super::*;
use serde_json::json;

fn profile(name: &str) -> Profile {
    Profile {
        name: name.into(),
        host: format!("{name}.example"),
        ..Default::default()
    }
}

#[test]
fn saved_profile_moves_and_removal_preserve_records_and_persist_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.json");
    let mut raw = json!([profile("a"), profile("b"), profile("c")]);
    raw[1]["future"] = json!({"keep": true});
    std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    for (label, relative_to, after, expected) in [
        ("c", "a", false, vec!["c", "a", "b"]),
        ("c", "b", true, vec!["a", "b", "c"]),
        ("a", "c", false, vec!["b", "a", "c"]),
        ("c", "b", true, vec!["b", "c", "a"]),
        ("a", "a", false, vec!["b", "c", "a"]),
        ("a", "c", true, vec!["b", "c", "a"]),
    ] {
        let profiles = edit_profiles_at(
            &path,
            &ProfileListEdit::Move {
                label: label.into(),
                relative_to: relative_to.into(),
                after,
            },
        )
        .unwrap();
        assert_eq!(
            profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            profiles_at(&path)
                .unwrap()
                .iter()
                .map(Profile::label)
                .collect::<Vec<_>>(),
            expected
        );
    }
    let profiles =
        edit_profiles_at(&path, &ProfileListEdit::Remove(Box::new(profile("c")))).unwrap();
    assert_eq!(
        profiles.iter().map(Profile::label).collect::<Vec<_>>(),
        ["b", "a"]
    );
    let remaining: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(remaining[0], raw[1]);
    assert_eq!(remaining[1], raw[0]);
}

#[test]
fn saved_profile_management_rejects_stale_deletes_and_missing_move_targets() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.json");
    let baseline = profile("a");
    save_profile_at(&path, &baseline, None).unwrap();
    save_profile_at(&path, &profile("b"), None).unwrap();
    let mut edited = baseline.clone();
    edited.user = "new-login".into();
    save_profile_at(&path, &edited, Some(&baseline)).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(
        edit_profiles_at(&path, &ProfileListEdit::Remove(Box::new(baseline.clone())))
            .unwrap_err()
            .to_string()
            .contains("changed in another window")
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    for (label, anchor) in [("missing", "a"), ("a", "missing")] {
        assert!(
            edit_profiles_at(
                &path,
                &ProfileListEdit::Move {
                    label: label.into(),
                    relative_to: anchor.into(),
                    after: false,
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    edit_profiles_at(&path, &ProfileListEdit::Remove(Box::new(edited))).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut stale_save = baseline.clone();
    stale_save.port = "2222".into();
    assert!(
        save_profile_at(&path, &stale_save, Some(&baseline)).is_err(),
        "Stale saves must not resurrect removed profiles"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // An explicit new name can save the retained draft as a new connection.
    stale_save.name = "new connection".into();
    let profiles = save_profile_at(&path, &stale_save, Some(&baseline)).unwrap();
    assert_eq!(
        profiles.iter().map(Profile::label).collect::<Vec<_>>(),
        ["b", "new connection"]
    );
}

#[test]
fn saved_profile_rename_keeps_its_order_and_rejects_name_collisions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.json");
    let original = profile("b");
    let mut raw = json!([profile("a"), original, profile("c")]);
    raw[1]["future"] = json!(42);
    std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let mut concurrent = original.clone();
    concurrent.user = "updated-user".into();
    save_profile_at(&path, &concurrent, Some(&original)).unwrap();
    let mut renamed = original.clone();
    renamed.name = "renamed".into();
    let result = save_profile_at(&path, &renamed, Some(&original)).unwrap();
    assert_eq!(
        result.iter().map(Profile::label).collect::<Vec<_>>(),
        ["a", "renamed", "c"]
    );
    assert_eq!(result[1].user, "updated-user");
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted[1]["future"], 42);
    let before = std::fs::read(&path).unwrap();
    let mut collision = renamed.clone();
    collision.name = "c".into();
    assert!(save_profile_at(&path, &collision, Some(&renamed)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn saved_profile_management_preserves_invalid_storage_and_ambiguous_names() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.json");
    for content in [
        "broken".to_owned(),
        "{}".to_owned(),
        serde_json::to_string(&vec![profile("a"); 129]).unwrap(),
        serde_json::to_string(&vec![profile("a"); 2]).unwrap(),
    ] {
        std::fs::write(&path, &content).unwrap();
        for edit in [
            ProfileListEdit::Remove(Box::new(profile("a"))),
            ProfileListEdit::Move {
                label: "a".into(),
                relative_to: "a".into(),
                after: false,
            },
        ] {
            assert!(edit_profiles_at(&path, &edit).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        }
        let original = profile("a");
        let mut edited = original.clone();
        edited.name = "renamed".into();
        assert!(save_profile_at(&path, &edited, Some(&original)).is_err());
        assert!(save_profile_at(&path, &original, None).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    }
}

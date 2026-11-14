use super::*;
use eframe::Storage;

fn app() -> (EditorApp, egui::Context) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    (EditorApp::new(&cc, Startup::default()), ctx)
}
fn local(root: &str) -> Bookmark {
    Bookmark {
        name: "Project".into(),
        project: Project::Local {
            root: root.into(),
            sandbox: Default::default(),
        },
    }
}

#[test]
fn location_identity_distinguishes_hosts_and_preserves_policy_independent_deduplication() {
    let first = local("/projects/example");
    let mut changed = first.clone();
    changed.name = "Renamed".into();
    if let Project::Local { sandbox, .. } = &mut changed.project {
        sandbox.enabled = !sandbox.enabled;
    }
    assert_eq!(first.project.id(), changed.project.id());
    let profile = Profile {
        host: "example.test".into(),
        root: "/projects/example".into(),
        ..Default::default()
    };
    let remote = Project::Ssh {
        profile: Box::new(profile.clone()),
    };
    assert_ne!(first.project.id(), remote.id());
    let mut renamed = profile.clone();
    renamed.name = "Another label".into();
    renamed.sandbox.enabled = !renamed.sandbox.enabled;
    assert_eq!(
        remote.id(),
        Project::Ssh {
            profile: Box::new(renamed)
        }
        .id()
    );
    let mut other_host = profile.clone();
    other_host.host = "other.test".into();
    assert!(!Project::same_connection(&profile, &other_host));
    assert_ne!(
        remote.id(),
        Project::Ssh {
            profile: Box::new(other_host)
        }
        .id()
    );
    let mut other_root = profile.clone();
    other_root.root = "/projects/second".into();
    assert!(Project::same_connection(&profile, &other_root));
    assert_ne!(
        remote.id(),
        Project::Ssh {
            profile: Box::new(other_root)
        }
        .id()
    );
}

#[test]
fn rename_preserves_unknown_record_fields_and_disabling_keeps_saved_projects() {
    let (mut app, _) = app();
    let bookmark = local("/projects/example");
    let id = bookmark.project.id();
    app.edit_bookmarks(Edit::Insert(bookmark)).unwrap();
    app.extensions
        .registry
        .preferences
        .get_mut(FEATURE.id())
        .unwrap()
        .settings
        .get_mut(SETTING)
        .unwrap()[&id]["future"] = serde_json::json!({"retained": true});
    app.edit_bookmarks(Edit::Rename(id.clone(), "Custom name".into()))
        .unwrap();
    assert_eq!(
        app.bookmark_settings().unwrap()[&id]["future"]["retained"],
        true
    );
    assert_eq!(app.project_bookmarks.projects[&id].name, "Custom name");
    app.set_extension(FEATURE.id(), false);
    assert!(!app.feature(FEATURE));
    app.sync_bookmarks();
    assert!(app.project_bookmarks.projects.contains_key(&id));
}

#[test]
fn malformed_or_mismatched_bookmarks_are_preserved_and_cannot_be_overwritten() {
    let (mut app, _) = app();
    let raw = serde_json::json!({"wrong-id": local("/projects/example")});
    app.extensions
        .registry
        .preferences
        .entry(FEATURE.id().into())
        .or_default()
        .settings
        .insert(SETTING.into(), raw.clone());
    app.sync_bookmarks();
    assert!(app.project_bookmarks.error.is_some());
    assert!(
        app.edit_bookmarks(Edit::Insert(local("/projects/other")))
            .is_err()
    );
    assert_eq!(app.bookmark_settings(), Some(&raw));
    assert!(model::decode(Some(&serde_json::json!([]))).is_err());
    assert!(model::validate_name("\n").is_err());
}

#[test]
fn reopening_local_and_ssh_bookmarks_waits_for_dirty_buffers_before_starting_work() {
    for remote in [false, true] {
        let (mut app, ctx) = app();
        app.new_file();
        app.documents
            .active_mut()
            .unwrap()
            .set_source("Keep this edit".into());
        let bookmark = if remote {
            Bookmark {
                name: "Remote".into(),
                project: Project::Ssh {
                    profile: Box::new(Profile {
                        host: "example.test".into(),
                        root: "/projects/example".into(),
                        ..Default::default()
                    }),
                },
            }
        } else {
            local("/projects/example")
        };
        app.reopen_bookmark(bookmark, &ctx);
        assert!(matches!(
            (remote, app.pending.as_ref()),
            (true, Some(Action::Remote(_))) | (false, Some(Action::Project { remote: false, .. }))
        ));
        assert!(!app.workspace.preparing());
        assert!(app.ssh_master.is_none());
        assert_eq!(app.documents.active().unwrap().text, "Keep this edit");
    }
}

#[test]
fn separate_preference_writers_merge_bookmarks_and_remove_only_the_selected_record() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.ron");
    let (mut first, _) = app();
    let (mut second, _) = app();
    let mut a = crate::persistence::Preferences::open(path.clone(), Arc::new(|| {}));
    let mut b = crate::persistence::Preferences::open(path.clone(), Arc::new(|| {}));
    let one = local("/projects/one");
    let two = local("/projects/two");
    let one_id = one.project.id();
    let two_id = two.project.id();
    first.edit_bookmarks(Edit::Insert(one)).unwrap();
    second.edit_bookmarks(Edit::Insert(two)).unwrap();
    first.persist_extensions(Some(&mut a));
    drop(a);
    // This window retains its A-only baseline while the other window adds B.
    let mut store = crate::persistence::Preferences::open(path.clone(), Arc::new(|| {}));
    second.persist_extensions(Some(&mut b));
    drop(b);
    let saved = crate::persistence::Preferences::open(path.clone(), Arc::new(|| {}));
    let preferences =
        crate::extensions::preferences(&saved.get_string(crate::extensions::STORAGE_KEY).unwrap())
            .unwrap();
    assert_eq!(
        model::decode(preferences[FEATURE.id()].settings.get(SETTING))
            .unwrap()
            .len(),
        2
    );
    drop(saved);
    first.edit_bookmarks(Edit::Remove(one_id.clone())).unwrap();
    first.persist_extensions(Some(&mut store));
    drop(store);
    let saved =
        crate::persistence::Preferences::open(directory.path().join("app.ron"), Arc::new(|| {}));
    let preferences =
        crate::extensions::preferences(&saved.get_string(crate::extensions::STORAGE_KEY).unwrap())
            .unwrap();
    let bookmarks = model::decode(preferences[FEATURE.id()].settings.get(SETTING)).unwrap();
    assert!(!bookmarks.contains_key(&one_id));
    assert!(bookmarks.contains_key(&two_id));
}

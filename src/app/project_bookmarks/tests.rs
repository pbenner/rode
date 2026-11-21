use super::*;
use eframe::Storage;

fn app() -> (EditorApp, egui::Context) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    (EditorApp::new(&cc, Startup::default()), ctx)
}
fn local(root: &str) -> Bookmark {
    Bookmark {
        shared_folders: None,
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
                shared_folders: None,
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
        assert!(matches!(app.pending.as_ref(), Some(Action::Bookmark(saved))
            if matches!(saved.project, Project::Ssh { .. }) == remote));
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

#[test]
fn shared_folder_settings_are_included_by_default_optional_and_bounded() {
    let (mut app, ctx) = app();
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    let config = crate::shared_folders::Config {
        source: "/local/chosen".into(),
        destination: "/remote/mount".into(),
        ..Default::default()
    };
    app.bind_project_transfers();
    app.shared_folders.restore(vec![config.clone()]);
    let saved = app.bookmark_candidate().unwrap();
    app.project_bookmarks.include_shares = false;
    let plain = app.bookmark_candidate().unwrap();
    assert!(plain.shared_folders.is_none());
    app.project_bookmarks.include_shares = true;
    assert_eq!(saved.project.id(), plain.project.id());
    assert_eq!(
        saved.shared_folders.as_ref().unwrap(),
        &vec![config.clone()]
    );
    let id = saved.project.id();
    app.edit_bookmarks(Edit::Insert(saved.clone())).unwrap();
    app.edit_bookmarks(Edit::Rename(id.clone(), "With shared folders".into()))
        .unwrap();
    assert_eq!(
        app.project_bookmarks.projects[&id].shared_folders,
        saved.shared_folders
    );
    app.shared_folders.reset();
    app.reopen_bookmark(saved, &ctx);
    assert!(app.project_bookmarks.restore_shares.is_some());
    assert!(app.shared_folders.configs().is_empty());
    assert!(!app.shared_folders.pending());
    app.project_bookmarks.restore_shares = None;
    app.edit_bookmarks(Edit::Shares(id.clone(), None)).unwrap();
    assert!(app.project_bookmarks.projects[&id].shared_folders.is_none());
    assert!(
        app.bookmark_settings().unwrap()[&id]
            .get("shared_folders")
            .is_none()
    );
    let mut invalid = app.project_bookmarks.projects[&id].clone();
    invalid.shared_folders = Some(vec![crate::shared_folders::Config::default()]);
    assert!(app.edit_bookmarks(Edit::Insert(invalid)).is_err());
}

#[test]
fn disconnected_project_cannot_clear_saved_shared_folders_from_the_menu() {
    let (mut app, ctx) = app();
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    app.bind_project_transfers();
    app.shared_folders
        .restore(vec![crate::shared_folders::Config {
            source: "/local/source".into(),
            destination: "/remote/mount".into(),
            ..Default::default()
        }]);
    app.project_bookmarks.include_shares = true;
    let bookmark = app.bookmark_candidate().unwrap();
    let id = bookmark.project.id();
    app.edit_bookmarks(Edit::Insert(bookmark)).unwrap();
    let saved = app.bookmark_settings().unwrap().clone();
    app.workspace.as_mut().unwrap().connected = false;
    assert!(app.bookmark_share_configs().is_none());
    let temp = tempfile::tempdir().unwrap();
    let snapshot = temp.path().join("state.json");
    ui_probe::install_at(&ctx, snapshot.clone());
    let mut frame = |events| {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(450., 600.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.project_bookmarks_ui(ui, ctx));
            },
        );
    };
    let control = |name: &str| {
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&snapshot).unwrap()).unwrap();
        state["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == name)
            .unwrap_or_else(|| panic!("Missing {name}"))
            .clone()
    };
    let center = |button: &serde_json::Value| {
        let rect = &button["rect"];
        egui::pos2(
            ((rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap()) / 2.) as f32,
            ((rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap()) / 2.) as f32,
        )
    };
    let pointer = |pos, button, pressed| egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Default::default(),
    };
    frame(vec![]);
    frame(vec![]);
    let row = center(&control(&format!("saved-project:{id}")));
    frame(vec![
        egui::Event::PointerMoved(row),
        pointer(row, egui::PointerButton::Secondary, true),
    ]);
    frame(vec![pointer(row, egui::PointerButton::Secondary, false)]);
    frame(vec![]);
    frame(vec![]);
    let update = control(&format!("bookmark-shares-update:{id}"));
    assert_eq!(update["enabled"], false);
    let point = center(&update);
    frame(vec![
        egui::Event::PointerMoved(point),
        pointer(point, egui::PointerButton::Primary, true),
    ]);
    frame(vec![pointer(point, egui::PointerButton::Primary, false)]);
    assert_eq!(app.bookmark_settings(), Some(&saved));
}

fn restoration(app: &EditorApp, stage: RestoreStage) -> Restoration {
    Restoration {
        project: app.bookmark_candidate().unwrap().project,
        configs: vec![crate::shared_folders::Config {
            source: "/chosen/source".into(),
            destination: "/chosen/mount".into(),
            ..Default::default()
        }],
        stage,
    }
}

#[test]
fn catalogue_loading_is_inert_and_superseded_preparations_cannot_restore_shares() {
    let (mut app, ctx) = app();
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    let mut bookmark = app.bookmark_candidate().unwrap();
    bookmark.shared_folders = Some(restoration(&app, RestoreStage::Armed).configs);
    app.edit_bookmarks(Edit::Insert(bookmark)).unwrap();
    app.restore_bookmark_shares(&ctx);
    assert!(app.project_bookmarks.restore_shares.is_none());
    assert!(!app.shared_folders.pending());

    let intent = restoration(&app, RestoreStage::Armed);
    let path = intent.project.root().to_owned();
    app.project_bookmarks.restore_shares = Some(intent);
    let first = app.workspace.begin_preparation();
    app.bookmark_project_preparing(first, &path, true);
    assert!(
        matches!(app.project_bookmarks.restore_shares.as_ref().unwrap().stage,
        RestoreStage::Preparing(id) if id == first)
    );
    app.workspace.begin_preparation();
    app.restore_bookmark_shares(&ctx);
    assert!(app.project_bookmarks.restore_shares.is_none());
    assert!(!app.shared_folders.pending());
}

#[test]
fn ready_restoration_requires_the_accepted_generation_and_enabled_transfers() {
    for disabled in [false, true] {
        let (mut app, ctx) = app();
        app.workspace
            .set_fixture(Some(crate::app::tests::remote_workspace()));
        app.project_bookmarks.restore_shares = Some(restoration(
            &app,
            RestoreStage::Ready(app.workers.generation()),
        ));
        if disabled {
            app.set_extension(crate::extensions::Feature::Transfers.id(), false);
        } else {
            app.workers.invalidate_workspace();
        }
        app.restore_bookmark_shares(&ctx);
        assert!(app.project_bookmarks.restore_shares.is_none());
        assert!(app.shared_folders.configs().is_empty());
        assert!(!app.shared_folders.pending());
        if disabled {
            assert!(app.notice.as_ref().unwrap().1);
        }
    }
}

#[test]
fn failed_commits_and_replaced_connections_cannot_mount_bookmark_recipes() {
    let (mut app, ctx) = app();
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    let intent = restoration(&app, RestoreStage::Armed);
    let id = app.workspace.begin_preparation();
    app.project_bookmarks.restore_shares = Some(Restoration {
        stage: RestoreStage::Preparing(id),
        ..intent
    });
    app.workspace.finish_preparation(id);
    app.bookmark_project_committed(id, app.workers.generation());
    assert!(app.project_bookmarks.restore_shares.is_none());
    app.project_bookmarks.restore_shares = Some(restoration(
        &app,
        RestoreStage::Connecting(app.workers.generation()),
    ));
    app.workspace.as_mut().unwrap().ready = false;
    app.restore_bookmark_shares(&ctx);
    assert!(app.project_bookmarks.restore_shares.is_some());
    app.workspace.as_mut().unwrap().remote = Some(
        Remote::new(Profile {
            host: "another-host".into(),
            ..Default::default()
        })
        .unwrap(),
    );
    app.restore_bookmark_shares(&ctx);
    assert!(app.project_bookmarks.restore_shares.is_none());
    assert!(!app.shared_folders.pending());
}

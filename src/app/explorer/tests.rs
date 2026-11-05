//! Pure path bookkeeping, async reply identity and modal focus regressions.
use super::*;

fn entry(path: &str, dir: bool) -> Entry {
    Entry {
        path: path.into(),
        name: ssh::display_name(path).into(),
        dir,
    }
}

#[test]
fn rename_updates_exact_entries_and_descendants_without_matching_similar_names() {
    let folder = entry("/project/source", true);
    assert_eq!(
        renamed("/project/source", &folder, "/project/new"),
        Some("/project/new".into())
    );
    assert_eq!(
        renamed("/project/source/nested/file.txt", &folder, "/project/new"),
        Some("/project/new/nested/file.txt".into())
    );
    assert_eq!(
        renamed("/project/source-other/file.txt", &folder, "/project/new"),
        None
    );
    let file = entry("/project/source.txt", false);
    assert_eq!(
        renamed("/project/source.txt", &file, "/project/renamed.txt"),
        Some("/project/renamed.txt".into())
    );
    assert_eq!(
        renamed("/project/source.txt/child", &file, "/project/renamed.txt"),
        None
    );
}

#[test]
fn renaming_a_cached_folder_keeps_unrelated_rows_and_replaces_stale_destination_caches() {
    let source = entry("/project/source", true);
    let mut directories = HashMap::from([
        (
            "/project".into(),
            vec![
                source.clone(),
                entry("/project/other", true),
                entry("/project/renamed", true),
            ],
        ),
        (
            source.path.clone(),
            vec![entry("/project/source/file.txt", false)],
        ),
        (
            "/project/other".into(),
            vec![entry("/project/other/keep.txt", false)],
        ),
        (
            "/project/renamed".into(),
            vec![entry("/project/renamed/stale.txt", false)],
        ),
    ]);
    rename_directories(&mut directories, &source, "/project/renamed");
    assert!(!directories.contains_key(&source.path));
    assert_eq!(
        directories["/project"]
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["other", "renamed"]
    );
    assert_eq!(
        directories["/project/renamed"][0].path,
        "/project/renamed/file.txt"
    );
    assert_eq!(
        directories["/project/other"][0].path,
        "/project/other/keep.txt"
    );
    assert_eq!(directories.len(), 3);
}

#[test]
fn operation_names_are_literal_single_components_with_a_utf8_byte_limit() {
    for name in [
        "hello.txt",
        "with spaces",
        "λ 🦀",
        "literal 'quotes' $(no-shell)",
    ] {
        assert!(valid_name(name));
    }
    for name in ["", " ", ".", "..", "nested/file", "nul\0byte"] {
        assert!(!valid_name(name));
    }
    assert!(!valid_name(&"λ".repeat(128)));
    assert!(valid_name(&"x".repeat(255)));
}

#[test]
fn mutation_controls_follow_actual_policy_connection_and_pending_io() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx);
    let mut app = EditorApp::new(&cc, Startup::default());
    assert!(!app.explorer_writable());
    app.workspace = Some(Workspace {
        root: "/project".into(),
        remote: None,
        connected: true,
        ready: true,
        plan: sandbox::Plan {
            active: true,
            project_read_only: true,
            ..Default::default()
        },
        sandbox_options: Default::default(),
    });
    assert!(!app.explorer_writable());
    app.workspace.as_mut().unwrap().plan.active = false;
    assert!(
        app.explorer_writable(),
        "Unavailable sandbox options are not an active read-only policy"
    );
    app.preparing = true;
    assert!(!app.explorer_writable());
    app.preparing = false;
    app.explorer_state.busy = true;
    assert!(!app.explorer_writable());
    app.explorer_state.busy = false;
    app.opening.insert("/project/file".into());
    assert!(!app.explorer_writable());
    app.opening.clear();
    app.new_file();
    app.documents[0].saving = true;
    assert!(!app.explorer_writable());
    app.documents[0].saving = false;
    app.workspace.as_mut().unwrap().connected = false;
    assert!(!app.explorer_writable());
}

#[test]
fn project_preparation_cannot_invalidate_an_inflight_explorer_operation() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let token = app.project_token;
    app.explorer_state.busy = true;
    app.prepare_project(
        "/project".into(),
        Default::default(),
        false,
        true,
        None,
        &ctx,
    );
    assert!(!app.preparing);
    assert_eq!(app.project_token, token);
    assert!(app.explorer_state.busy);
}

#[test]
fn a_form_recovers_focus_lost_when_its_context_menu_closes() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.explorer_form(Kind::File, entry("/project", true));
    // The pointer click that opened a modal can clear its initial focus request.
    app.dialog_focus = false;
    for _ in 0..2 {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1380., 900.),
                )),
                ..Default::default()
            },
            |ctx| app.dialogs(ctx),
        );
    }
    assert!(ctx.memory(|memory| memory.has_focus(egui::Id::new("explorer-operation-name"))));
}

#[test]
fn stale_directory_replies_cannot_restore_rows_or_clear_new_loading_state() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.explorer_state.epoch = 2;
    app.loading.insert("/project".into());
    app.tx
        .send(Event::Done {
            generation: app.generation,
            job: Job::List("/project".into(), 1),
            result: Box::new(Ok(Response {
                path: "/project".into(),
                entries: vec![entry("/project/stale", false)],
                ..Default::default()
            })),
        })
        .unwrap();
    app.poll(&ctx);
    assert!(app.directories.is_empty());
    assert!(app.loading.contains("/project"));
}

#[test]
fn a_confirmation_dialog_keeps_its_enter_key_instead_of_toggling_a_tree_row() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let folder = entry("/project/folder", true);
    app.tree_rows.push(TreeRow {
        entry: folder.clone(),
        depth: 0,
    });
    app.expanded.insert(folder.path.clone());
    app.navigation.tree.path = Some(folder.path.clone());
    app.dialog = Dialog::Explorer;
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        },
        |ctx| {
            ctx.memory_mut(|memory| memory.request_focus(navigation::Pane::Explorer.id()));
            app.tree_keyboard(ctx);
            assert!(ctx.input(|input| input.events.iter().any(|event| matches!(
                event,
                egui::Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    ..
                }
            ))));
        },
    );
    assert!(app.expanded.contains(&folder.path));
}

#[test]
fn a_deleted_files_retained_editor_blocks_reusing_its_path() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().to_string_lossy().into_owned();
    let path = base.path().join("retained.txt");
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace = Some(Workspace {
        root: root.clone(),
        remote: None,
        connected: true,
        ready: true,
        plan: Default::default(),
        sandbox_options: Default::default(),
    });
    app.new_file();
    app.documents[0].path = Some(path.to_string_lossy().into_owned());
    app.documents[0].text = "retained unsaved contents".into();
    app.documents[0].modified = true;
    app.explorer_form(Kind::File, entry(&root, true));
    app.dialog_input = "retained.txt".into();
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.explorer_dialog(ui, ctx));
        },
    );
    assert!(!app.explorer_state.busy);
    assert!(app.dialog == Dialog::Explorer);
    assert!(!path.exists());
    assert_eq!(app.documents[0].text, "retained unsaved contents");
}

use super::*;

fn fixture(root: &std::path::Path) -> (EditorApp, egui::Context) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let root = root.to_string_lossy().into_owned();
    app.workspace.set_fixture(Some(Workspace {
        root: root.clone(),
        remote: None,
        connected: true,
        ready: true,
        plan: sandbox::Plan {
            root,
            ..Default::default()
        },
        sandbox_options: Default::default(),
    }));
    refresh(&mut app);
    (app, ctx)
}

fn refresh(app: &mut EditorApp) {
    let reply = files::local(&Request::List {
        path: app.workspace.as_ref().unwrap().root.clone(),
    })
    .unwrap();
    app.transfers.panes[Side::Local]
        .browser
        .loaded(files::Response {
            path: reply.path,
            entries: reply.entries,
            ..Default::default()
        });
}

fn entries(app: &EditorApp, name: &str) -> Vec<Entry> {
    app.transfers.panes[Side::Local]
        .browser
        .entries
        .iter()
        .filter(|entry| entry.name == name)
        .cloned()
        .collect()
}

fn wait(app: &mut EditorApp, ctx: &egui::Context) {
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while app.transfers.file_operation.is_some() {
        app.transfer_tick(ctx);
        assert!(std::time::Instant::now() < until, "{}", app.status);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn confirmed_changes_preserve_unsaved_editors_and_never_replace_existing_entries() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("original ' 日本.txt");
    std::fs::write(&source, "saved contents").unwrap();
    let destination = root.path().join("renamed.txt");
    let occupied = root.path().join("occupied.txt");
    std::fs::write(&occupied, "keep this").unwrap();
    let (mut app, ctx) = fixture(root.path());
    app.new_file();
    app.documents[0].fixture_content().path = Some(source.to_string_lossy().into());
    app.documents[0].fixture_content().text = "unsaved edits".into();
    app.documents[0].set_dirty_fixture(true);

    app.transfer_file_form(
        Side::Local,
        Action::Rename,
        entries(&app, "original ' 日本.txt"),
    );
    app.transfers.file_form.as_mut().unwrap().name = "renamed.txt".into();
    app.start_transfer_files(&ctx);
    assert!(app.explorer_state.busy);
    wait(&mut app, &ctx);
    assert!(!app.explorer_state.busy);
    assert!(!source.exists());
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "saved contents"
    );
    assert_eq!(app.documents[0].path.as_deref(), destination.to_str());
    assert_eq!(app.documents[0].text, "unsaved edits");
    assert!(app.documents[0].dirty());

    refresh(&mut app);
    app.transfer_file_form(Side::Local, Action::Rename, entries(&app, "renamed.txt"));
    app.transfers.file_form.as_mut().unwrap().name = "occupied.txt".into();
    app.start_transfer_files(&ctx);
    wait(&mut app, &ctx);
    assert_eq!(std::fs::read_to_string(&occupied).unwrap(), "keep this");
    assert!(destination.exists());
    assert!(app.transfers.error.is_some());

    refresh(&mut app);
    app.transfer_file_form(Side::Local, Action::Delete, entries(&app, "renamed.txt"));
    app.start_transfer_files(&ctx);
    wait(&mut app, &ctx);
    assert!(!destination.exists());
    assert_eq!(app.documents[0].text, "unsaved edits");
    assert!(app.documents[0].dirty());
}

#[test]
fn stale_confirmations_roots_and_unlisted_paths_cannot_start_mutations() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("keep.txt");
    std::fs::write(&file, "keep").unwrap();
    let (mut app, ctx) = fixture(root.path());
    app.transfer_file_form(Side::Local, Action::Delete, entries(&app, "keep.txt"));
    app.transfers.panes[Side::Local].browser.token += 1;
    app.start_transfer_files(&ctx);
    assert!(app.transfers.file_operation.is_none());
    assert!(file.exists());
    app.transfer_file_form(
        Side::Local,
        Action::Delete,
        vec![Entry {
            name: "root".into(),
            path: root.path().to_string_lossy().into(),
            dir: true,
        }],
    );
    assert!(app.transfers.file_form.is_none());
    app.transfer_file_form(
        Side::Local,
        Action::Delete,
        vec![Entry {
            name: "unlisted".into(),
            path: root.path().join("unlisted").to_string_lossy().into(),
            dir: false,
        }],
    );
    assert!(app.transfers.file_form.is_none());
    app.transfer_file_form(Side::Local, Action::Delete, entries(&app, "keep.txt"));
    app.transfers.reset();
    app.start_transfer_files(&ctx);
    assert!(app.transfers.file_operation.is_none());
    assert!(file.exists());
}

#[test]
fn local_operations_do_not_rebind_remote_documents_with_the_same_path() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("source.txt");
    std::fs::write(&file, "local").unwrap();
    let (mut app, ctx) = fixture(root.path());
    app.workspace.as_mut().unwrap().remote = crate::app::tests::remote_workspace().remote;
    app.new_file();
    app.documents[0].fixture_content().path = Some(file.to_string_lossy().into());
    app.transfer_file_form(Side::Local, Action::Rename, entries(&app, "source.txt"));
    app.transfers.file_form.as_mut().unwrap().name = "moved.txt".into();
    app.start_transfer_files(&ctx);
    assert!(!app.explorer_state.busy);
    wait(&mut app, &ctx);
    assert!(root.path().join("moved.txt").exists());
    assert_eq!(app.documents[0].path.as_deref(), file.to_str());
}

#[test]
fn a_delete_dialog_starts_on_cancel_and_enter_preserves_the_files() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("keep.txt");
    std::fs::write(&file, "keep").unwrap();
    let (mut app, ctx) = fixture(root.path());
    app.transfer_file_form(Side::Local, Action::Delete, entries(&app, "keep.txt"));
    for frame in 0..4 {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(850., 560.),
                )),
                events: if frame == 3 {
                    vec![egui::Event::Key {
                        key: Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }]
                } else {
                    vec![]
                },
                ..Default::default()
            },
            |ctx| app.dialogs(ctx),
        );
    }
    assert!(app.dialog == Dialog::None);
    assert!(app.transfers.file_operation.is_none());
    assert!(file.exists());
}

#[test]
fn a_rename_dialog_keeps_enter_on_the_focused_cancel_button() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("source.txt");
    std::fs::write(&file, "keep").unwrap();
    let (mut app, ctx) = fixture(root.path());
    app.transfer_file_form(Side::Local, Action::Rename, entries(&app, "source.txt"));
    app.transfers.file_form.as_mut().unwrap().name = "renamed.txt".into();
    let probe = root.path().join("ui.json");
    ui_probe::install_at(&ctx, probe.clone());
    let mut render = |key: Option<Key>| {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(850., 560.),
                )),
                events: key
                    .map(|key| {
                        vec![egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }]
                    })
                    .unwrap_or_default(),
                ..Default::default()
            },
            |ctx| app.dialogs(ctx),
        );
    };
    for _ in 0..3 {
        render(None);
    }
    render(Some(Key::Tab));
    render(Some(Key::Tab));
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&probe).unwrap()).unwrap();
    assert!(
        snapshot["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|button| {
                button["name"] == "transfer-file-cancel" && button["focused"] == true
            })
    );
    render(Some(Key::Enter));
    assert!(app.dialog == Dialog::None);
    assert!(app.transfers.file_operation.is_none());
    assert!(file.exists());
    assert!(!root.path().join("renamed.txt").exists());
}

#[test]
fn stale_completion_cannot_rebind_current_editors_or_repopulate_a_changed_browser() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), "source").unwrap();
    let (mut app, ctx) = fixture(root.path());
    let entry = entries(&app, "source.txt").pop().unwrap();
    app.new_file();
    app.documents[0].fixture_content().path = Some(entry.path.clone());
    let operation = Running {
        id: 7,
        generation: app.transfers.generation,
        project_generation: Some(app.workers.generation().wrapping_add(1)),
        side: Side::Local,
        action: Action::Rename,
        requested: 1,
        cancel: Arc::default(),
    };
    let mutation = Action::Rename.mutation(&entry, "moved.txt");
    let path = entry.path.clone();
    app.transfers.reset();
    app.transfers.panes[Side::Local].browser.needs_load = false;
    app.finish_transfer_files(
        operation,
        vec![Completed {
            entry,
            mutation,
            result: Ok(Response {
                path: root.path().join("moved.txt").to_string_lossy().into(),
                ..Default::default()
            }),
        }],
        &ctx,
    );
    assert_eq!(app.documents[0].path.as_deref(), Some(path.as_str()));
    assert!(!app.transfers.panes[Side::Local].browser.needs_load);
}

#[test]
fn rename_cannot_reuse_a_deleted_files_retained_editor_binding() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.txt");
    std::fs::write(&source, "source").unwrap();
    let (mut app, ctx) = fixture(root.path());
    app.new_file();
    app.documents[0].fixture_content().path =
        Some(root.path().join("retained.txt").to_string_lossy().into());
    app.transfer_file_form(Side::Local, Action::Rename, entries(&app, "source.txt"));
    app.transfers.file_form.as_mut().unwrap().name = "retained.txt".into();
    app.start_transfer_files(&ctx);
    assert!(app.transfers.file_operation.is_none());
    assert!(source.exists());
    assert!(!root.path().join("retained.txt").exists());
}

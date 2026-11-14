//! Pure path bookkeeping, async reply identity and modal focus regressions.
use super::model::{flatten_tree, rename_directories};
use super::*;
use std::collections::BTreeMap;

fn entry(path: &str, dir: bool) -> Entry {
    Entry {
        path: path.into(),
        name: ssh::display_name(path).into(),
        dir,
    }
}

struct Policies(String);

#[test]
fn a_failed_file_operation_cancels_a_project_wait_without_discarding_buffers() {
    let directory = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].text = "preserve this source".into();
    app.explorer_state.busy = true;
    app.guarded(
        Action::Project {
            path: directory.path().to_string_lossy().into(),
            remote: false,
            options: Default::default(),
        },
        &ctx,
    );
    assert!(app.pending.modal());
    app.finish_explorer(
        Change {
            kind: Kind::Delete,
            entry: entry(&directory.path().join("entry.txt").to_string_lossy(), false),
        },
        Err("file operation refused".into()),
        &ctx,
    );
    app.advance_guarded_action(&ctx);
    assert!(app.pending.is_none());
    assert!(!app.workspace.preparing());
    assert_eq!(app.documents[0].text, "preserve this source");
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .0
            .contains("file operation refused")
    );
}

#[test]
fn folder_disclosure_uses_vector_icons_and_aligned_filename_labels() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.explorer_state.directories.insert(
        "/project".into(),
        vec![
            entry("/project/open", true),
            entry("/project/shut", true),
            entry("/project/file.rs", false),
        ],
    );
    app.explorer_state
        .directories
        .insert("/project/open".into(), vec![]);
    app.explorer_state.expanded.insert("/project/open".into());
    for width in [180., 450.] {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 400.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.tree(ui, "/project", ctx));
            },
        );
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels
                .iter()
                .map(|text| text.galley.text())
                .collect::<Vec<_>>(),
            ["open", "shut", "file.rs"]
        );
        assert!(labels.iter().all(|text| text.pos.x == labels[0].pos.x));
        let chevrons: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Path(path) if path.points.len() == 3 => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(chevrons.len(), 2);
        assert!(chevrons.iter().all(|path| {
            path.points
                .iter()
                .all(|point| ctx.content_rect().contains(*point))
        }));
    }
}

impl eframe::Storage for Policies {
    fn get_string(&self, key: &str) -> Option<String> {
        (key == "project_preferences").then(|| self.0.clone())
    }
    fn set_string(&mut self, _: &str, _: String) {}
    fn flush(&mut self) {}
}

#[test]
fn selected_project_picker_preserves_host_policy_and_edits_until_confirmation() {
    for remote in [false, true] {
        for saved in [false, true] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            let inherited = sandbox::Options {
                enabled: false,
                share_network: false,
                ..Default::default()
            };
            app.workspace.set_fixture(Some(Workspace {
                root: "/project".into(),
                remote: remote.then(|| {
                    Remote::new(Profile {
                        host: "picker-fixture.invalid".into(),
                        ..Default::default()
                    })
                    .unwrap()
                }),
                connected: true,
                ready: true,
                plan: Default::default(),
                sandbox_options: inherited.clone(),
            }));
            let path = "/project/selected folder";
            let saved_options = sandbox::Options {
                enabled: true,
                backend: sandbox::Backend::Landlock,
                read_only: vec!["/shared/tools".into()],
                ..Default::default()
            };
            if saved {
                let key = app.project_settings_key(path, remote);
                let storage = Policies(
                    serde_json::to_string(&BTreeMap::from([(
                        key,
                        serde_json::json!({"sandbox": saved_options}),
                    )]))
                    .unwrap(),
                );
                app.workbench = workbench::Workbench::load(Some(&storage));
            }
            app.new_file();
            app.documents[0].text = "unsaved edits".into();
            let token = app.workspace.preparation_identity();
            let defaults = app.sandbox_defaults.clone();
            app.open_folder_dialog_at(path.into());
            assert!(app.dialog == Dialog::Folder);
            assert_eq!(app.project_picker.path, path);
            assert_eq!(app.project_picker.policy_path, path);
            assert_eq!(app.project_picker.remote, remote);
            assert!(app.project_picker.needs_load);
            assert_eq!(
                app.project_picker.sandbox,
                if saved { saved_options } else { inherited }
            );
            assert_eq!(app.sandbox_defaults, defaults);
            assert!(app.pending.is_none() && !app.workspace.preparing());
            assert_eq!(app.workspace.preparation_identity(), token);
            assert_eq!(app.workspace.as_ref().unwrap().root, "/project");
            assert!(app.documents[0].dirty());

            // Listing the prefilled folder must not replace choices made while
            // that asynchronous request was in flight, even with saved settings.
            let options = app.project_picker.sandbox.clone();
            app.project_picker.sandbox.enabled = true;
            app.project_picker.sandbox.share_network = false;
            app.project_picker.sandbox.project_read_only = true;
            let chosen = app.project_picker.sandbox.clone();
            app.project_picker.needs_load = false;
            app.project_picker.loading = true;
            app.workers
                .workspace(&ctx)
                .send(WorkspaceEvent::Done {
                    job: Job::Browse {
                        remote,
                        token: app.project_picker.token,
                        path: path.into(),
                        options,
                    },
                    result: Box::new(Ok(files::Response {
                        path: path.into(),
                        entries: vec![entry("/project/selected folder/child", true)],
                        ..Default::default()
                    })),
                })
                .unwrap();
            app.poll(&ctx);
            assert_eq!(app.project_picker.sandbox, chosen);
            assert_eq!(app.project_picker.entries.len(), 1);

            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1380., 1100.),
                    )),
                    events: vec![egui::Event::Key {
                        key: Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::COMMAND,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.folder_dialog(ui, ctx));
                },
            );
            assert!(
                matches!(app.pending.as_ref(), Some(Action::Project { path: selected, remote: selected_remote, options }) if selected == path && *selected_remote == remote && options == &chosen)
            );
            assert_eq!(app.workspace.preparation_identity(), token);
            assert_eq!(app.workspace.as_ref().unwrap().root, "/project");
            assert_eq!(app.documents[0].text, "unsaved edits");
            assert!(!app.workspace.preparing());
        }
    }
}

#[test]
fn canonical_folder_replies_restore_saved_policy_only_before_the_user_edits_it() {
    for edited in [false, true] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let alias = "/project/linked-folder";
        let canonical = "/shared/actual-folder";
        let saved = sandbox::Options {
            enabled: false,
            share_network: true,
            project_read_only: false,
            ..Default::default()
        };
        let storage = Policies(
            serde_json::to_string(&BTreeMap::from([(
                app.project_settings_key(canonical, false),
                serde_json::json!({"sandbox": saved}),
            )]))
            .unwrap(),
        );
        app.workbench = workbench::Workbench::load(Some(&storage));
        app.open_folder_dialog_at(alias.into());
        let browse = app.project_picker.begin(alias.into());
        if edited {
            app.project_picker.sandbox.share_network = false;
            app.project_picker.sandbox.project_read_only = true;
        }
        let chosen = app.project_picker.sandbox.clone();
        app.project_picker.needs_load = false;
        app.workers
            .workspace(&ctx)
            .send(WorkspaceEvent::Done {
                job: Job::Browse {
                    remote: false,
                    token: browse.token,
                    path: browse.path,
                    options: browse.options,
                },
                result: Box::new(Ok(files::Response {
                    path: canonical.into(),
                    ..Default::default()
                })),
            })
            .unwrap();
        app.poll(&ctx);
        assert_eq!(app.project_picker.path, canonical);
        assert_eq!(app.project_picker.policy_path, canonical);
        assert_eq!(
            app.project_picker.sandbox,
            if edited { chosen } else { saved }
        );
        assert!(!app.project_picker.loading);
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
    app.workspace.set_fixture(Some(Workspace {
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
    }));
    assert!(!app.explorer_writable());
    app.workspace.as_mut().unwrap().plan.active = false;
    assert!(
        app.explorer_writable(),
        "Unavailable sandbox options are not an active read-only policy"
    );
    app.workspace.begin_preparation();
    assert!(!app.explorer_writable());
    app.workspace.cancel_preparation();
    app.explorer_state.busy = true;
    assert!(!app.explorer_writable());
    app.explorer_state.busy = false;
    app.documents.begin_open("/project/file".into());
    assert!(!app.explorer_writable());
    app.documents.cancel_opens();
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
    let token = app.workspace.preparation_identity();
    app.explorer_state.busy = true;
    app.prepare_project(
        "/project".into(),
        Default::default(),
        false,
        true,
        None,
        &ctx,
    );
    assert!(!app.workspace.preparing());
    assert_eq!(app.workspace.preparation_identity(), token);
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
fn a_prefilled_project_dialog_recovers_focus_after_its_context_menu_closes() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.open_folder_dialog_at("/selected/folder".into());
    app.project_picker.needs_load = false;
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
    assert!(ctx.memory(|memory| memory.has_focus(egui::Id::new("project-dialog-path"))));
    assert_eq!(app.project_picker.path, "/selected/folder");
    assert!(app.workspace.is_none() && app.pending.is_none());
}

#[test]
fn stale_directory_replies_cannot_restore_rows_or_clear_new_loading_state() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.explorer_state.epoch = 2;
    app.explorer_state.loading.insert("/project".into());
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::List("/project".into(), 1),
            result: Box::new(Ok(Response {
                path: "/project".into(),
                entries: vec![entry("/project/stale", false)],
                ..Default::default()
            })),
        })
        .unwrap();
    app.poll(&ctx);
    assert!(app.explorer_state.directories.is_empty());
    assert!(app.explorer_state.loading.contains("/project"));
}

#[test]
fn a_confirmation_dialog_keeps_its_enter_key_instead_of_toggling_a_tree_row() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let folder = entry("/project/folder", true);
    app.explorer_state.tree_rows.push(TreeRow {
        entry: folder.clone(),
        depth: 0,
    });
    app.explorer_state.expanded.insert(folder.path.clone());
    app.explorer_state.selection.path = Some(folder.path.clone());
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
    assert!(app.explorer_state.expanded.contains(&folder.path));
}

#[test]
fn focused_delete_confirmations_submit_one_enter_and_remove_files_or_nonempty_folders() {
    for directory in [false, true] {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().to_string_lossy().into_owned();
        let target = base.path().join("target ' with spaces");
        if directory {
            std::fs::create_dir(&target).unwrap();
            std::fs::write(target.join("child.txt"), "saved contents").unwrap();
        } else {
            std::fs::write(&target, "saved contents").unwrap();
        }
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
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
        app.explorer_form(Kind::Delete, entry(target.to_str().unwrap(), directory));
        let mut render = |events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1380., 900.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| app.dialogs(ctx),
            );
        };
        for _ in 0..3 {
            render(vec![]);
        }
        assert!(ctx.memory(|memory| memory.focused().is_some()));
        render(
            [true, false]
                .map(|pressed| egui::Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Default::default(),
                })
                .into(),
        );
        assert!(app.dialog == Dialog::None);
        assert!(app.explorer_state.busy);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.explorer_state.busy {
            app.poll(&ctx);
            assert!(std::time::Instant::now() < until, "{}", app.status);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!target.exists(), "{}", app.status);
    }
}

#[test]
fn a_deleted_files_retained_editor_blocks_reusing_its_path() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().to_string_lossy().into_owned();
    let path = base.path().join("retained.txt");
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace.set_fixture(Some(Workspace {
        root: root.clone(),
        remote: None,
        connected: true,
        ready: true,
        plan: Default::default(),
        sandbox_options: Default::default(),
    }));
    app.new_file();
    app.documents[0].path = Some(path.to_string_lossy().into_owned());
    app.documents[0].text = "retained unsaved contents".into();
    app.documents[0].set_dirty_fixture(true);
    app.explorer_form(Kind::File, entry(&root, true));
    app.explorer_state.input = "retained.txt".into();
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

#[test]
fn explorer_flattens_only_expanded_visible_folders() {
    let entries = |path: &str, dir| Entry {
        path: path.into(),
        name: ssh::display_name(path).into(),
        dir,
    };
    let directories = HashMap::from([
        (
            "/root".into(),
            vec![
                entries("/root/src", true),
                entries("/root/.git", true),
                entries("/root/main.rs", false),
            ],
        ),
        ("/root/src".into(), vec![entries("/root/src/lib.rs", false)]),
    ]);
    let mut rows = vec![];
    flatten_tree(&directories, &HashSet::new(), "/root", 0, false, &mut rows);
    assert_eq!(rows.len(), 2);
    rows.clear();
    flatten_tree(
        &directories,
        &HashSet::from(["/root/src".into()]),
        "/root",
        0,
        true,
        &mut rows,
    );
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[1].depth, 1);
}

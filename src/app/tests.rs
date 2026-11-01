//! Editor lifecycle, document and appearance regressions.

use super::*;

#[test]
fn local_startup_opens_files_and_directories_and_saves_to_the_original_file() {
    // Relative input stays relative to the process cwd without changing that
    // shared cwd while other tests run. Include spaces, Unicode and a symlink.
    let directory = tempfile::Builder::new()
        .prefix(".rode-startup-")
        .tempdir_in(".")
        .unwrap();
    let file = directory.path().join("individual file λ.txt");
    std::fs::write(&file, "startup contents\n").unwrap();
    let absolute_file = file.canonicalize().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let link = directory.path().join("linked-file.txt");
    std::os::unix::fs::symlink(&absolute_file, &link).unwrap();

    for path in [
        directory.path().to_owned(),
        root.clone(),
        file.clone(),
        absolute_file.clone(),
        link,
    ] {
        let is_file = path.is_file();
        std::fs::write(&file, "startup contents\n").unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(
            &cc,
            Startup {
                local: Some(path.clone()),
                ..Default::default()
            },
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.preparing || !app.opening.is_empty() {
            app.poll(&ctx);
            assert!(
                Instant::now() < deadline,
                "Startup did not finish: {path:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let workspace = app
            .workspace
            .as_ref()
            .unwrap_or_else(|| panic!("Startup failed for {path:?}: {:?}", app.notice));
        assert_eq!(workspace.root, root.to_string_lossy());
        assert!(workspace.ready && workspace.connected);
        assert_eq!(app.documents.len(), usize::from(is_file));
        if is_file {
            let doc = &mut app.documents[app.active];
            assert_eq!(doc.path.as_deref(), absolute_file.to_str());
            assert_eq!(doc.text, "startup contents\n");
            assert!(!doc.dirty());
            assert_eq!(app.workbench.surface, workbench::Surface::Editor);
            doc.text.push_str("edited\n");
            doc.modified = true;
            doc.edit_version += 1;
            app.save_active(&ctx, false);
            while app.documents[app.active].saving {
                app.poll(&ctx);
                assert!(Instant::now() < deadline, "Save did not finish: {path:?}");
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                "startup contents\nedited\n"
            );
            assert!(!app.documents[app.active].dirty());
        }
    }
}

#[test]
fn local_startup_missing_path_reports_an_error_without_creating_a_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing.txt");
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(
        &cc,
        Startup {
            local: Some(path.clone()),
            ..Default::default()
        },
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.preparing {
        app.poll(&ctx);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.workspace.is_none() && app.documents.is_empty());
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(text, error, _)| { *error && text.contains("missing.txt") })
    );
    assert!(!path.exists());
}

#[test]
fn extensions_shortcut_handles_native_cut_events_without_consuming_plain_cut() {
    let shortcut = egui::Modifiers::CTRL | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    for (modifiers, events, opens, cut_remains) in [
        (shortcut, vec![egui::Event::Cut], true, false),
        (
            egui::Modifiers::NONE,
            vec![
                egui::Event::Cut,
                egui::Event::Key {
                    key: Key::X,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: shortcut,
                },
            ],
            true,
            false,
        ),
        (
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            vec![egui::Event::Cut],
            false,
            true,
        ),
        (
            shortcut | egui::Modifiers::ALT,
            vec![egui::Event::Cut],
            false,
            true,
        ),
    ] {
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                modifiers,
                events,
                ..Default::default()
            },
            |ctx| {
                assert_eq!(consume_extensions_shortcut(ctx), opens);
                assert_eq!(
                    ctx.input(|i| i.events.contains(&egui::Event::Cut)),
                    cut_remains
                );
            },
        );
    }
}

#[test]
fn terminal_navigation_reaches_the_pty_on_the_first_frame_after_focus() {
    use portable_pty::CommandBuilder;
    for (key, modifiers, bytes) in [
        (Key::Tab, egui::Modifiers::NONE, b"\t".as_slice()),
        (Key::Tab, egui::Modifiers::SHIFT, b"\x1b[Z".as_slice()),
        (Key::ArrowUp, egui::Modifiers::NONE, b"\x1b[A".as_slice()),
        (Key::ArrowLeft, egui::Modifiers::NONE, b"\x1b[D".as_slice()),
        (Key::Escape, egui::Modifiers::NONE, b"\x1b".as_slice()),
    ] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        let mut command = CommandBuilder::new("python3");
        command.args(["-u", "-c", &format!("import sys,tty;tty.setraw(0);print('READY',flush=True);data=sys.stdin.buffer.read({});print('BYTES_'+data.hex(),flush=True)", bytes.len())]);
        app.terminal = Some(Terminal::spawn(&ctx, command, "Input recorder".into()).unwrap());
        app.show_terminal = true;
        app.terminal_intent();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.terminal.as_ref().unwrap().contents().contains("READY") {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let frame = |time| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(850., 560.),
            )),
            time: Some(time),
            ..Default::default()
        };
        let _ = ctx.run(frame(0.), |ctx| {
            app.workspace_content(ctx);
            Terminal::focus(ctx);
        });
        let mut input = frame(0.1);
        input.modifiers = modifiers;
        input.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        let _ = ctx.run(input, |ctx| {
            app.shortcuts(ctx);
            app.workspace_content(ctx);
        });
        // Back-tab navigation can schedule a focus change for the next frame.
        let _ = ctx.run(frame(0.2), |ctx| {
            app.shortcuts(ctx);
            app.workspace_content(ctx);
        });
        assert!(
            Terminal::focused(&ctx),
            "{modifiers:?}+{key:?} moved terminal focus into editor controls"
        );
        let expected = format!(
            "BYTES_{}",
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app
            .terminal
            .as_ref()
            .unwrap()
            .contents()
            .contains(&expected)
        {
            assert!(
                Instant::now() < deadline,
                "{modifiers:?}+{key:?} did not reach the PTY"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.documents[0].text.is_empty());
    }
}

#[test]
fn edits_made_while_a_project_opens_are_not_silently_discarded() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.project_token = 1;
    app.preparing = true;
    // The request started with no buffers, but the user edited while waiting.
    app.new_file();
    app.documents[0].text = "typed during slow project opening".into();
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("startup.txt");
    std::fs::write(&file, "open after confirmation").unwrap();
    let response = files::local(&Request::Prepare {
        path: project.path().to_string_lossy().into(),
        sandbox: Default::default(),
    })
    .unwrap();
    app.tx
        .send(Event::Done {
            generation: app.generation,
            job: Job::Prepare {
                remote: false,
                options: Default::default(),
                keep_buffers: false,
                token: 1,
                documents_at_start: vec![],
                restore_saved: None,
                open_file: Some(file.to_string_lossy().into_owned()),
            },
            result: Box::new(Ok(response)),
        })
        .unwrap();
    app.poll(&ctx);
    assert!(matches!(
        &app.pending,
        Some(Action::PreparedProject { open_file: Some(path), .. }) if path == &file.to_string_lossy()
    ));
    assert_eq!(app.documents[0].text, "typed during slow project opening");
    assert!(app.workspace.is_none());
    app.pending = None; // Keep editing.
    assert_eq!(app.documents.len(), 1);
}

#[test]
fn file_operations_wait_until_the_requested_project_environment_is_ready() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let mut workspace = remote_workspace();
    workspace.ready = false;
    app.workspace = Some(workspace);
    app.new_file();
    app.documents[0].text = "keep edits".into();
    app.save_document(app.documents[0].id, "/not-written.rs".into(), true, &ctx);
    assert!(!app.documents[0].saving);
    assert_eq!(app.documents[0].text, "keep edits");
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(text, _, _)| text.contains("environment to be ready"))
    );
    app.open_file("/not-read.rs".into(), &ctx);
    assert!(app.opening.is_empty());
    assert_eq!(app.documents.len(), 1);
}

pub(super) fn remote_workspace() -> Workspace {
    let remote = Remote::new(Profile {
        host: "test-host".into(),
        user: "researcher".into(),
        ..Default::default()
    })
    .unwrap();
    Workspace {
        root: "/scratch/researcher/project".into(),
        remote: Some(remote),
        connected: true,
        ready: true,
        plan: Default::default(),
        sandbox_options: Default::default(),
    }
}

#[test]
fn disconnect_preserves_dirty_remote_buffers_and_invalidates_host_work() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace = Some(remote_workspace());
    let remote = app.workspace.as_ref().unwrap().remote.clone().unwrap();
    std::fs::write(&remote.socket, "owned connection fixture").unwrap();
    app.new_file();
    let doc = &mut app.documents[0];
    doc.path = Some("/scratch/researcher/project/main.rs".into());
    doc.revision = Some("remote revision".into());
    doc.text = "unsaved remote edits".into();
    doc.modified = true;
    doc.edit_version = 3;
    doc.selection = Some((2, 5));
    let generation = app.generation;
    let project_token = app.project_token;
    let folder_token = app.folder_token;
    let cancel = app.connection_cancel.clone();
    app.preparing = true;
    app.disconnect_ssh(&ctx);
    assert!(app.workspace.is_none());
    assert!(!app.project_ready());
    assert!(!app.preparing);
    assert!(cancel.load(Ordering::Acquire));
    assert!(!remote.socket.exists());
    assert!(app.generation > generation);
    assert!(app.project_token > project_token);
    assert!(app.folder_token > folder_token);
    assert_eq!(app.documents[0].text, "unsaved remote edits");
    assert!(app.documents[0].dirty());
    assert_eq!(app.documents[0].edit_version, 3);
    assert_eq!(app.documents[0].selection, Some((2, 5)));
    assert!(app.documents[0].path.is_none());
    assert!(app.documents[0].revision.is_none());
    assert_eq!(app.documents[0].name(), "main.rs");
    assert_eq!(
        app.documents[0].display_path(),
        Some("/scratch/researcher/project/main.rs")
    );
    assert!(app.pending.is_none());
    assert!(app.ssh_master.is_none());
    assert!(app.terminal.is_none());
    app.tx
        .send(Event::Connected {
            generation,
            result: Ok(()),
        })
        .unwrap();
    app.poll(&ctx);
    assert!(app.workspace.is_none());
    app.set_dialog(Dialog::Folder);
    assert!(!app.folder_remote);
    assert_eq!(
        app.dialog_input,
        std::env::current_dir().unwrap().to_string_lossy()
    );
    app.workspace_banner(&ctx);
    assert_eq!(app.last_window_title, "RODE");
    app.save_document(app.documents[0].id, "/not-local.rs".into(), false, &ctx);
    assert!(!app.documents[0].saving);
    assert_eq!(app.documents[0].text, "unsaved remote edits");
}

#[test]
fn open_folder_defaults_to_the_remote_host_and_local_open_is_explicit() {
    let cc = eframe::CreationContext::_new_kittest(egui::Context::default());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace = Some(remote_workspace());
    let socket = app
        .workspace
        .as_ref()
        .unwrap()
        .remote
        .as_ref()
        .unwrap()
        .socket
        .clone();
    app.set_dialog(Dialog::Folder);
    assert!(app.folder_remote);
    assert_eq!(app.dialog_input, "/scratch/researcher/project");
    app.open_local_folder_dialog();
    assert!(!app.folder_remote);
    assert_eq!(
        app.workspace
            .as_ref()
            .unwrap()
            .remote
            .as_ref()
            .unwrap()
            .socket,
        socket
    );
}

#[test]
fn project_switch_still_protects_unsaved_buffers() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace = Some(remote_workspace());
    app.new_file();
    app.documents[0].text = "unsaved remote edits".into();
    app.guarded(
        Action::Project {
            path: "/scratch/other".into(),
            remote: true,
            options: Default::default(),
        },
        &ctx,
    );
    assert!(app.pending.is_some());
    assert!(!app.preparing);
    assert_eq!(app.documents[0].text, "unsaved remote edits");
    assert_eq!(
        app.workspace.as_ref().unwrap().root,
        "/scratch/researcher/project"
    );
}

#[test]
fn late_folder_browse_cannot_overwrite_another_dialog_or_user_input() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.set_dialog(Dialog::Folder);
    let old = app.dialog_input.clone();
    let token = app.folder_token;
    app.set_dialog(Dialog::Open);
    app.dialog_input = "keep-this.rs".into();
    let response = files::Response {
        path: "/old/folder".into(),
        image: None,
        binary: None,
        pdf: None,
        hex: None,
        attachment: None,
        notebook: None,
        preview: None,
        entries: vec![],
        text: String::new(),
        revision: String::new(),
        project: None,
        tool: None,
    };
    app.tx
        .send(Event::Done {
            generation: app.generation,
            job: Job::Browse {
                remote: false,
                token,
                path: old,
            },
            result: Box::new(Ok(response.clone())),
        })
        .unwrap();
    app.poll(&ctx);
    assert_eq!(app.dialog_input, "keep-this.rs");
    app.set_dialog(Dialog::Folder);
    let old = app.dialog_input.clone();
    let token = app.folder_token;
    app.dialog_input = "/newly/typed/path".into();
    app.tx
        .send(Event::Done {
            generation: app.generation,
            job: Job::Browse {
                remote: false,
                token,
                path: old,
            },
            result: Box::new(Ok(response)),
        })
        .unwrap();
    app.poll(&ctx);
    assert_eq!(app.dialog_input, "/newly/typed/path");
}

#[derive(Default)]
struct MemoryStorage(HashMap<String, String>, usize);

impl eframe::Storage for MemoryStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn flush(&mut self) {
        self.1 += 1;
    }
}

#[test]
fn interface_scale_tracks_monitor_dpi_and_preserves_editor_state() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].text = "unsaved work".into();
    app.font_size = 19.;
    app.set_ui_scale(0.75, &ctx);
    let mut storage = MemoryStorage::default();
    app.persist_ui_scale(Some(&mut storage));
    assert_eq!(storage.0["ui_scale"], "0.75");
    assert_eq!(storage.1, 1);
    app.persist_ui_scale(Some(&mut storage));
    assert_eq!(storage.1, 1, "Idle frames must not save scale repeatedly");
    for native_dpi in [1., 2.] {
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(native_dpi);
        let _ = ctx.run(input, |ctx| {
            assert_eq!(ctx.pixels_per_point(), native_dpi * 0.75);
        });
    }
    app.select_theme("carbon", &ctx);
    assert_eq!(ctx.zoom_factor(), 0.75);
    assert_eq!(app.font_size, 19.);
    assert_eq!(app.documents[0].text, "unsaved work");
    assert!(app.documents[0].dirty());
    let restored_ctx = egui::Context::default();
    let mut cc = eframe::CreationContext::_new_kittest(restored_ctx.clone());
    // Ignore stale scale in egui's private GUI snapshot on restart.
    restored_ctx.options_mut(|options| options.zoom_factor = 2.);
    cc.storage = Some(&storage);
    let restored = EditorApp::new(&cc, Startup::default());
    assert_eq!(restored.ui_scale, 0.75);
    let _ = restored_ctx.run(Default::default(), |ctx| {
        assert_eq!(ctx.zoom_factor(), 0.75);
        assert!(!ctx.options(|options| options.zoom_with_keyboard));
    });
    assert!(!restored.ui_scale_save_pending);
}

#[test]
fn invalid_interface_scale_uses_system_display_scaling() {
    for invalid in ["NaN", "inf", "bad", "0", "-1", "5"] {
        let mut storage = MemoryStorage::default();
        storage.0.insert("ui_scale".into(), invalid.into());
        let mut cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        cc.storage = Some(&storage);
        let app = EditorApp::new(&cc, Startup::default());
        assert_eq!(app.ui_scale, 1., "{invalid}");
    }
}

#[test]
fn theme_change_flushes_preferences_without_exiting_or_waiting_for_autosave() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let mut storage = MemoryStorage::default();
    storage.0.insert("font_size".into(), "19".into());
    app.persist_theme_choice(Some(&mut storage));
    assert_eq!(storage.1, 0);
    app.select_theme("carbon", &ctx);
    app.persist_theme_choice(Some(&mut storage));
    assert_eq!(storage.0[theme::STORAGE_KEY], "carbon");
    assert_eq!(storage.0["font_size"], "19");
    assert_eq!(storage.1, 1);
    app.persist_theme_choice(Some(&mut storage));
    assert_eq!(
        storage.1, 1,
        "Unchanged frames must not repeatedly flush storage"
    );
    app.select_theme("phosphor", &ctx);
    app.persist_theme_choice(Some(&mut storage));
    assert_eq!(storage.0[theme::STORAGE_KEY], "phosphor");
    assert_eq!(storage.1, 2);
}

#[test]
fn missing_preference_storage_reports_that_the_theme_could_not_be_saved() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.select_theme("carbon", &ctx);
    app.persist_theme_choice(None);
    assert_eq!(app.themes.active().id, "carbon");
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(text, error, _)| *error && text.contains("could not be saved"))
    );
    assert!(!app.theme_save_pending);
}

#[test]
fn theme_choice_persists_without_touching_unsaved_edits_or_font_size() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].text = "unsaved work".into();
    app.font_size = 19.0;
    app.select_theme("carbon", &ctx);
    assert_eq!(app.documents.len(), 1);
    assert_eq!(app.documents[0].text, "unsaved work");
    assert!(app.documents[0].dirty());
    assert_eq!(app.font_size, 19.0);
    let mut storage = MemoryStorage::default();
    eframe::App::save(&mut app, &mut storage);
    let mut cc = eframe::CreationContext::_new_kittest(egui::Context::default());
    cc.storage = Some(&storage);
    let restored = EditorApp::new(&cc, Startup::default());
    assert_eq!(restored.themes.active().id, "carbon");
    assert_eq!(restored.font_size, 19.0);
    let overridden = EditorApp::new(
        &cc,
        Startup {
            theme: Some("phosphor".into()),
            ..Default::default()
        },
    );
    assert_eq!(overridden.themes.active().id, "phosphor");
}

#[test]
fn syntax_selection_saves_only_its_own_preference_and_restores_independently() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].text = "unsaved text".into();
    app.font_size = 19.;
    let mut storage = MemoryStorage::default();
    storage.0.insert(theme::STORAGE_KEY.into(), "neon".into());
    app.select_syntax_theme("dracula", &ctx);
    app.persist_theme_choice(Some(&mut storage));
    assert_eq!(storage.0[theme::syntax::STORAGE_KEY], "dracula");
    assert_eq!(storage.0[theme::STORAGE_KEY], "neon");
    assert_eq!(storage.1, 1);
    app.persist_theme_choice(Some(&mut storage));
    assert_eq!(storage.1, 1);
    assert_eq!(app.documents[0].text, "unsaved text");
    assert_eq!(app.font_size, 19.);
    assert!(app.documents[0].dirty());
    app.select_theme("carbon", &ctx);
    app.persist_theme_choice(Some(&mut storage));
    assert_eq!(storage.0[theme::syntax::STORAGE_KEY], "dracula");
    assert_eq!(app.themes.active_syntax().id, "dracula");
    let mut cc = eframe::CreationContext::_new_kittest(egui::Context::default());
    cc.storage = Some(&storage);
    let restored = EditorApp::new(&cc, Startup::default());
    assert_eq!(restored.themes.active().id, "carbon");
    assert_eq!(restored.themes.active_syntax().id, "dracula");
    let override_app = EditorApp::new(
        &cc,
        Startup {
            syntax_theme: Some("nord".into()),
            ..Default::default()
        },
    );
    assert_eq!(override_app.themes.active().id, "carbon");
    assert_eq!(override_app.themes.active_syntax().id, "nord");
    let fallback = EditorApp::new(
        &cc,
        Startup {
            syntax_theme: Some("absent".into()),
            ..Default::default()
        },
    );
    assert_eq!(fallback.themes.active_syntax().id, "ui");
    assert!(fallback.notice.is_some());
}

#[test]
fn save_acknowledgement_keeps_edits_typed_during_save() {
    let mut document = Document {
        recovery: None,
        markdown: None,
        marp: None,
        image: None,
        pdf: None,
        hex: None,
        notebook: None,
        id: 1,
        path: Some("file.rs".into()),
        text: "newer edits".into(),
        saved_text: "original".into(),
        revision: Some("old".into()),
        saving: true,
        modified: true,
        edit_version: 0,
        last_cursor: usize::MAX,
        cursor: (1, 1),
        selection: None,
        lines: 1,
    };
    document.acknowledge_save(
        "file.rs".into(),
        "submitted edits".into(),
        "new revision".into(),
    );
    assert!(document.dirty());
    assert!(!document.saving);
    assert_eq!(document.text, "newer edits");
    assert_eq!(document.revision.as_deref(), Some("new revision"));
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

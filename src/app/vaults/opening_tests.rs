//! Shared project opening must retain encryption's admission and secret handling
//! without requiring an installed encryption tool or kernel mounts.
use super::*;

fn app() -> (EditorApp, egui::Context) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    (EditorApp::new(&cc, Startup::default()), ctx)
}

#[test]
fn explorer_and_encrypted_command_use_the_standard_picker_without_opening_storage() {
    let (mut app, _) = app();
    app.new_file();
    app.documents
        .active_mut()
        .unwrap()
        .set_source("keep this draft".into());
    app.vaults.create = true;
    app.encrypted_dialog_at("/selected encrypted folder".into());
    assert!(app.dialog == Dialog::Folder && app.project_picker.encrypted);
    assert_eq!(app.project_picker.path, "/selected encrypted folder");
    assert!(!app.vaults.create);
    assert!(app.vaults.pending.is_none() && app.vaults.active.is_none());
    assert_eq!(app.documents.active().unwrap().text, "keep this draft");
    assert!(app.documents.active().unwrap().dirty());
}

#[test]
fn encrypted_submission_captures_the_full_shared_sandbox_draft() {
    let (mut app, ctx) = app();
    app.encrypted_dialog_at("/cipher".into());
    let options = sandbox::Options {
        enabled: true,
        backend: sandbox::Backend::Landlock,
        project_read_only: true,
        share_network: false,
        read_only: vec!["/shared-config".into()],
        read_write: vec!["/tool-cache".into()],
        ..Default::default()
    };
    // A password mismatch stops before process launch, independently of whether
    // the development host has gocryptfs, FUSE or a native helper installed.
    app.vaults.create = true;
    app.vaults.password = "first".into();
    app.vaults.confirmation = "different".into();
    app.open_encrypted_from_picker("/cipher".into(), false, options.clone(), false, &ctx);
    assert_eq!(app.vaults.options, options);
    assert_eq!(app.vaults.settings.cipherdir, "/cipher");
    assert!(!app.vaults.remote);
    assert!(app.vaults.message.contains("passwords do not match"));
    assert!(app.vaults.pending.is_none() && !app.workspace.preparing());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .0
            .contains("passwords do not match")
    );
}

#[test]
fn cancelling_or_replacing_the_shared_dialog_clears_credentials() {
    for replacement in [Dialog::Open, Dialog::Connect, Dialog::Folder] {
        let (mut app, _) = app();
        app.encrypted_dialog_at("/cipher".into());
        app.vaults.password = "secret".into();
        app.vaults.confirmation = "secret".into();
        app.set_dialog(replacement);
        assert!(app.vaults.password.is_empty() && app.vaults.confirmation.is_empty());
    }
    let (mut app, ctx) = app();
    app.encrypted_dialog_at("/cipher".into());
    app.project_picker.needs_load = false;
    app.vaults.password = "secret".into();
    for _ in 0..3 {
        let _ = ctx.run(Default::default(), |ctx| app.dialogs(ctx));
    }
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        },
        |ctx| app.dialogs(ctx),
    );
    assert!(app.dialog == Dialog::None);
    assert!(app.vaults.password.is_empty() && app.vaults.confirmation.is_empty());
}

#[test]
fn shared_picker_keeps_actions_visible_and_never_publishes_passwords() {
    for (size, scale) in [(egui::vec2(850., 560.), 1.), (egui::vec2(640., 360.), 1.5)] {
        let (mut app, ctx) = app();
        ctx.set_zoom_factor(scale);
        let fixture = tempfile::tempdir().unwrap();
        let probe = fixture.path().join("project-form.json");
        ui_probe::install_at(&ctx, probe.clone());
        app.encrypted_dialog_at("/encrypted".into());
        app.project_picker.needs_load = false;
        app.vaults.password = "never-publish-this-password".into();
        for _ in 0..5 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| app.dialogs(ctx),
            );
        }
        let snapshot = std::fs::read_to_string(probe).unwrap();
        assert!(!snapshot.contains("never-publish-this-password"));
        let value: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
        let rect = |name: &str| {
            let control = value["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .find(|control| control["name"] == name)
                .unwrap_or_else(|| panic!("Missing {name}: {size}/{scale}: {value}"));
            let [x, y, right, bottom]: [f32; 4] =
                serde_json::from_value(control["rect"].clone()).unwrap();
            egui::Rect::from_min_max(egui::pos2(x, y), egui::pos2(right, bottom))
        };
        for name in [
            "project-dialog-open",
            "project-dialog-cancel",
            "project-dialog-path",
        ] {
            assert!(
                ctx.content_rect().contains_rect(rect(name)),
                "Clipped {name}: {size}/{scale}: {value}"
            );
        }
        assert!(!rect("project-dialog-open").intersects(rect("project-dialog-cancel")));
        assert!(!rect("project-dialog-path").intersects(rect("project-dialog-browse")));
    }
}

#[test]
fn closing_a_shared_encrypted_form_releases_pending_unlocks() {
    let (mut app, ctx) = app();
    app.encrypted_dialog_at("/cipher".into());
    app.project_picker.needs_load = false;
    app.vaults.pending = Some(super::tests::fixture_client(
        "import sys\nfor line in sys.stdin: pass\n".into(),
    ));
    app.cancel_encrypted_dialog();
    assert!(app.vaults.pending.is_none());
    assert_eq!(app.vaults.closing.len(), 1);
    app.poll_vaults(&ctx);
    assert!(app.vaults.cleanup_pending());
}

#[test]
fn host_changes_clear_secrets_and_reject_late_folder_replies() {
    let (mut app, ctx) = app();
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    app.encrypted_dialog_at("/cipher".into());
    app.vaults.password = "remote-secret".into();
    app.vaults.confirmation = "remote-secret".into();
    let old = app.project_picker.begin("/cipher".into());
    app.select_project_dialog_host(false);
    assert!(!app.project_picker.remote && app.project_picker.encrypted);
    assert!(app.vaults.password.is_empty() && app.vaults.confirmation.is_empty());
    let selected = app.project_picker.path.clone();
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::Browse {
                token: old.token,
                path: old.path,
                remote: true,
                options: old.options,
            },
            result: Box::new(Ok(Response {
                path: "/obsolete-remote".into(),
                ..Default::default()
            })),
        })
        .unwrap();
    app.poll(&ctx);
    assert_eq!(app.project_picker.path, selected);
    assert!(app.project_picker.entries.is_empty());
}

#[test]
fn disconnected_ssh_selection_cannot_fall_back_to_local_browsing() {
    let (mut app, _) = app();
    app.encrypted_dialog_at("/cipher".into());
    let selected = app.project_picker.path.clone();
    app.select_project_dialog_host(true);
    assert!(!app.project_picker.remote);
    assert_eq!(app.project_picker.path, selected);
    assert!(
        app.project_picker
            .error
            .as_ref()
            .unwrap()
            .contains("Connect SSH")
    );
}

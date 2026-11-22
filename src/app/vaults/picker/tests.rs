use super::*;

#[test]
fn cancelled_reopened_and_changed_host_listings_cannot_replace_the_folder_draft() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.encrypted_dialog_at("/encrypted".into());
    app.vaults.password = "transient password".into();
    app.open_encrypted_folder_picker(&ctx);
    let old = app.vaults.picker.browser.begin("/encrypted");
    assert!(app.vaults.folder_matches(old, "/encrypted", false));
    assert!(!app.vaults.folder_matches(old, "/encrypted", true));
    assert!(app.vaults.cancel_folder_picker());
    assert_eq!(app.vaults.password, "transient password");
    app.open_encrypted_folder_picker(&ctx);
    let current = app.vaults.picker.browser.begin("/encrypted");
    assert_ne!(old, current);
    app.workers
        .workspace(&ctx)
        .send(WorkspaceEvent::Done {
            job: Job::BrowseEncrypted {
                token: old,
                path: "/encrypted".into(),
                remote: false,
            },
            result: Box::new(Ok(Response {
                path: "/obsolete".into(),
                ..Default::default()
            })),
        })
        .unwrap();
    app.poll(&ctx);
    assert_eq!(app.vaults.picker.browser.requested, "/encrypted");
    assert!(app.vaults.picker.browser.directory.is_empty());
    app.vaults.picker.browser.location = "/typed elsewhere".into();
    assert!(!app.vaults.folder_matches(current, "/encrypted", false));
    app.cancel_encrypted_dialog();
    assert!(app.vaults.password.is_empty());
    assert!(!app.vaults.picker.open);
}

#[test]
fn remote_browsing_without_a_connection_never_lists_the_local_host() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.encrypted_dialog();
    app.vaults.remote = true;
    app.open_encrypted_folder_picker(&ctx);
    let token = app.vaults.picker.browser.token;
    app.browse_encrypted_folder("/".into(), &ctx);
    assert_eq!(app.vaults.picker.browser.token, token);
    assert!(app.vaults.picker.browser.entries.is_empty());
    assert!(
        app.vaults
            .picker
            .browser
            .error
            .as_deref()
            .unwrap()
            .contains("Connect SSH")
    );
    assert!(!app.vaults.picker.browser.loading);
}

#[test]
fn explorer_prefills_storage_without_creating_unlocking_or_replacing_the_project() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents
        .active_mut()
        .unwrap()
        .set_source("keep this draft".into());
    app.vaults.create = true;
    app.encrypted_dialog_at("/selected encrypted folder".into());
    assert!(app.dialog == Dialog::Encrypted);
    assert_eq!(app.vaults.settings.cipherdir, "/selected encrypted folder");
    assert!(!app.vaults.create);
    assert!(app.vaults.pending.is_none() && app.vaults.active.is_none());
    assert_eq!(app.documents.active().unwrap().text, "keep this draft");
    assert!(app.documents.active().unwrap().dirty());
}

#[test]
fn form_and_browser_keep_actions_visible_and_paths_clear_of_buttons() {
    for browsing in [false, true] {
        for (size, scale) in [(egui::vec2(850., 560.), 1.), (egui::vec2(640., 360.), 1.5)] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            ctx.set_pixels_per_point(scale);
            let fixture = tempfile::tempdir().unwrap();
            let probe = fixture.path().join("vault-form.json");
            ui_probe::install_at(&ctx, probe.clone());
            app.encrypted_dialog_at("/encrypted".into());
            app.vaults.password = "do-not-publish-this-password".into();
            if browsing {
                app.open_encrypted_folder_picker(&ctx);
                app.vaults.picker.browser.begin("/encrypted");
                app.vaults.folder_loaded(Response {
                    path: "/encrypted".into(),
                    entries: (0..50)
                        .map(|n| Entry {
                            path: format!("/encrypted/folder-{n}"),
                            name: format!("folder-{n}"),
                            dir: true,
                        })
                        .collect(),
                    ..Default::default()
                });
            }
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
            assert!(!snapshot.contains("do-not-publish-this-password"));
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
            let (submit, cancel, path, browse) = if browsing {
                (
                    "vault-picker-select",
                    "vault-picker-cancel",
                    "vault-picker-path",
                    "vault-picker-go",
                )
            } else {
                (
                    "vault-unlock",
                    "vault-cancel",
                    "vault-folder",
                    "vault-browse",
                )
            };
            for name in [submit, cancel] {
                assert!(
                    ctx.content_rect().contains_rect(rect(name)),
                    "Clipped {name}: {size}/{scale}: {value}"
                );
            }
            assert!(!rect(submit).intersects(rect(cancel)));
            assert!(
                !rect(path).intersects(rect(browse)),
                "Folder field overlaps Browse"
            );
            assert!((rect(path).center().y - rect(browse).center().y).abs() < 2.);
        }
    }
}

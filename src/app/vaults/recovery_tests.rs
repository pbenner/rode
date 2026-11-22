//! Interrupted coordinator recovery must close the retained mount, not reopen a project.
//! Protocol peers need neither gocryptfs nor FUSE; mount verification has host fixtures.
use super::tests::{active_reply, fixture_client};
use super::*;

fn interrupted() -> (EditorApp, egui::Context) {
    let (mut app, ctx) = active_reply(true, "");
    let until = Instant::now() + Duration::from_secs(5);
    // Kill an established coordinator, matching a running helper that was
    // terminated externally rather than cancelling its asynchronous startup.
    loop {
        assert!(Instant::now() < until, "Coordinator did not start");
        match app.vaults.active.as_ref().unwrap().session.poll() {
            Some(crate::process::Event::Started) => break,
            Some(crate::process::Event::Error(error)) => panic!("{error}"),
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    app.vaults.active.as_ref().unwrap().session.stop();
    while !app.vaults.lost || !app.vaults.active.as_ref().unwrap().session.is_finished() {
        assert!(Instant::now() < until, "Coordinator did not exit");
        app.poll_vaults(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!app.workspace.as_ref().unwrap().ready);
    assert!(!app.vaults.released);
    (app, ctx)
}

fn reply_client(locked: bool, remote: Option<Remote>) -> Client {
    let reply = serde_json::json!({
        "kind": "closed",
        "info": {"cipherdir": "/cipher", "mountpoint": "/plain"},
        "locked": locked,
        "message": if locked { "Locked" } else { "Fixture mount is busy" },
    });
    let mut client = fixture_client(format!(
        "print({}, flush=True)\n",
        serde_json::to_string(&reply.to_string()).unwrap()
    ));
    client.remote = remote;
    client
}

fn finish(app: &mut EditorApp, ctx: &egui::Context) {
    let until = Instant::now() + Duration::from_secs(5);
    while app.vaults.locking {
        assert!(Instant::now() < until, "Lock recovery did not finish");
        app.poll_vaults(ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn interrupted_close_and_quit_verify_the_mount_without_opening_the_project_picker() {
    for action in [Action::CloseProject, Action::Quit, Action::LockEncrypted] {
        let quit = matches!(action, Action::Quit);
        let (mut app, ctx) = interrupted();
        let work = app.vaults.work();
        app.guarded(action, &ctx);
        assert!(app.dialog == Dialog::Encrypted);
        assert!(app.vaults.locking && !app.exit.closing());
        app.poll_vaults(&ctx);
        assert!(
            !app.vaults.lock_sent,
            "Recovery must drain owned work first"
        );
        assert!(!app.documents.is_empty());
        drop(work);
        app.send_encrypted_lock(|request, remote| {
            assert!(request.lock_only && !request.create);
            assert!(request.password.is_empty());
            assert_eq!(request.settings.cipherdir, "/cipher");
            assert!(remote.is_none());
            Ok(reply_client(true, remote))
        })
        .unwrap();
        finish(&mut app, &ctx);
        assert!(!app.vaults.has_active());
        assert!(app.documents.is_empty());
        assert_eq!(app.exit.closing(), quit);
        assert!(!app.workspace.project_open());
    }
}

#[test]
fn refused_recovery_keeps_buffers_and_can_retry_after_the_one_shot_helper_exits() {
    let (mut app, ctx) = interrupted();
    app.documents[0].fixture_content().text = "retained source".into();
    app.lock_encrypted(AfterLock::Quit, &ctx);
    app.send_encrypted_lock(|_, remote| Ok(reply_client(false, remote)))
        .unwrap();
    finish(&mut app, &ctx);
    assert!(app.vaults.has_active() && app.vaults.lost);
    assert!(!app.exit.closing());
    assert_eq!(app.documents[0].text, "retained source");
    assert!(app.vaults.message.contains("Fixture mount is busy"));
    let until = Instant::now() + Duration::from_secs(5);
    while !app.vaults.active.as_ref().unwrap().session.is_finished() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    app.poll_vaults(&ctx);
    assert!(app.vaults.message.contains("Fixture mount is busy"));
    app.guarded(Action::Quit, &ctx);
    assert!(app.dialog == Dialog::Encrypted && app.vaults.locking);
    app.send_encrypted_lock(|_, remote| Ok(reply_client(true, remote)))
        .unwrap();
    finish(&mut app, &ctx);
    assert!(app.exit.closing());
    assert!(!app.vaults.has_active());
}

#[test]
fn recovery_uses_retained_storage_and_only_the_matching_authenticated_ssh_host() {
    let (mut app, ctx) = active_reply(true, "");
    let mut workspace = crate::app::tests::remote_workspace();
    let original = workspace.remote.clone().unwrap();
    app.vaults.active.as_mut().unwrap().remote = Some(original.clone());
    app.vaults.lost = true;
    app.vaults.settings.cipherdir = "/unrelated-picker-draft".into();
    app.vaults.create = true;
    app.vaults.password = "draft-password".into();
    assert!(app.vaults.recovery_lock_request(None).is_err());
    workspace.connected = false;
    assert!(app.vaults.recovery_lock_request(Some(&workspace)).is_err());
    workspace.connected = true;
    workspace.remote.as_mut().unwrap().profile.host = "other-host".into();
    assert!(app.vaults.recovery_lock_request(Some(&workspace)).is_err());
    app.workspace.set_fixture(Some(workspace));
    app.guarded(Action::Quit, &ctx);
    assert!(app.dialog == Dialog::Encrypted);
    assert!(!app.vaults.locking && !app.exit.closing());
    assert!(app.vaults.message.contains("Reconnect SSH"));
    assert!(!app.documents.is_empty());
    let mut replacement = original;
    replacement.socket = "/new-authenticated-connection".into();
    app.workspace.as_mut().unwrap().remote = Some(replacement.clone());
    let (request, remote) = app
        .vaults
        .recovery_lock_request(app.workspace.as_ref())
        .unwrap();
    assert_eq!(remote.unwrap().socket, replacement.socket);
    assert_eq!(request.settings.cipherdir, "/cipher");
    assert!(request.lock_only && !request.create && request.password.is_empty());
}

#[test]
fn dirty_buffers_still_guard_quit_after_the_coordinator_dies() {
    let (mut app, ctx) = interrupted();
    app.documents[0].set_dirty_fixture(true);
    app.guarded(Action::Quit, &ctx);
    assert!(matches!(app.pending.as_ref(), Some(Action::Quit)));
    assert!(!app.vaults.locking && !app.exit.closing());
    assert!(!matches!(app.dialog, Dialog::Folder));
    assert!(app.documents[0].dirty());
}

#[test]
fn interrupted_active_projects_cannot_bypass_another_mounts_cleanup_on_quit() {
    let (mut app, ctx) = interrupted();
    app.vaults.needs_lock = true;
    assert!(matches!(
        app.vaults.quit_policy(),
        QuitPolicy::RecoveryRequired
    ));
    app.guarded(Action::Quit, &ctx);
    assert!(!app.exit.closing() && !app.vaults.locking);
    app.lock_encrypted(AfterLock::Quit, &ctx);
    assert!(app.dialog == Dialog::Encrypted);
    assert!(!app.exit.closing() && !app.vaults.locking);
    assert!(app.vaults.has_active() && app.vaults.needs_lock);
}

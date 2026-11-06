//! Real Git precedence and commit behaviour, plus settings failure/draft handling.
use super::*;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn git(root: &Path, args: &[String]) -> Output {
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for name in [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "EMAIL",
    ] {
        command.env_remove(name);
    }
    command.output().unwrap()
}

fn run(root: &Path, args: Vec<String>) -> String {
    let output = git(root, &args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn identity_scope_precedence_and_inheritance_allow_a_real_commit_without_touching_other_settings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    run(root, strings(&["init", "-b", "main"]));
    let empty = git(root, &read_args());
    assert_eq!(empty.status.code(), Some(1));
    assert!(
        Snapshot::parse(std::str::from_utf8(&empty.stdout).unwrap())
            .unwrap()
            .configured
            .name
            .is_none()
    );
    let failed = git(
        root,
        &strings(&[
            "-c",
            "user.useConfigOnly=true",
            "commit",
            "--allow-empty",
            "-m",
            "Identity check",
        ]),
    );
    assert!(!failed.status.success());
    assert!(missing(&String::from_utf8_lossy(&failed.stderr)));

    run(
        root,
        strings(&["config", "--local", "core.editor", "unchanged-editor"]),
    );
    for (key, value) in [
        ("user.name", "Global fixture"),
        ("user.email", "global@example.invalid"),
    ] {
        run(root, write_args(Scope::Global, key, Some(value)));
    }
    let name = "--config 'Test é' $(literal)";
    for (key, value) in [("user.name", name), ("user.email", "repo@example.invalid")] {
        run(root, write_args(Scope::Repository, key, Some(value)));
    }
    let snapshot = Snapshot::parse(&run(root, read_args())).unwrap();
    assert_eq!(
        snapshot.draft(Scope::Repository),
        (name.into(), "repo@example.invalid".into())
    );
    assert_eq!(
        snapshot.draft(Scope::Global),
        ("Global fixture".into(), "global@example.invalid".into())
    );
    run(
        root,
        strings(&["commit", "--allow-empty", "-m", "Identity configured"]),
    );
    let author = run(root, strings(&["log", "-1", "--format=%an <%ae>"]));
    assert_eq!(author.trim(), format!("{name} <repo@example.invalid>"));
    for key in ["user.name", "user.email"] {
        run(root, write_args(Scope::Repository, key, None));
    }
    let inherited = Snapshot::parse(&run(root, read_args())).unwrap();
    assert_eq!(
        inherited.draft(Scope::Repository),
        inherited.draft(Scope::Global)
    );
    assert_eq!(
        run(
            root,
            strings(&["config", "--local", "--get", "core.editor"])
        )
        .trim(),
        "unchanged-editor"
    );
    assert!(!root.join("literal").exists());
}

#[test]
fn identity_metadata_keeps_empty_values_reports_overrides_and_rejects_incomplete_records() {
    let snapshot = Snapshot::parse("global\0user.name\nFirst\0local\0user.name\n\0worktree\0user.email\nworktree@example.invalid\0global\0author.name\nSeparate author\0").unwrap();
    assert_eq!(snapshot.draft(Scope::Repository).0, "");
    assert_eq!(
        snapshot.configured.email.as_deref(),
        Some("worktree@example.invalid")
    );
    assert!(snapshot.overrides);
    for text in [
        "local\0",
        "local\0bad record\0",
        "unknown\0user.name\nname\0",
        "local\0core.editor\nvalue\0",
    ] {
        assert!(Snapshot::parse(text).is_err());
    }
}

#[test]
fn identity_validation_accepts_literal_names_and_rejects_empty_or_multiline_values() {
    let mut state = State {
        name: "  N'ame é $(literal) ".into(),
        email: " person+git@example.invalid ".into(),
        ..Default::default()
    };
    assert_eq!(
        state.values().unwrap(),
        (
            "N'ame é $(literal)".into(),
            "person+git@example.invalid".into()
        )
    );
    for (name, email) in [
        ("", "a@b"),
        ("Name", ""),
        ("Bad\nName", "a@b"),
        ("Name", "a b@c"),
        ("Name", "a<b>@c"),
    ] {
        state.name = name.into();
        state.email = email.into();
        assert!(state.values().is_err());
    }
    assert!(!missing("pre-commit hook failed"));
    assert!(!missing("Permission denied (publickey)"));
}

#[test]
fn selecting_a_save_scope_preserves_edits_and_discard_loads_that_scopes_values() {
    let mut state = State { snapshot: Some(Snapshot::parse("global\0user.name\nGlobal\0global\0user.email\nglobal@example.invalid\0local\0user.name\nRepository\0").unwrap()), ..Default::default() };
    state.reset_draft();
    assert_eq!(state.name, "Repository");
    state.name = "Unapplied identity".into();
    state.dirty = true;
    state.select_scope(Scope::Global);
    assert_eq!(state.name, "Unapplied identity");
    assert!(state.dirty);
    state.reset_draft();
    assert_eq!(state.name, "Global");
    assert!(!state.dirty);
}

#[test]
fn partial_saves_keep_the_draft_and_commit_message_and_report_the_failed_key() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workbench.git.commit_message = "Keep staged commit".into();
    app.workbench.git.identity.name = "New name".into();
    app.workbench.git.identity.email = "new@example.invalid".into();
    app.workbench.git.identity.dirty = true;
    app.workbench.git.identity.notice = "Saving identity".into();
    app.git_identity_reply(
        Job::Email {
            scope: Scope::Global,
            unset: false,
        },
        project::Output {
            code: 4,
            stderr: "config lock: Permission denied".into(),
            ..Default::default()
        },
        &ctx,
    );
    assert!(app.workbench.git.git_error.contains("name was updated"));
    assert!(app.workbench.git.git_error.contains("Permission denied"));
    assert!(app.workbench.git.identity.dirty);
    assert!(app.workbench.git.identity.notice.is_empty());
    assert_eq!(app.workbench.git.identity.email, "new@example.invalid");
    assert_eq!(app.workbench.git.commit_message, "Keep staged commit");
    app.git_identity_reply(
        Job::Email {
            scope: Scope::Repository,
            unset: true,
        },
        project::Output {
            code: 4,
            stderr: "config lock: Permission denied".into(),
            ..Default::default()
        },
        &ctx,
    );
    assert!(
        app.workbench
            .git
            .git_error
            .contains("email could not be removed")
    );
    assert!(
        app.workbench
            .git
            .git_error
            .contains("Use inherited identity")
    );
    assert!(app.workbench.git.identity.dirty);
    assert_eq!(app.workbench.git.commit_message, "Keep staged commit");
}

#[test]
fn identity_shortcut_waits_for_pending_git_replies_before_switching_tabs() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workbench.git.git_root = "/fixture".into();
    app.workbench.git.git_error = "Author identity unknown".into();
    app.workbench.git.git_tab = GitTab::History;
    let frame = |app: &mut EditorApp, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700., 800.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.git_ui(ui, ctx));
            },
        )
    };
    for busy in [true, false] {
        app.workbench.git.git_busy = busy;
        frame(&mut app, vec![]);
        let output = frame(&mut app, vec![]);
        let target = layout::tests::button_frame(&output, "Configure Git identity")
            .0
            .center();
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![
                    egui::Event::PointerMoved(target),
                    egui::Event::PointerButton {
                        pos: target,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(
            app.workbench.git.git_tab,
            if busy {
                GitTab::History
            } else {
                GitTab::Settings
            }
        );
    }
}

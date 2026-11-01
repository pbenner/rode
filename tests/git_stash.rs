//! Native/Python stash identity, Git writer locking and reflog parity.
use rode::{
    files,
    project::{self, Request, Response, StashAction},
};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .trim_end_matches('\n')
        .into()
}
fn invoke(
    root: &Path,
    peer: bool,
    action: StashAction,
    reference: &str,
    oid: &str,
) -> anyhow::Result<project::Output> {
    let request = Request::GitStash {
        root: root.to_string_lossy().into(),
        action,
        reference: reference.into(),
        oid: oid.into(),
    };
    let response = if peer {
        let mut child = Command::new("python3")
            .args(["-c", files::REMOTE_HELPER])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&files::Request::Tool { request })?)?;
        let output = child.wait_with_output()?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<files::Response>(&output.stdout)?
            .tool
            .unwrap()
    } else {
        project::local(&request)?
    };
    let Response::Command(output) = response else {
        panic!("Missing Git reply")
    };
    Ok(output)
}
fn repository(root: &Path, sha256: bool) {
    let format = if sha256 {
        "--object-format=sha256"
    } else {
        "--object-format=sha1"
    };
    git(root, &["init", "-b", "main", format]);
    git(root, &["config", "user.name", "RODE Test"]);
    git(root, &["config", "user.email", "rode@example.invalid"]);
    fs::write(root.join("file.txt"), "baseline\n").unwrap();
    git(root, &["add", "--", "file.txt"]);
    git(root, &["commit", "-qm", "Baseline"]);
}
fn stash(root: &Path, value: &str) -> String {
    // Git 2.43's stash push fails in SHA-256 repositories. Build the same
    // two-parent stash shape with plumbing so hash-format coverage still runs.
    if git(root, &["rev-parse", "--show-object-format"]) == "sha256" {
        let base = git(root, &["rev-parse", "HEAD"]);
        let index_tree = git(root, &["write-tree"]);
        let index_commit = git(
            root,
            &[
                "commit-tree",
                &index_tree,
                "-p",
                &base,
                "-m",
                "Index snapshot",
            ],
        );
        fs::write(root.join("file.txt"), format!("{value}\n")).unwrap();
        git(root, &["add", "--", "file.txt"]);
        let tree = git(root, &["write-tree"]);
        let message = format!("On main: {value}");
        let commit = git(
            root,
            &[
                "commit-tree",
                &tree,
                "-p",
                &base,
                "-p",
                &index_commit,
                "-m",
                &message,
            ],
        );
        git(
            root,
            &[
                "update-ref",
                "--create-reflog",
                "-m",
                &message,
                "refs/stash",
                &commit,
            ],
        );
        git(root, &["reset", "--hard", "HEAD"]);
        return commit;
    }
    fs::write(root.join("file.txt"), format!("{value}\n")).unwrap();
    git(root, &["stash", "push", "-m", value]);
    git(root, &["rev-parse", "refs/stash"])
}
fn hashes(root: &Path) -> Vec<String> {
    git(root, &["stash", "list", "--format=%H"])
        .lines()
        .map(str::to_owned)
        .collect()
}
fn clean(root: &Path) {
    git(root, &["restore", "--", "file.txt"]);
}

#[test]
fn interrupted_workers_release_git_locks_even_without_language_cleanup() {
    // The isolated parent reaps orphaned Git processes without changing this
    // parallel Rust test runner's process-wide child ownership.
    let mut child = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/stash_interrupt.py"
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(
        child.stdin.take().unwrap(),
        &serde_json::json!({
            "binary": env!("CARGO_BIN_EXE_rode"),
            "peer": files::REMOTE_HELPER,
        }),
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn stale_selectors_never_drop_another_stash_and_apply_uses_the_captured_object() {
    for peer in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        repository(root, false);
        let a = stash(root, "selected A");
        let b = stash(root, "later B");
        let before = hashes(root);
        let error = invoke(root, peer, StashAction::Drop, "stash@{0}", &a).unwrap_err();
        assert!(error.to_string().contains("Stash list changed"));
        assert_eq!(hashes(root), before);
        assert!(
            invoke(root, peer, StashAction::Apply, "stash@{0}", &a)
                .unwrap()
                .success()
        );
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "selected A\n"
        );
        clean(root);
        let pop = invoke(root, peer, StashAction::Pop, "stash@{0}", &a).unwrap();
        assert!(!pop.success());
        assert!(pop.stderr.contains("was applied") && pop.stderr.contains("refresh"));
        assert_eq!(hashes(root), before);
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "selected A\n"
        );
        clean(root);
        assert!(
            invoke(root, peer, StashAction::Drop, "stash@{1}", &a)
                .unwrap()
                .success()
        );
        assert_eq!(hashes(root), vec![b.clone()]);
        assert!(
            invoke(root, peer, StashAction::Pop, "stash@{0}", &b)
                .unwrap()
                .success()
        );
        assert!(hashes(root).is_empty());
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "later B\n"
        );
        assert!(!root.join(".git/refs/stash").exists());
        assert!(!root.join(".git/logs/refs/stash").exists());
        git(root, &["fsck", "--no-reflogs"]);
    }
}

#[test]
fn packed_stash_refs_and_linked_worktrees_preserve_other_refs_for_both_hash_formats() {
    use std::os::unix::fs::PermissionsExt;
    for peer in [false, true] {
        for sha256 in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            repository(root, sha256);
            git(root, &["tag", "-a", "retained", "-m", "Annotated tag"]);
            let tag = git(root, &["rev-parse", "retained"]);
            let a = stash(root, "A");
            let b = stash(root, "B");
            let reference_mode = fs::metadata(root.join(".git/refs/stash"))
                .unwrap()
                .permissions()
                .mode();
            let linked = root.join("linked 'worktree' 🦀");
            git(
                root,
                &["worktree", "add", "--detach", linked.to_str().unwrap()],
            );
            git(root, &["pack-refs", "--all"]);
            assert!(!root.join(".git/refs/stash").exists());
            assert!(
                invoke(&linked, peer, StashAction::Drop, "stash@{0}", &b)
                    .unwrap()
                    .success()
            );
            assert_eq!(hashes(root), vec![a.clone()]);
            assert_eq!(
                fs::metadata(root.join(".git/refs/stash"))
                    .unwrap()
                    .permissions()
                    .mode(),
                reference_mode
            );
            assert_eq!(git(root, &["rev-parse", "refs/stash"]), a);
            git(root, &["pack-refs", "--all"]);
            assert!(
                invoke(&linked, peer, StashAction::Drop, "stash@{0}", &a)
                    .unwrap()
                    .success()
            );
            assert!(hashes(root).is_empty());
            assert_eq!(git(root, &["rev-parse", "retained"]), tag);
            assert!(!git(root, &["show-ref"]).contains("refs/stash"));
            git(root, &["fsck", "--no-reflogs"]);
        }
    }
}

#[test]
fn locks_validation_and_conflicting_pop_leave_saved_stashes_intact() {
    for peer in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        repository(root, false);
        let a = stash(root, "A");
        let log = root.join(".git/logs/refs/stash");
        let before = fs::read(&log).unwrap();
        for name in [
            "refs/stash.lock",
            "packed-refs.lock",
            "logs/refs/stash.lock",
        ] {
            let lock = root.join(".git").join(name);
            fs::write(&lock, "another Git writer").unwrap();
            let error = invoke(root, peer, StashAction::Drop, "stash@{0}", &a).unwrap_err();
            assert!(error.to_string().contains("busy"));
            assert_eq!(fs::read_to_string(&lock).unwrap(), "another Git writer");
            assert_eq!(fs::read(&log).unwrap(), before);
            fs::remove_file(lock).unwrap();
        }
        for (reference, oid) in [
            ("stash@{18446744073709551615}", a.as_str()),
            ("stash@{-1}", &a),
            ("stash@{0}", "--help"),
        ] {
            assert!(invoke(root, peer, StashAction::Drop, reference, oid).is_err());
            assert_eq!(hashes(root), vec![a.clone()]);
        }
        fs::write(root.join("file.txt"), "unsaved on-disk work\n").unwrap();
        assert!(
            !invoke(root, peer, StashAction::Pop, "stash@{0}", &a)
                .unwrap()
                .success()
        );
        assert_eq!(hashes(root), vec![a.clone()]);
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "unsaved on-disk work\n"
        );
        assert_eq!(fs::read(&log).unwrap(), before);
    }
}

#[test]
fn final_stash_deletion_respects_hooks_and_does_not_unlink_a_later_writers_lock() {
    use std::os::unix::fs::PermissionsExt;
    for peer in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        repository(root, false);
        let a = stash(root, "A");
        let hook = root.join(".git/hooks/reference-transaction");
        fs::write(&hook, "#!/bin/sh\nprintf '%s\\n' \"$1\" >> phases\nif [ \"$1\" = prepared ]; then echo rejected >&2; exit 1; fi\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            invoke(root, peer, StashAction::Drop, "stash@{0}", &a)
                .unwrap_err()
                .to_string()
                .contains("rejected")
        );
        assert_eq!(hashes(root), vec![a.clone()]);
        assert_eq!(
            fs::read_to_string(root.join("phases")).unwrap(),
            "prepared\naborted\n"
        );
        fs::write(&hook, format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' \"$1\" >> phases\nif [ \"$1\" = prepared ]; then\n if git stash store -m intrusion {a} 2>/dev/null; then exit 1; fi\nfi\nif [ \"$1\" = committed ]; then printf 'new writer' > .git/refs/stash.lock; fi\n")).unwrap();
        assert!(
            invoke(root, peer, StashAction::Drop, "stash@{0}", &a)
                .unwrap()
                .success()
        );
        assert!(hashes(root).is_empty());
        assert_eq!(
            fs::read_to_string(root.join(".git/refs/stash.lock")).unwrap(),
            "new writer"
        );
    }
}

#[test]
fn reflog_rewrites_match_git_drop_for_first_middle_and_last_entries() {
    for peer in [false, true] {
        for index in 0..3 {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            repository(root, false);
            for value in ["A", "B", "C"] {
                stash(root, value);
            }
            let reference = format!("stash@{{{index}}}");
            let oid = git(root, &["rev-parse", &reference]);
            let log = root.join(".git/logs/refs/stash");
            let tip = root.join(".git/refs/stash");
            let original_log = fs::read(&log).unwrap();
            let original_tip = fs::read(&tip).unwrap();
            assert!(
                invoke(root, peer, StashAction::Drop, &reference, &oid)
                    .unwrap()
                    .success()
            );
            let rewritten_log = fs::read(&log).unwrap();
            let rewritten_tip = fs::read(&tip).unwrap();
            fs::write(&log, original_log).unwrap();
            fs::write(&tip, original_tip).unwrap();
            git(root, &["stash", "drop", &reference]);
            assert_eq!(fs::read(&log).unwrap(), rewritten_log);
            assert_eq!(fs::read(&tip).unwrap(), rewritten_tip);
        }
    }
}

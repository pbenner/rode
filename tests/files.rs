//! Local/Python file protocol parity, content conflicts, permissions/symlinks, quoting and real
//! PTY input.

use rode::files::{self, Request};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};
use tempfile::TempDir;

fn exercise(run: impl Fn(&Request) -> anyhow::Result<files::Response>) {
    let root = TempDir::new().unwrap();
    let path = root
        .path()
        .join("a 'quoted' $file; 🚀.py")
        .to_str()
        .unwrap()
        .to_string();
    let original = "# remote host work\r\nprint('héllo 🦀')\r\n";
    fs::write(&path, original).unwrap();
    let read = run(&Request::Read { path: path.clone() }).unwrap();
    assert_eq!(
        read.text, original,
        "UTF-8 and CRLF must round-trip without normalization"
    );
    assert_eq!(read.revision, files::digest(original.as_bytes()));
    let saved = run(&Request::Write {
        path: path.clone(),
        text: "changed\n".into(),
        expected: Some(read.revision.clone()),
    })
    .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "changed\n");
    assert_eq!(saved.revision, files::digest(b"changed\n"));
    let conflict = run(&Request::Write {
        path: path.clone(),
        text: "stale\n".into(),
        expected: Some(read.revision),
    })
    .unwrap_err();
    assert!(conflict.to_string().contains("conflict"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "changed\n");
    let overwrite = run(&Request::Write {
        path: path.clone(),
        text: "overwrite".into(),
        expected: None,
    });
    assert!(
        overwrite.is_err(),
        "Save As must not silently overwrite an existing file"
    );
    fs::remove_file(&path).unwrap();
    assert!(
        run(&Request::Write {
            path: path.clone(),
            text: "stale".into(),
            expected: Some(saved.revision)
        })
        .is_err()
    );
    run(&Request::Write {
        path: path.clone(),
        text: "new file".into(),
        expected: None,
    })
    .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new file");
    fs::create_dir(root.path().join("z-dir")).unwrap();
    let listing = run(&Request::List {
        path: root.path().to_str().unwrap().into(),
    })
    .unwrap();
    assert!(listing.entries[0].dir);
    assert_eq!(listing.entries.len(), 2);
    assert!(listing.entries.iter().any(|e| e.path == path));
    fs::write(&path, b"binary\0data").unwrap();
    assert!(run(&Request::Read { path: path.clone() }).is_err());
    fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(run(&Request::Read { path: path.clone() }).is_err());
    fs::File::create(&path)
        .unwrap()
        .set_len(files::MAX_FILE_BYTES + 1)
        .unwrap();
    assert!(run(&Request::Read { path }).is_err());
}

#[test]
fn local_file_lifecycle_preserves_edits_and_detects_conflicts() {
    exercise(files::local);
}

fn python(request: &Request) -> anyhow::Result<files::Response> {
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
        .write_all(&serde_json::to_vec(request)?)?;
    let result = child.wait_with_output()?;
    anyhow::ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(serde_json::from_slice(&result.stdout)?)
}

#[test]
fn remote_helper_has_same_file_semantics() {
    exercise(python);
}

#[test]
#[cfg(unix)]
fn saves_preserve_executable_permissions_and_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for run in [files::local, python] {
        let root = TempDir::new().unwrap();
        let target = root.path().join("job.sh");
        let link = root.path().join("link.sh");
        fs::write(&target, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o750)).unwrap();
        symlink(&target, &link).unwrap();
        let read = run(&Request::Read {
            path: link.to_str().unwrap().into(),
        })
        .unwrap();
        run(&Request::Write {
            path: link.to_str().unwrap().into(),
            text: "#!/bin/sh\necho ready\n".into(),
            expected: Some(read.revision),
        })
        .unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            2,
            "No temporary files left behind"
        );
    }
}

#[test]
fn unicode_find_offsets_are_valid_and_non_overlapping() {
    use rode::app::find_matches;
    let text = "İx ÄBC äbc 🦀";
    let matches = find_matches(text, "äbc", false);
    assert_eq!(
        matches
            .iter()
            .map(|(a, b)| &text[*a..*b])
            .collect::<Vec<_>>(),
        vec!["ÄBC", "äbc"]
    );
    assert!(find_matches(text, "", false).is_empty());
    assert_eq!(find_matches("aaaa", "aa", true), vec![(0, 2), (2, 4)]);
    assert_eq!(find_matches("İi", "i", false), vec![(2, 3)]);
}

#[test]
fn ssh_arguments_and_shell_paths_do_not_inject_commands() {
    use rode::ssh::{Profile, quote, remote_shell};
    let profile = Profile {
        host: "workstation".into(),
        user: "researcher".into(),
        port: "2222".into(),
        jump: "login-a,login-b".into(),
        identity: "/tmp/key with spaces".into(),
        ..Default::default()
    };
    profile.validate().unwrap();
    let options = profile.options();
    assert!(options.windows(2).any(|p| p == ["-J", "login-a,login-b"]));
    assert!(
        options
            .windows(2)
            .any(|p| p == ["-i", "/tmp/key with spaces"])
    );
    for host in ["", "-oProxyCommand=bad", "host\nother", "host with spaces"] {
        assert!(
            Profile {
                host: host.into(),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        Profile {
            host: "workstation".into(),
            port: "0".into(),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    let value = "a 'quoted' $(echo NOT_EXECUTED) `echo bad`; file\nnext";
    let output = Command::new("sh")
        .args(["-c", &format!("printf '%s' {}", quote(value))])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap(), value);
    assert_eq!(
        remote_shell("~/a b"),
        "cd -- \"$HOME\"/'a b' && exec \"${SHELL:-/bin/sh}\" -l"
    );
}

#[test]
fn terminal_keys_keep_shell_control_sequences() {
    use eframe::egui::{Key, Modifiers};
    use rode::terminal::key_bytes;
    assert_eq!(key_bytes(Key::C, Modifiers::CTRL, false), Some(vec![3]));
    assert_eq!(
        key_bytes(Key::ArrowUp, Modifiers::NONE, true),
        Some(b"\x1bOA".to_vec())
    );
    assert_eq!(
        key_bytes(Key::Tab, Modifiers::SHIFT, false),
        Some(b"\x1b[Z".to_vec())
    );
}

#[test]
fn rust_documents_have_colored_syntax_spans() {
    let ctx = eframe::egui::Context::default();
    let theme = egui_extras::syntax_highlighting::CodeTheme::dark(14.0);
    let layout = egui_extras::syntax_highlighting::highlight(
        &ctx,
        &Default::default(),
        &theme,
        "// comment\nfn main() { let x = 42; }",
        rode::app::syntax_token(Some("main.rs")),
    );
    let colors: std::collections::HashSet<_> =
        layout.sections.iter().map(|s| s.format.color).collect();
    assert!(
        colors.len() > 2,
        "Rust source must use syntax highlighting, not the plain-text fallback"
    );
}

#[test]
#[cfg(unix)]
fn real_pty_runs_and_parses_ansi_output() {
    use std::time::{Duration, Instant};
    let mut command = portable_pty::CommandBuilder::new("/bin/sh");
    command.args(["-c", r"printf '\033[32mRODE_PTY_OK\033[0m'; sleep 0.1"]);
    let terminal =
        rode::terminal::Terminal::spawn(&eframe::egui::Context::default(), command, "test".into())
            .unwrap();
    let start = Instant::now();
    while !terminal.contents().contains("RODE_PTY_OK") && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(terminal.contents().contains("RODE_PTY_OK"));
    assert!(!terminal.contents().contains("\x1b"));
}

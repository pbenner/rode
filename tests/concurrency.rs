//! Local Rust helpers and the Python SSH peer coordinate on the same file inodes.
use rode::{
    files::{self, Request},
    persistence,
};
use std::{
    fs,
    io::Write,
    process::{Child, Command, Stdio},
    sync::Arc,
};

fn writer(request: &Request, python: bool) -> Child {
    let mut command = if python {
        let mut command = Command::new("python3");
        command.args(["-c", files::REMOTE_HELPER]);
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
        command.arg("--file-helper");
        command
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(request).unwrap())
        .unwrap();
    child
}

#[test]
fn simultaneous_existing_file_saves_have_one_winner_across_rust_and_python() {
    use std::os::fd::AsRawFd;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("quoted ' 🦀.txt");
    fs::write(&path, "original").unwrap();
    let locked = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    assert_eq!(unsafe { libc::flock(locked.as_raw_fd(), libc::LOCK_EX) }, 0);
    let children: Vec<_> = (0..8)
        .map(|i| {
            writer(
                &Request::Write {
                    path: path.to_string_lossy().into(),
                    text: format!("writer-{i}"),
                    expected: Some(files::digest(b"original")),
                },
                i % 2 == 0,
            )
        })
        .collect();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "original",
        "A writer bypassed the existing-file lock"
    );
    drop(locked);
    let mut winners = vec![];
    for (i, child) in children.into_iter().enumerate() {
        let out = child.wait_with_output().unwrap();
        if out.status.success() {
            winners.push(i);
        } else {
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("conflict"),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    assert_eq!(winners.len(), 1);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("writer-{}", winners[0])
    );
}

#[test]
fn simultaneous_save_as_never_replaces_a_file_another_process_just_created() {
    let root = tempfile::tempdir().unwrap();
    for round in 0..3 {
        let path = root.path().join(format!("new-{round}"));
        let children: Vec<_> = (0..8)
            .map(|i| {
                writer(
                    &Request::Write {
                        path: path.to_string_lossy().into(),
                        text: format!("writer-{i}\n{}", "x".repeat(256 * 1024)),
                        expected: None,
                    },
                    i % 2 == 0,
                )
            })
            .collect();
        let mut winners = vec![];
        for (i, child) in children.into_iter().enumerate() {
            let out = child.wait_with_output().unwrap();
            if out.status.success() {
                winners.push(i);
            } else {
                assert!(String::from_utf8_lossy(&out.stderr).contains("conflict"));
            }
        }
        assert_eq!(winners.len(), 1);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .starts_with(&format!("writer-{}\n", winners[0]))
        );
    }
}

#[test]
fn editing_the_preferences_file_and_changing_a_setting_cooperate() {
    use eframe::Storage;
    use std::os::fd::AsRawFd;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    let original = "{\"color_theme\":\"phosphor\"}";
    fs::write(&path, original).unwrap();
    let locked = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    unsafe { libc::flock(locked.as_raw_fd(), libc::LOCK_EX) };
    let editor = writer(
        &Request::Write {
            path: path.to_string_lossy().into(),
            text: "{\"color_theme\":\"phosphor\",\"manual\":\"keep\"}".into(),
            expected: Some(files::digest(original.as_bytes())),
        },
        true,
    );
    let mut prefs = persistence::Preferences::open(path.clone(), Arc::new(|| {}));
    prefs.set_string("color_theme", "carbon".into());
    prefs.flush();
    drop(locked);
    let out = editor.wait_with_output().unwrap();
    drop(prefs);
    let values: std::collections::BTreeMap<String, String> =
        ron::de::from_bytes(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(values["color_theme"], "carbon");
    if out.status.success() {
        assert_eq!(values["manual"], "keep");
    } else {
        assert!(String::from_utf8_lossy(&out.stderr).contains("conflict"));
    }
}

#[test]
fn parallel_sandboxed_helpers_do_not_collide_even_with_identical_namespace_pids() {
    let root = tempfile::tempdir().unwrap();
    let plan = rode::sandbox::prepare(
        root.path().to_str().unwrap(),
        &rode::sandbox::Options {
            enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    if !plan.active {
        eprintln!("Sandbox unavailable: {}", plan.reason);
        return;
    }
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|index| {
                let barrier = &barrier;
                let plan = &plan;
                let root = root.path();
                scope.spawn(move || {
                    let path = root.join(format!("file-{index}.txt"));
                    let text = format!("file-{index}\n{}", "x".repeat(2 * 1024 * 1024));
                    barrier.wait();
                    plan.local_request_with_helper(
                        &Request::Write {
                            path: path.to_string_lossy().into(),
                            text: text.clone(),
                            expected: None,
                        },
                        std::path::Path::new(env!("CARGO_BIN_EXE_rode")),
                    )
                    .unwrap();
                    assert_eq!(fs::read_to_string(path).unwrap(), text);
                })
            })
            .collect();
        for job in jobs {
            job.join().unwrap();
        }
    });
    assert_eq!(
        fs::read_dir(root.path()).unwrap().count(),
        8,
        "Temporary files leaked into the project"
    );
}

#[test]
fn concurrent_environment_creation_reserves_the_folder_once() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("slow-python");
    fs::write(
        &program,
        "#!/bin/sh\nsleep 0.3\nexec /usr/bin/python3 \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let request = Request::Tool {
        request: rode::project::Request::CreateEnvironment {
            root: root.path().to_string_lossy().into(),
            path: ".venv".into(),
            python: program.to_string_lossy().into(),
            with_pip: false,
        },
    };
    let first = writer(&request, false);
    let second = writer(&request, true);
    let outputs = [
        first.wait_with_output().unwrap(),
        second.wait_with_output().unwrap(),
    ];
    assert_eq!(outputs.iter().filter(|o| o.status.success()).count(), 1);
    for output in &outputs {
        if !output.status.success() {
            assert!(String::from_utf8_lossy(&output.stderr).contains("already exist"));
        }
    }
    assert!(root.path().join(".venv/pyvenv.cfg").exists());
}

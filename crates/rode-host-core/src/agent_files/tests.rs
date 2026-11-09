//! Exercise the native boundary without launching shells, interpreters or models.
use super::*;
use std::{
    os::unix::fs::symlink,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn call(root: &Path, tool: &str, arguments: Value, read_only: bool) -> Result<Value> {
    execute(
        &Request {
            root: root.to_string_lossy().into_owned(),
            tool: tool.into(),
            arguments,
            read_only,
            protected_paths: vec![],
            protected_configuration: vec![],
        },
        &Root::open(root.to_str().unwrap(), &[])?,
    )
}

#[test]
fn private_configuration_aliases_are_excluded_from_reads_lists_and_searches() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let state = root.join("state");
    fs::create_dir(&state).unwrap();
    fs::write(state.join("auth.json"), "fixture credential").unwrap();
    fs::write(root.join("source.txt"), "fixture source").unwrap();
    let prefix = vec![
        "bwrap".into(),
        "--ro-bind".into(),
        state.to_string_lossy().into(),
        "/rode-private-state".into(),
        "--".into(),
    ];
    let paths = configuration_paths(root, Some("/rode-private-state"), None, &prefix).unwrap();
    assert!(paths.iter().any(|path| Path::new(path) == state));
    let capability = Root::open(root.to_str().unwrap(), &paths).unwrap();
    assert!(capability.relative("state/auth.json", false).is_err());
    let listed = capability.list(Path::new(".")).unwrap();
    assert_eq!(listed["entries"].as_array().unwrap().len(), 1);
    let search = capability.search(PathBuf::from("."), "fixture").unwrap();
    assert_eq!(search["matches"].as_array().unwrap().len(), 1);
    assert_eq!(search["matches"][0]["path"], "./source.txt");
    let home = root.join("home");
    assert_eq!(
        configuration_paths(root, Some(""), home.to_str(), &[]).unwrap(),
        configuration_paths(root, None, home.to_str(), &[]).unwrap()
    );
    assert!(
        configuration_paths(root, None, None, &[])
            .unwrap()
            .is_empty()
    );
    let captured_home = ["landlock", "--setenv", "HOME", "/captured/home", "--"].map(String::from);
    assert_eq!(
        configuration_paths(root, None, None, &captured_home).unwrap(),
        vec!["/captured/home/.codex"]
    );
}

#[test]
fn native_tools_create_read_search_replace_and_delete_with_revision_checks() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    call(root, "rode_create_directory", json!({"path":"src"}), false).unwrap();
    let created = call(
        root,
        "rode_write_file",
        json!({"path":"src/main.rs","text":"fn main() {}\n","revision":null}),
        false,
    )
    .unwrap();
    let read = call(root, "rode_read_file", json!({"path":"src/main.rs"}), false).unwrap();
    assert_eq!(created["revision"], read["revision"]);
    assert_eq!(read["text"], "fn main() {}\n");
    let listed = call(root, "rode_list_files", json!({"path":"src"}), false).unwrap();
    assert_eq!(listed["entries"][0]["path"], "src/main.rs");
    let found = call(
        root,
        "rode_search_files",
        json!({"path":".","text":"main()"}),
        false,
    )
    .unwrap();
    assert_eq!(found["matches"][0]["line"], 1);
    let updated = call(root, "rode_write_file", json!({"path":"src/main.rs","text":"fn main() { println!(\"hello\"); }\n","revision":read["revision"]}), false).unwrap();
    assert!(
        call(
            root,
            "rode_delete_file",
            json!({"path":"src/main.rs","revision":read["revision"]}),
            false
        )
        .is_err()
    );
    call(
        root,
        "rode_delete_file",
        json!({"path":"src/main.rs","revision":updated["revision"]}),
        false,
    )
    .unwrap();
    assert!(!root.join("src/main.rs").exists());
}

#[test]
fn outside_paths_symlinks_and_metadata_are_rejected_for_every_operation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    fs::create_dir(&root).unwrap();
    let outside = temporary.path().join("private.txt");
    fs::write(&outside, "private").unwrap();
    symlink(&outside, root.join("link")).unwrap();
    symlink(temporary.path(), root.join("escape")).unwrap();
    for path in [
        "../private.txt",
        outside.to_str().unwrap(),
        "link",
        "escape/private.txt",
        ".git/config",
        "nested/.codex/config.toml",
    ] {
        assert!(
            call(&root, "rode_read_file", json!({"path":path}), false).is_err(),
            "Read allowed {path}"
        );
        assert!(
            call(
                &root,
                "rode_write_file",
                json!({"path":path,"text":"damaged","revision":null}),
                false
            )
            .is_err(),
            "Write allowed {path}"
        );
        assert!(
            call(
                &root,
                "rode_delete_file",
                json!({"path":path,"revision":crate::files::digest(b"private")}),
                false
            )
            .is_err(),
            "Delete allowed {path}"
        );
    }
    assert!(call(&root, "rode_list_files", json!({"path":"escape"}), false).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "private");
    assert_eq!(
        call(&root, "rode_list_files", json!({"path":"."}), false).unwrap()["entries"],
        json!([])
    );
}

#[test]
fn mutation_lock_ends_with_its_owner_even_if_a_spawn_retains_the_descriptor() {
    let temporary = tempfile::tempdir().unwrap();
    let path = Path::new("source");
    fs::write(temporary.path().join(path), "original").unwrap();
    let root = Root::open(temporary.path().to_str().unwrap(), &[]).unwrap();
    let (owner, _, revision) = root.read(path, true).unwrap();
    // A descriptor copied across fork shares the same open file description.
    let inherited = owner.try_clone().unwrap();
    assert!(root.write(path, "blocked", Some(&revision)).is_err());
    drop(owner);
    root.write(path, "replacement", Some(&revision)).unwrap();
    assert_eq!(
        fs::read_to_string(temporary.path().join(path)).unwrap(),
        "replacement"
    );
    drop(inherited);
}

#[test]
fn read_only_and_external_edits_cannot_be_bypassed() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::write(root.join("source"), "original").unwrap();
    let read = call(root, "rode_read_file", json!({"path":"source"}), true).unwrap();
    for (tool, arguments) in [
        (
            "rode_write_file",
            json!({"path":"source","text":"replacement","revision":read["revision"]}),
        ),
        (
            "rode_write_file",
            json!({"path":"new","text":"new","revision":null}),
        ),
        (
            "rode_delete_file",
            json!({"path":"source","revision":read["revision"]}),
        ),
        ("rode_create_directory", json!({"path":"newdir"})),
    ] {
        assert!(call(root, tool, arguments, true).is_err());
    }
    fs::write(root.join("source"), "external edit").unwrap();
    assert!(
        call(
            root,
            "rode_write_file",
            json!({"path":"source","text":"replacement","revision":read["revision"]}),
            false
        )
        .is_err()
    );
    assert!(
        call(
            root,
            "rode_write_file",
            json!({"path":"source","text":"replacement","revision":null}),
            false
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(root.join("source")).unwrap(),
        "external edit"
    );
    assert_eq!(fs::read_dir(root).unwrap().count(), 1);
}

#[test]
fn replacement_does_not_modify_hard_link_targets_outside_the_project() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    fs::create_dir(&root).unwrap();
    let outside = temporary.path().join("private");
    fs::write(&outside, "original").unwrap();
    fs::hard_link(&outside, root.join("source")).unwrap();
    let read = call(&root, "rode_read_file", json!({"path":"source"}), false).unwrap();
    call(
        &root,
        "rode_write_file",
        json!({"path":"source","text":"replacement","revision":read["revision"]}),
        false,
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&outside).unwrap(), "original");
    assert_eq!(
        fs::read_to_string(root.join("source")).unwrap(),
        "replacement"
    );
}

#[test]
fn path_swaps_never_follow_an_outside_symlink() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    fs::create_dir(&root).unwrap();
    let outside = temporary.path().join("private");
    fs::write(&outside, "private data").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        let stop_writer = stop.clone();
        let path = root.join("source");
        let outside = &outside;
        scope.spawn(move || {
            while !stop_writer.load(Ordering::Acquire) {
                let _ = fs::remove_file(&path);
                let _ = symlink(outside, &path);
                let _ = fs::remove_file(&path);
                let _ = fs::write(&path, "project data");
            }
        });
        for _ in 0..200 {
            if let Ok(read) = call(&root, "rode_read_file", json!({"path":"source"}), false) {
                assert_ne!(read["text"], "private data");
            }
        }
        stop.store(true, Ordering::Release);
    });
    assert_eq!(fs::read_to_string(outside).unwrap(), "private data");
}

#[test]
fn executable_input_special_files_and_oversized_text_are_unavailable() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    assert!(call(root, "exec_command", json!({"cmd":"touch escaped"}), false).is_err());
    assert!(
        call(
            root,
            "rode_list_files",
            json!({"path":".","command":"touch escaped"}),
            false
        )
        .is_err()
    );
    assert!(
        call(
            root,
            "rode_write_file",
            json!({"path":"source","text":"abc"}),
            false
        )
        .is_err()
    );
    assert!(
        call(
            root,
            "rode_write_file",
            json!({"path":"source","text":"x".repeat(MAX_FILE as usize+1),"revision":null}),
            false
        )
        .is_err()
    );
    let fifo = CString::new(root.join("fifo").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(call(root, "rode_read_file", json!({"path":"fifo"}), false).is_err());
    assert!(!root.join("escaped").exists());
}

#[test]
fn protected_directories_and_scan_budgets_apply_to_every_entry_and_byte() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let protected = root.join("runtime");
    fs::create_dir(&protected).unwrap();
    let request = Request {
        root: root.to_str().unwrap().into(),
        tool: "rode_create_directory".into(),
        arguments: json!({"path":"runtime/new"}),
        read_only: false,
        protected_paths: vec![protected.to_str().unwrap().into()],
        protected_configuration: vec![],
    };
    assert!(execute(&request, &Root::open(root.to_str().unwrap(), &[]).unwrap()).is_err());
    assert!(!protected.join("new").exists());
    for index in 0..=MAX_ENTRIES {
        symlink("missing", root.join(format!("link-{index}"))).unwrap();
    }
    let listed = call(root, "rode_list_files", json!({"path":"."}), false).unwrap();
    assert_eq!(
        listed["truncated"], true,
        "Filtered symlinks still consume the scan budget"
    );
    for index in 0..10 {
        fs::write(
            protected.join(format!("{index:02}.bin")),
            vec![0; MAX_FILE as usize],
        )
        .unwrap();
    }
    let found = call(
        root,
        "rode_search_files",
        json!({"path":"runtime","text":"needle"}),
        false,
    )
    .unwrap();
    assert_eq!(
        found["truncated"], true,
        "Rejected binary reads still consume the byte budget"
    );
    assert!(
        call(
            root,
            "rode_search_files",
            json!({"path":"missing","text":"needle"}),
            false
        )
        .is_err()
    );
}

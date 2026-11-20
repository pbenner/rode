use super::*;
use std::fs;

fn job(source: &Path, destination: &Path) -> Job {
    Job {
        from: Endpoint::local(),
        to: Endpoint::local(),
        paths: vec![source.to_string_lossy().into()],
        directory: destination.to_string_lossy().into(),
        protected: Arc::default(),
    }
}
fn parts(directory: &Path) -> usize {
    fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
        .count()
}

#[test]
fn copies_binary_trees_and_empty_directories_with_bounded_blocks() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source tree");
    let destination = root.path().join("destination");
    fs::create_dir_all(source.join("empty")).unwrap();
    fs::create_dir(&destination).unwrap();
    let bytes: Vec<_> = (0..2_000_003).map(|n| (n % 239) as u8).collect();
    fs::write(source.join("binary-日本.bin"), &bytes).unwrap();
    fs::write(source.join("empty.txt"), b"").unwrap();
    let progress = Arc::new(Mutex::new(Progress::default()));
    run(job(&source, &destination), Arc::default(), progress.clone()).unwrap();
    assert_eq!(
        fs::read(destination.join("source tree/binary-日本.bin")).unwrap(),
        bytes
    );
    assert!(destination.join("source tree/empty").is_dir());
    assert_eq!(progress.lock().unwrap().files, 2);
    assert_eq!(progress.lock().unwrap().bytes, bytes.len() as u64);
}

fn conflicting(
    action: Option<Action>,
    remaining: bool,
    cancelled: bool,
    dirty: bool,
) -> (tempfile::TempDir, Result<()>, Arc<Mutex<Progress>>) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    fs::create_dir_all(source.join("folder")).unwrap();
    fs::create_dir_all(destination.join("folder")).unwrap();
    for name in ["a.txt", "b.txt"] {
        fs::write(source.join("folder").join(name), b"new").unwrap();
        fs::write(destination.join("folder").join(name), b"original").unwrap();
    }
    let job = job(&source.join("folder"), &destination);
    if dirty {
        job.protected
            .lock()
            .unwrap()
            .push(destination.join("folder/a.txt").to_string_lossy().into());
    }
    let progress = Arc::new(Mutex::new(Progress::default()));
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_progress = progress.clone();
    let worker_cancel = cancel.clone();
    let thread = std::thread::spawn(move || run(job, worker_cancel, worker_progress));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut answered = 0;
    let mut paths = std::collections::HashSet::new();
    while !thread.is_finished() {
        assert!(Instant::now() < deadline, "Conflict response timed out");
        let conflict = progress.lock().unwrap().conflict.clone();
        if let Some(conflict) = conflict {
            if cancelled {
                cancel.store(true, Ordering::Release);
            } else if let Some(action) = action.clone().filter(|_| !paths.contains(&conflict.path))
                && conflict
                    .answer
                    .try_send(Decision { action, remaining })
                    .is_ok()
            {
                answered += 1;
                paths.insert(conflict.path);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let result = thread.join().unwrap();
    if !cancelled && remaining && result.is_ok() {
        assert_eq!(answered, 1);
    }
    (root, result, progress)
}

#[test]
fn skip_replace_and_rename_apply_to_remaining_conflicts() {
    for action in [Action::Skip, Action::Replace, Action::Rename(String::new())] {
        let (root, result, progress) = conflicting(Some(action.clone()), true, false, false);
        result.unwrap();
        for name in ["a", "b"] {
            let path = root.path().join(format!("destination/folder/{name}.txt"));
            assert_eq!(
                fs::read(&path).unwrap(),
                if matches!(action, Action::Replace) {
                    b"new".as_slice()
                } else {
                    b"original".as_slice()
                }
            );
            if matches!(action, Action::Rename(_)) {
                assert_eq!(
                    fs::read(path.with_file_name(format!("{name} copy 1.txt"))).unwrap(),
                    b"new"
                );
            }
        }
        assert_eq!(
            progress.lock().unwrap().skipped,
            if matches!(action, Action::Skip) { 2 } else { 0 }
        );
    }
}

#[test]
fn automatic_rename_copies_maximum_length_unicode_files_without_changing_the_original() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    for name in [
        format!("{}.txt", "文".repeat(83)),
        format!("x.{}", "y".repeat(253)),
    ] {
        let path = source.join(&name);
        fs::write(&path, b"new bytes").unwrap();
        fs::write(destination.join(&name), b"original").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut copier = Copier {
            source: Endpoint::local().open(cancel.clone()).unwrap(),
            destination: Endpoint::local().open(cancel.clone()).unwrap(),
            progress: Arc::default(),
            cancel,
            policy: Some(Action::Rename(String::new())),
            protected: Arc::default(),
        };
        let entry = copier
            .source
            .request(Request::Stat {
                path: path.to_string_lossy().into(),
            })
            .unwrap()
            .entry
            .unwrap();
        let (target, expected) = copier
            .target(destination.join(&name).to_string_lossy().into(), false)
            .unwrap()
            .unwrap();
        copier
            .file(
                &Item {
                    path: path.to_string_lossy().into(),
                    parent: None,
                    entry,
                },
                target.clone(),
                expected,
            )
            .unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new bytes");
        assert_eq!(fs::read(destination.join(&name)).unwrap(), b"original");
        assert!(Path::new(&target).file_name().unwrap().len() <= 255);
    }
    assert_eq!(
        copy_name("folder.with.dots", 1, true),
        "folder.with.dots copy 1"
    );
}

#[test]
fn cancellation_and_dirty_destinations_keep_originals() {
    for (cancelled, dirty) in [(true, false), (false, true)] {
        let (root, result, _) = conflicting(Some(Action::Replace), true, cancelled, dirty);
        assert!(result.is_err());
        let directory = root.path().join("destination/folder");
        assert_eq!(fs::read(directory.join("a.txt")).unwrap(), b"original");
        assert_eq!(parts(&directory), 0);
    }
}

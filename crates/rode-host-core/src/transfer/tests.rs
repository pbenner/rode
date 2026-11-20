use super::*;

fn write(session: &mut Session, path: &Path, expected: Option<String>, bytes: &[u8]) {
    session
        .request(&Request::OpenWrite {
            path: path.to_string_lossy().into(),
            expected,
        })
        .unwrap();
    for block in bytes.chunks(BLOCK) {
        session
            .request(&Request::Write {
                data: STANDARD.encode(block),
            })
            .unwrap();
    }
}
fn commit(session: &mut Session, bytes: &[u8]) -> Result<Reply> {
    session.request(&Request::Commit {
        size: bytes.len() as u64,
        digest: crate::files::digest(bytes),
        executable: false,
    })
}
fn stages(directory: &Path) -> usize {
    fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".rode-transfer-")
        })
        .count()
}

#[test]
fn streams_binary_blocks_and_empty_files_without_publishing_partial_contents() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("binary.bin");
    let bytes: Vec<_> = (0..BLOCK * 3 + 19).map(|n| (n % 251) as u8).collect();
    let mut session = Session::default();
    write(&mut session, &destination, None, &bytes);
    assert!(!destination.exists());
    assert_eq!(stages(root.path()), 1);
    commit(&mut session, &bytes).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), bytes);
    assert_eq!(stages(root.path()), 0);
    let entry = inspect(&destination).unwrap().unwrap();
    session
        .request(&Request::OpenRead {
            path: destination.to_string_lossy().into(),
            version: entry.version,
        })
        .unwrap();
    let mut read = Vec::new();
    loop {
        let block = session.request(&Request::Read).unwrap();
        assert!(block.data.len() <= BLOCK.div_ceil(3) * 4);
        if block.data.is_empty() {
            assert_eq!(block.digest, crate::files::digest(&read));
            break;
        }
        read.extend(STANDARD.decode(block.data).unwrap());
    }
    assert_eq!(read, bytes);
    let empty = root.path().join("empty");
    write(&mut session, &empty, None, b"");
    commit(&mut session, b"").unwrap();
    assert_eq!(fs::metadata(empty).unwrap().len(), 0);
}

#[test]
fn collisions_changed_destinations_and_bad_digests_preserve_existing_files() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("existing");
    fs::write(&target, b"original").unwrap();
    let mut session = Session::default();
    assert!(
        session
            .request(&Request::OpenWrite {
                path: target.to_string_lossy().into(),
                expected: None
            })
            .is_err()
    );
    let original = inspect(&target).unwrap().unwrap().version;
    write(&mut session, &target, Some(original), b"replacement");
    fs::write(&target, b"external edit").unwrap();
    assert!(commit(&mut session, b"replacement").is_err());
    assert_eq!(fs::read(&target).unwrap(), b"external edit");
    assert_eq!(stages(root.path()), 0);
    let expected = inspect(&target).unwrap().unwrap().version;
    write(&mut session, &target, Some(expected), b"new bytes");
    assert!(commit(&mut session, b"different").is_err());
    assert_eq!(fs::read(&target).unwrap(), b"external edit");
    assert_eq!(stages(root.path()), 0);
    let new = root.path().join("new");
    write(&mut session, &new, None, b"upload");
    fs::write(&new, b"concurrent create").unwrap();
    assert!(commit(&mut session, b"upload").is_err());
    assert_eq!(fs::read(new).unwrap(), b"concurrent create");
}

#[test]
fn abort_drop_and_invalid_blocks_remove_only_owned_stages() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    let other = root.path().join(".rode-transfer-unrelated.part");
    fs::write(&other, b"keep").unwrap();
    let mut session = Session::default();
    write(&mut session, &target, None, b"partial");
    session.request(&Request::Abort).unwrap();
    assert!(!target.exists());
    assert_eq!(stages(root.path()), 1);
    write(&mut session, &target, None, b"partial");
    drop(session);
    assert_eq!(stages(root.path()), 1);
    let mut session = Session::default();
    write(&mut session, &target, None, b"partial");
    assert!(
        session
            .request(&Request::Write {
                data: STANDARD.encode(vec![0; BLOCK + 1])
            })
            .is_err()
    );
    assert_eq!(stages(root.path()), 1);
    assert_eq!(fs::read(other).unwrap(), b"keep");
}

#[test]
fn refuses_redirected_sources_and_destinations_and_detects_source_changes() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let link = root.path().join("link");
    fs::write(&source, b"source bytes").unwrap();
    symlink(&source, &link).unwrap();
    let mut session = Session::default();
    let entry = inspect(&link).unwrap().unwrap();
    assert!(!entry.regular && !entry.directory);
    assert!(
        session
            .request(&Request::OpenRead {
                path: link.to_string_lossy().into(),
                version: entry.version
            })
            .is_err()
    );
    assert!(
        session
            .request(&Request::OpenWrite {
                path: link.to_string_lossy().into(),
                expected: None
            })
            .is_err()
    );
    let entry = inspect(&source).unwrap().unwrap();
    session
        .request(&Request::OpenRead {
            path: source.to_string_lossy().into(),
            version: entry.version,
        })
        .unwrap();
    fs::write(&source, b"changed length").unwrap();
    assert!(session.request(&Request::Read).is_err());
    let fifo = root.path().join("fifo");
    let value = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(value.as_ptr(), 0o600) }, 0);
    let entry = inspect(&fifo).unwrap().unwrap();
    let start = std::time::Instant::now();
    assert!(
        session
            .request(&Request::OpenRead {
                path: fifo.to_string_lossy().into(),
                version: entry.version
            })
            .is_err()
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn stream_eof_removes_an_unpublished_destination() {
    use std::os::unix::net::UnixStream;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("destination");
    let (mut desktop, mut host) = UnixStream::pair().unwrap();
    let mut output = host.try_clone().unwrap();
    let worker = std::thread::spawn(move || {
        serve_stream(
            &mut host,
            &mut output,
            &std::sync::atomic::AtomicBool::new(false),
        )
    });
    let _: Result<Reply, String> = framing::read(&mut desktop).unwrap().unwrap();
    for request in [
        Request::OpenWrite {
            path: path.to_string_lossy().into(),
            expected: None,
        },
        Request::Write {
            data: STANDARD.encode(b"partial"),
        },
    ] {
        framing::write(&mut desktop, &request).unwrap();
        let reply: Result<Reply, String> = framing::read(&mut desktop).unwrap().unwrap();
        reply.unwrap();
    }
    assert_eq!(stages(root.path()), 1);
    drop(desktop);
    worker.join().unwrap().unwrap();
    assert!(!path.exists());
    assert_eq!(stages(root.path()), 0);
}

#[test]
fn directory_replacement_cannot_redirect_a_transfer_or_its_cleanup() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    let moved = root.path().join("moved");
    let other = root.path().join("other");
    fs::create_dir(&parent).unwrap();
    fs::create_dir(&other).unwrap();
    let path = parent.join("target");
    let mut session = Session::default();
    write(&mut session, &path, None, b"copy");
    fs::rename(&parent, &moved).unwrap();
    symlink(&other, &parent).unwrap();
    fs::write(other.join("target"), b"preserved").unwrap();
    assert!(commit(&mut session, b"copy").is_err());
    assert_eq!(fs::read(other.join("target")).unwrap(), b"preserved");
    assert!(!moved.join("target").exists());
    assert_eq!(stages(&moved), 0);
    let entry = inspect(&path).unwrap().unwrap();
    assert!(
        session
            .request(&Request::OpenRead {
                path: path.to_string_lossy().into(),
                version: entry.version
            })
            .is_err()
    );
}

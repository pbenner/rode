use super::*;

fn server(root: &Path, read_only: bool) -> Server {
    Server {
        root: Dir::open_ambient_dir(root, cap_std::ambient_authority()).unwrap(),
        read_only,
        handles: HashMap::new(),
        next: 0,
    }
}
fn path(value: &str) -> Vec<u8> {
    let mut out = vec![];
    string(&mut out, value.as_bytes());
    out
}
fn request(server: &mut Server, kind: u8, body: &[u8]) -> io::Result<Vec<u8>> {
    server.request(kind, 1, &mut Reader(body))
}
fn open(server: &mut Server, name: &str, flags: u32) -> io::Result<u32> {
    let mut body = path(name);
    u32(&mut body, flags);
    u32(&mut body, 0);
    let response = request(server, 3, &body)?;
    let mut r = Reader(&response[5..]);
    r.handle()
}

#[test]
fn capabilities_block_parent_and_symlink_escapes_and_read_only_cannot_mutate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("shared");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(temp.path().join("secret"), "outside").unwrap();
    std::fs::write(root.join("file"), "original").unwrap();
    std::os::unix::fs::symlink(temp.path(), root.join("escape")).unwrap();
    std::os::unix::fs::symlink(temp.path().join("secret"), root.join("link")).unwrap();
    let mut server = server(&root, true);
    for escape in ["../secret", "/../secret", "escape/secret", "link"] {
        assert!(open(&mut server, escape, 1).is_err(), "{escape}");
    }
    let h = open(&mut server, "file", 1).unwrap();
    let mut body = path(&h.to_string());
    body.extend(0u64.to_be_bytes());
    u32(&mut body, 64);
    let result = request(&mut server, 5, &body).unwrap();
    assert_eq!(&result[9..], b"original");
    for flags in [2, 10, 26, 42] {
        assert!(open(&mut server, "file", flags).is_err());
    }
    for op in [6, 9, 10, 13, 14, 15, 18] {
        assert_eq!(
            request(&mut server, op, &[]).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    let mut body = path("posix-rename@openssh.com");
    body.extend(path("file"));
    body.extend(path("other"));
    assert!(request(&mut server, 200, &body).is_err());
    assert_eq!(std::fs::read(root.join("file")).unwrap(), b"original");
    assert_eq!(
        std::fs::read(temp.path().join("secret")).unwrap(),
        b"outside"
    );
}

#[test]
fn writable_protocol_reads_writes_truncates_and_renames_without_replacing_v3_destinations() {
    let temp = tempfile::tempdir().unwrap();
    let mut server = server(temp.path(), false);
    let h = open(&mut server, "new", 2 | 8 | 32).unwrap();
    let mut body = path(&h.to_string());
    body.extend(0u64.to_be_bytes());
    string(&mut body, b"hello world");
    request(&mut server, 6, &body).unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("new")).unwrap(),
        b"hello world"
    );
    let mut attrs = path(&h.to_string());
    u32(&mut attrs, 1);
    attrs.extend(5u64.to_be_bytes());
    request(&mut server, 10, &attrs).unwrap();
    assert_eq!(std::fs::read(temp.path().join("new")).unwrap(), b"hello");
    std::fs::write(temp.path().join("existing"), b"keep").unwrap();
    let mut rename = path("new");
    rename.extend(path("existing"));
    assert!(request(&mut server, 18, &rename).is_err());
    assert_eq!(
        std::fs::read(temp.path().join("existing")).unwrap(),
        b"keep"
    );
    let mut rename = path("new");
    rename.extend(path("renamed"));
    request(&mut server, 18, &rename).unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("renamed")).unwrap(),
        b"hello"
    );
    assert!(request(&mut server, 13, &path("../secret")).is_err());
}

#[test]
fn frames_and_headers_reject_oversized_and_truncated_input_before_service_work() {
    let temp = tempfile::tempdir().unwrap();
    let export = Export {
        root: temp.path().to_str().unwrap().into(),
        read_only: true,
        plan: None,
    };
    for bytes in [
        u32::MAX.to_be_bytes().to_vec(),
        vec![0, 0, 0, 5, 1, 0],
        vec![0, 0, 0, 0],
    ] {
        assert!(serve(&export, &mut bytes.as_slice(), &mut Vec::new()).is_err());
    }
    let mut input = &b"{\"root\":\"/tmp/share\",\"read_only\":true,\"plan\":null}\nSFTP"[..];
    let parsed: Export = header(&mut input).unwrap();
    assert!(parsed.read_only);
    assert_eq!(input, b"SFTP");
    assert!(header::<Export>(&mut &vec![b'x'; 128 * 1024][..]).is_err());
}

#[test]
fn append_handles_append_even_when_a_client_supplies_an_old_offset() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("log"), b"first").unwrap();
    let mut server = server(temp.path(), false);
    let handle = open(&mut server, "log", 2 | 4).unwrap();
    let mut bytes = path(&handle.to_string());
    bytes.extend(0u64.to_be_bytes());
    string(&mut bytes, b"second");
    request(&mut server, 6, &bytes).unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("log")).unwrap(),
        b"firstsecond"
    );
}

#[test]
fn readdir_does_not_report_eof_before_exhausting_excluded_entries() {
    let temp = tempfile::tempdir().unwrap();
    for index in 0..1100 {
        std::os::unix::fs::symlink("missing", temp.path().join(format!("link-{index}"))).unwrap();
    }
    let mut server = server(temp.path(), true);
    let opened = request(&mut server, 11, &path(".")).unwrap();
    let handle = Reader(&opened[5..]).handle().unwrap();
    let response = request(&mut server, 12, &path(&handle.to_string())).unwrap();
    assert_eq!(response[0], 101);
    assert_eq!(&response[5..9], &1u32.to_be_bytes());
    let Some(Handle::Directory(entries)) = server.handles.get_mut(&handle) else {
        panic!("Missing directory handle");
    };
    assert!(entries.next().is_none(), "EOF hid unread directory entries");
}

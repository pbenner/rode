use super::*;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::PermissionsExt;

#[test]
fn service_startup_never_appends_to_a_redirected_log() {
    use std::os::unix::fs::symlink;
    for hard_link in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        private_directory_at(&runtime).unwrap();
        let victim = root.path().join("unrelated.txt");
        fs::write(&victim, "untouched\n").unwrap();
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o600)).unwrap();
        if hard_link {
            fs::hard_link(&victim, runtime.join("service.log")).unwrap();
        } else {
            symlink(&victim, runtime.join("service.log")).unwrap();
        }
        let launcher = Launcher {
            executable: "/bin/sh".into(),
            prefix: vec![
                "-c".into(),
                "printf 'redirected service error\\n' >&2; exit 1".into(),
                "log-fixture".into(),
            ],
        };
        let request = Request::Create {
            id: new_id().unwrap(),
            name: "Log fixture".into(),
            root: root.path().to_string_lossy().into(),
            policy: "a".repeat(64),
            mode: "NORMAL MODE".into(),
            argv: vec!["/bin/sh".into()],
            initial_input: String::new(),
            env: BTreeMap::new(),
            persistent: false,
            allocation: None,
        };
        assert!(request_once(&request, &launcher, &runtime).is_err());
        assert_eq!(
            fs::read_to_string(&victim).unwrap(),
            "untouched\n",
            "Service log followed an untrusted link"
        );
    }
}

#[test]
fn runtime_directory_replacement_cannot_redirect_host_artifact_writes() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("runtime");
    let directory = private_directory_at(&runtime).unwrap();
    let moved = root.path().join("original");
    let victim = root.path().join("other");
    fs::create_dir(&victim).unwrap();
    fs::write(victim.join("service.log"), "untouched").unwrap();
    fs::rename(&runtime, &moved).unwrap();
    symlink(&victim, &runtime).unwrap();
    let mut log = rode_host_core::locking::open_private(
        &runtime_path(&runtime, &directory).join("service.log"),
        rode_host_core::locking::PrivateFile::Append,
    )
    .unwrap();
    log.write_all(b"private log").unwrap();
    assert_eq!(
        fs::read_to_string(victim.join("service.log")).unwrap(),
        "untouched"
    );
    assert_eq!(
        fs::read_to_string(moved.join("service.log")).unwrap(),
        "private log"
    );
}

#[test]
fn authenticated_control_requires_the_current_protocol() {
    for protocol in [PROTOCOL, PROTOCOL - 1] {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let address = control_address(&fs::metadata(dir.path()).unwrap()).unwrap();
        let token = control_auth::Token::publish(dir.path()).unwrap();
        let listener = UnixListener::bind_addr(&address).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            token.verify(&mut stream).unwrap();
            send_json(&mut stream, &Response::Ok).unwrap();
            assert!(matches!(
                recv_json::<_, Request>(&mut stream).unwrap(),
                Request::Protocol
            ));
            send_json(&mut stream, &Response::Protocol(protocol)).unwrap();
        });
        let result = connect_control(dir.path());
        server.join().unwrap();
        if protocol == PROTOCOL {
            assert_eq!(
                result.unwrap().peer_addr().unwrap().as_abstract_name(),
                address.as_abstract_name()
            );
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("protocol mismatch")
            );
        }
    }
}

#[test]
fn authentication_disconnects_preserve_the_transport_error_for_restart_retries() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let address = control_address(&fs::metadata(dir.path()).unwrap()).unwrap();
    let token = control_auth::Token::publish(dir.path()).unwrap();
    let listener = UnixListener::bind_addr(&address).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        token.verify(&mut stream).unwrap();
        // Shutdown can close a connected client before authentication is acknowledged.
    });
    let error = connect_control(dir.path()).unwrap_err();
    server.join().unwrap();
    assert!(error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::UnexpectedEof)
    }));
}

#[test]
fn capability_errors_allow_restart_retries_only_after_the_connected_peer_disconnects() {
    for disconnected in [false, true] {
        for errno in [libc::EACCES, libc::EPERM, libc::ENOENT] {
            let (client, server) = UnixStream::pair().unwrap();
            let _server = (!disconnected).then_some(server);
            let error = control_authentication_error(
                &client,
                std::io::Error::from_raw_os_error(errno).into(),
            )
            .context(ControlAuthentication);
            assert!(
                !service_offline(&error),
                "Authentication never proves service absence"
            );
            assert_eq!(
                error.downcast_ref::<ControlDisconnected>().is_some(),
                disconnected
            );
            assert!(
                error.chain().any(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.raw_os_error() == Some(errno))
                }),
                "The original capability error must remain available"
            );
        }
    }
}

#[test]
fn control_never_uses_unauthenticated_endpoints() {
    for protected in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let _discovery = UnixListener::bind(dir.path().join("socket")).unwrap();
        let meta = fs::metadata(dir.path()).unwrap();
        let address = std::os::unix::net::SocketAddr::from_abstract_name(format!(
            "rode-session-{}-{}-{}",
            meta.uid(),
            meta.dev(),
            meta.ino()
        ))
        .unwrap();
        let _unprotected = UnixListener::bind_addr(&address).unwrap();
        let _protected =
            protected.then(|| UnixListener::bind_addr(&control_address(&meta).unwrap()).unwrap());
        let error = connect_control(dir.path()).unwrap_err();
        assert!(format!("{error:#}").contains("outside the project sandbox"));
        assert_eq!(
            service_offline(&error),
            !protected,
            "A live listener without its capability must not disappear from discovery or acknowledge shutdown"
        );
        fs::remove_file(dir.path().join("socket")).unwrap();
        let launcher = Launcher {
            executable: "/unused-helper".into(),
            prefix: vec![],
        };
        let error = request_once(&Request::List, &launcher, dir.path()).unwrap_err();
        assert_eq!(service_offline(&error), !protected);
    }
}

#[test]
fn namespace_checks_deny_unreadable_proc_entries_and_keep_the_underlying_error() {
    for peer_denied in [false, true] {
        for errno in [libc::EACCES, libc::EPERM, libc::ENOENT] {
            let denied = Err(std::io::Error::from_raw_os_error(errno));
            let visible = Ok(PathBuf::from("mnt:[1]"));
            let error = if peer_denied {
                verify_control_namespace(denied, visible)
            } else {
                verify_control_namespace(visible, denied)
            }
            .unwrap_err();
            assert!(error.to_string().contains("outside the project sandbox"));
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(errno)
            );
        }
    }
    assert!(verify_control_namespace(Ok("mnt:[1]".into()), Ok("mnt:[1]".into())).is_ok());
    let error = verify_control_namespace(Ok("mnt:[2]".into()), Ok("mnt:[1]".into())).unwrap_err();
    assert!(error.to_string().contains("outside the project sandbox"));
}

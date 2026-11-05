use super::*;
use std::os::linux::net::SocketAddrExt;

#[test]
fn service_starting_after_initial_refusal_uses_protected_control() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let address = control_address(&fs::metadata(dir.path()).unwrap()).unwrap();
    assert_eq!(
        UnixStream::connect_addr(&address).unwrap_err().kind(),
        std::io::ErrorKind::ConnectionRefused
    );

    // Publish the service between the client's initial refusal and fallback,
    // in the same order as serve(). No scheduler timing or sleeps are needed.
    let token = control_auth::Token::publish(dir.path()).unwrap();
    let protected = UnixListener::bind_addr(&address).unwrap();
    let _discovery = UnixListener::bind(dir.path().join("socket")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = protected.accept().unwrap();
        token.verify(&mut stream).unwrap();
        send_json(&mut stream, &Response::Ok).unwrap();
    });
    let stream = connect_control_fallback(dir.path(), &address).unwrap();
    server.join().unwrap();
    assert_eq!(
        stream.peer_addr().unwrap().as_abstract_name(),
        address.as_abstract_name(),
        "The discovery socket rejects control requests"
    );
}

#[test]
fn legacy_abstract_control_remains_available_without_an_authenticated_listener() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let address = legacy_control_address(&fs::metadata(dir.path()).unwrap()).unwrap();
    let _listener = UnixListener::bind_addr(&address).unwrap();
    let stream = connect_control(dir.path()).unwrap();
    assert_eq!(
        stream.peer_addr().unwrap().as_abstract_name(),
        address.as_abstract_name()
    );
}

#[test]
fn an_authentication_failure_never_falls_back_to_legacy_control() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let _legacy = UnixListener::bind(dir.path().join("socket")).unwrap();
    let address = control_address(&fs::metadata(dir.path()).unwrap()).unwrap();
    let _protected = UnixListener::bind_addr(&address).unwrap();
    // The current endpoint exists, but its capability is missing. A legacy
    // listener must not turn this into a successful unauthenticated connection.
    let error = connect_control(dir.path()).unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains("outside the project sandbox"),
        "{diagnostic}"
    );
    let cause = std::io::Error::from_raw_os_error(libc::ENOENT).to_string();
    assert!(diagnostic.contains(&cause), "{diagnostic}");
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

#[test]
fn legacy_control_remains_available_without_a_protected_listener() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("socket");
    let _legacy = UnixListener::bind(&path).unwrap();
    let stream = connect_control(dir.path()).unwrap();
    assert_eq!(
        stream.peer_addr().unwrap().as_pathname(),
        Some(path.as_path())
    );
}

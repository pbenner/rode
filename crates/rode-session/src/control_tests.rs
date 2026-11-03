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
    let _protected = UnixListener::bind_addr(&address).unwrap();
    let _discovery = UnixListener::bind(dir.path().join("socket")).unwrap();
    let stream = connect_control_fallback(dir.path(), &address).unwrap();
    assert_eq!(
        stream.peer_addr().unwrap().as_abstract_name(),
        address.as_abstract_name(),
        "The discovery socket rejects control requests"
    );
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

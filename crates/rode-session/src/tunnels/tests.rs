//! Real SSH transport without an external ssh client, daemon or display server.
use super::*;
use russh::{
    keys::{Algorithm, PrivateKey},
    server::{self, Server as _},
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[derive(Clone)]
struct Relay {
    allowed: keys::PublicKey,
    handles: Arc<Mutex<Vec<server::Handle>>>,
    auths: Arc<AtomicUsize>,
    deny_forwarding: Arc<AtomicBool>,
}
struct Peer {
    relay: Relay,
    listener: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.take() {
            listener.abort();
        }
    }
}
impl server::Server for Relay {
    type Handler = Peer;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Peer {
        Peer {
            relay: self.clone(),
            listener: None,
        }
    }
}
impl server::Handler for Peer {
    type Error = anyhow::Error;
    async fn auth_publickey(
        &mut self,
        _: &str,
        key: server::VerifiedPublicKey<'_>,
    ) -> Result<server::Auth> {
        self.relay.auths.fetch_add(1, Ordering::SeqCst);
        Ok(if key.public_key == &self.relay.allowed {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }
    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut server::Session,
    ) -> Result<bool> {
        ensure!(address == "127.0.0.1", "Non-loopback forwarding request");
        if self.relay.deny_forwarding.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let Ok(listener) = TcpListener::bind((address, *port as u16)).await else {
            return Ok(false);
        };
        let handle = session.handle();
        self.relay.handles.lock().unwrap().push(handle.clone());
        let port = *port;
        self.listener = Some(tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let handle = handle.clone();
                tokio::spawn(async move {
                    if let Ok(channel) = handle
                        .channel_open_forwarded_tcpip("127.0.0.1", port, "127.0.0.1", 54321)
                        .await
                    {
                        let _ =
                            tokio::io::copy_bidirectional(&mut stream, &mut channel.into_stream())
                                .await;
                    }
                });
            }
        }));
        Ok(true)
    }
}
fn key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}
fn spec() -> Spec {
    Spec {
        relay_host: "127.0.0.1".into(),
        relay_user: "compute".into(),
        ..Default::default()
    }
}
async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(12), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Tunnel state timed out");
}
async fn roundtrip(port: u16) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream
            .write_all(b"SSH-2.0-compute-fixture\r\n")
            .await
            .unwrap();
        let mut bytes = vec![0; 25];
        stream.read_exact(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"SSH-2.0-compute-fixture\r\n");
    })
    .await
    .unwrap();
}

fn assert_listener_refusal(info: &Info) {
    assert_eq!(info.status, Status::Reconnecting);
    for detail in [
        format!(
            "SSH login as {} to relay {} succeeded",
            info.spec.relay_user,
            relay_endpoint(&info.spec)
        ),
        format!(
            "refused tunnel listener 127.0.0.1:{}",
            info.spec.listen_port
        ),
        "SSH did not report why".into(),
        format!(
            "On the relay, run: ss -ltnp 'sport = :{}'",
            info.spec.listen_port
        ),
        "sshd-session".into(),
        "Relay port under Port forwarding".into(),
        "AllowTcpForwarding".into(),
        "authorized_keys".into(),
        "RODE will retry automatically".into(),
        format!("SSH detail: {}", russh::Error::RequestDenied),
    ] {
        assert!(
            info.error.contains(&detail),
            "Missing {detail}: {}",
            info.error
        );
    }
}

#[test]
fn tunnel_forwarding_errors_distinguish_refusal_from_lost_transport() {
    let settings = Spec {
        relay_host: "2001:db8::1".into(),
        relay_port: 8443,
        ..spec()
    };
    let error = forwarding_error(&settings, russh::Error::Disconnect);
    assert!(matches!(
        error.downcast_ref::<russh::Error>(),
        Some(russh::Error::Disconnect)
    ));
    let message = format!("{error:#}");
    assert!(message.contains("[2001:db8::1]:8443"), "{message}");
    assert!(message.contains("127.0.0.1:22022"), "{message}");
    assert!(message.contains("could not complete"), "{message}");
    assert!(
        !message.contains("refused") && !message.contains("already in use"),
        "{message}"
    );

    // Full diagnostics must survive the service's existing 2048-character limit,
    // even with the largest allowed host and account names.
    let settings = Spec {
        relay_host: "h".repeat(253),
        relay_user: "u".repeat(253),
        ..settings
    };
    settings.validate().unwrap();
    let error = forwarding_error(&settings, russh::Error::RequestDenied);
    assert!(matches!(
        error.downcast_ref::<russh::Error>(),
        Some(russh::Error::RequestDenied)
    ));
    let message = format!("{error:#}");
    assert!(message.chars().count() <= 2048, "{message}");
    assert!(message.contains("SSH did not report why"));
}
#[test]
fn validates_bounded_explicit_credentials_and_ports() {
    assert!(spec().validate().is_ok());
    for value in ["", "-relay", "relay\nhost", "user@host"] {
        assert!(
            Spec {
                relay_host: value.into(),
                ..spec()
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        Spec {
            listen_port: 0,
            ..spec()
        }
        .validate()
        .is_err()
    );
    assert!(
        Spec {
            identity: String::new(),
            ..spec()
        }
        .validate()
        .is_err()
    );
    assert!(
        Spec {
            identity: "relative".into(),
            ..spec()
        }
        .validate()
        .is_err()
    );
    assert!(
        Spec {
            relay_key: "invalid".into(),
            ..spec()
        }
        .validate()
        .is_err()
    );
    let store = Store::default();
    assert!(store.start("not-a-service-id".into(), spec()).is_err());
    assert!(!store.live());
}

#[test]
fn tunnel_identity_errors_identify_the_file_and_preserve_the_cause() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing identity");
    let error = read_identity(&missing).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotFound
    );
    let message = format!("{error:#}");
    assert!(message.contains(missing.to_str().unwrap()), "{message}");
    assert!(
        message.contains("Identity file") && message.contains("tunnel host"),
        "{message}"
    );
    assert!(message.contains("Agent socket"), "{message}");

    let invalid = directory.path().join("invalid-key");
    let private_content = "invalid-private-key-content-must-not-be-displayed";
    std::fs::write(&invalid, private_content).unwrap();
    let message = format!("{:#}", read_identity(&invalid).unwrap_err());
    assert!(message.contains(invalid.to_str().unwrap()), "{message}");
    assert!(message.contains("decode"), "{message}");
    assert!(!message.contains(private_content));
}

#[test]
fn tunnel_identity_fifo_is_rejected_without_waiting_for_a_writer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("identity-fifo");
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let (tx, rx) = std::sync::mpsc::channel();
    let source = path.clone();
    let reader = std::thread::spawn(move || {
        let _ = tx.send(read_identity(&source).map(|_| ()));
    });
    let result = rx.recv_timeout(Duration::from_secs(2));
    // Release a broken reader before asserting, so a regression cannot leak a
    // blocked test thread or keep the fixture open indefinitely.
    let _release = result.is_err().then(|| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
            .unwrap()
    });
    reader.join().unwrap();
    assert!(
        result
            .expect("Identity open waited for a FIFO writer")
            .unwrap_err()
            .to_string()
            .contains("regular file")
    );
}

#[tokio::test]
async fn verifies_relay_forwards_reconnects_and_stops_only_its_worker() {
    let directory = tempfile::tempdir().unwrap();
    let identity = key();
    let identity_path = directory.path().join("identity");
    std::fs::write(
        &identity_path,
        identity
            .to_openssh(keys::ssh_key::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let host_key = key();
    let alternate_key = PrivateKey::random(
        &mut rand::rng(),
        Algorithm::Ecdsa {
            curve: keys::EcdsaCurve::NistP256,
        },
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_port = listener.local_addr().unwrap().port();
    let reserved = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen_port = reserved.local_addr().unwrap().port();
    drop(reserved);
    let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ssh_port = echo.local_addr().unwrap().port();
    let echo_task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = echo.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = socket.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let relay = Relay {
        allowed: identity.public_key().clone(),
        handles: Default::default(),
        auths: Default::default(),
        deny_forwarding: Default::default(),
    };
    let mut server = relay.clone();
    let config = Arc::new(server::Config {
        keys: vec![host_key.clone(), alternate_key.clone()],
        auth_rejection_time: Duration::ZERO,
        ..Default::default()
    });
    let server_task = tokio::spawn(async move {
        server.run_on_socket(config, &listener).await.unwrap();
    });
    let store = Store::default();
    let id = crate::new_id().unwrap();
    let mut settings = Spec {
        relay_port,
        listen_port,
        ssh_port,
        identity: identity_path.to_string_lossy().into(),
        ..spec()
    };
    store.start(id.clone(), settings.clone()).unwrap();
    until(|| store.list()[0].status == Status::HostKeyRequired).await;
    let untrusted = store.list().remove(0);
    assert_eq!(untrusted.hostname, local_hostname().unwrap());
    // New clients still accept retained tunnels from services started before
    // hostname metadata was introduced, without restarting those services.
    let mut legacy = serde_json::to_value(&untrusted).unwrap();
    legacy.as_object_mut().unwrap().remove("hostname");
    assert!(
        serde_json::from_value::<Info>(legacy)
            .unwrap()
            .hostname
            .is_empty()
    );
    assert_eq!(
        relay.auths.load(Ordering::SeqCst),
        0,
        "No credentials before relay verification"
    );
    assert_eq!(
        untrusted.observed_key,
        host_key.public_key().to_openssh().unwrap()
    );
    assert!(
        TcpStream::connect(("127.0.0.1", listen_port))
            .await
            .is_err()
    );
    // A changed key remains blocked, including when another key would authenticate.
    settings.relay_key = key().public_key().to_openssh().unwrap();
    store.start(id.clone(), settings.clone()).unwrap();
    until(|| store.list()[0].status == Status::HostKeyRequired).await;
    assert!(store.list()[0].error.contains("CHANGED"));
    settings.relay_key = untrusted.observed_key;
    let reserved = TcpListener::bind(("127.0.0.1", listen_port)).await.unwrap();
    store.start(id.clone(), settings.clone()).unwrap();
    // Another process already owns the relay port. SSH supplies only a refusal,
    // so report the successful login and diagnostics without inventing a cause.
    until(|| store.list()[0].status == Status::Reconnecting).await;
    assert_listener_refusal(&store.list()[0]);
    assert!(relay.auths.load(Ordering::SeqCst) > 0);
    // Keep retrying: releasing a conflicting listener must recover unattended.
    drop(reserved);
    until(|| store.list()[0].status == Status::Connected).await;
    assert!(store.list()[0].error.is_empty());
    assert!(store.live());
    roundtrip(listen_port).await;
    // Prefer the explicitly pinned ECDSA identity even when the relay also offers Ed25519.
    settings.relay_key = alternate_key.public_key().to_openssh().unwrap();
    store.start(id.clone(), settings.clone()).unwrap();
    until(|| store.list()[0].status == Status::Connected).await;
    assert_eq!(store.list()[0].observed_key, settings.relay_key);
    roundtrip(listen_port).await;
    let attempts = store.list()[0].attempts;
    store.start(id.clone(), settings.clone()).unwrap();
    assert_eq!(
        store.list()[0].attempts,
        attempts,
        "Repeated Start must be idempotent"
    );
    let conflict = store
        .start(crate::new_id().unwrap(), settings.clone())
        .unwrap_err()
        .to_string();
    for detail in [
        settings.name.clone(),
        format!("127.0.0.1:{listen_port}"),
        relay_endpoint(&settings),
        "Relay port under Port forwarding".into(),
    ] {
        assert!(conflict.contains(&detail), "{conflict}");
    }
    let handle = relay.handles.lock().unwrap().last().unwrap().clone();
    assert!(
        handle
            .channel_open_forwarded_tcpip("0.0.0.0", listen_port.into(), "127.0.0.1", 1)
            .await
            .is_err()
    );
    assert!(
        handle
            .channel_open_forwarded_tcpip("127.0.0.1", u32::from(listen_port) + 1, "127.0.0.1", 1)
            .await
            .is_err()
    );
    // A policy refusal is indistinguishable from a port conflict on the wire.
    relay.deny_forwarding.store(true, Ordering::SeqCst);
    handle
        .disconnect(
            russh::Disconnect::ByApplication,
            "test disconnect".into(),
            String::new(),
        )
        .await
        .unwrap();
    until(|| {
        let info = &store.list()[0];
        info.attempts > attempts && info.status == Status::Reconnecting
    })
    .await;
    assert_listener_refusal(&store.list()[0]);
    relay.deny_forwarding.store(false, Ordering::SeqCst);
    until(|| {
        let info = &store.list()[0];
        info.attempts > attempts && info.status == Status::Connected
    })
    .await;
    assert!(store.list()[0].error.is_empty());
    roundtrip(listen_port).await;
    let mut active = TcpStream::connect(("127.0.0.1", listen_port))
        .await
        .unwrap();
    active.write_all(b"live").await.unwrap();
    let mut bytes = [0; 4];
    active.read_exact(&mut bytes).await.unwrap();
    store.stop(&id);
    assert!(!store.live());
    let closed = tokio::time::timeout(Duration::from_secs(3), active.read(&mut bytes))
        .await
        .unwrap();
    assert!(matches!(closed, Ok(0) | Err(_)));
    until(|| relay.handles.lock().unwrap().len() >= 2).await;
    // Stop joins the worker; its listener is removed when the relay observes EOF.
    tokio::time::timeout(Duration::from_secs(3), async {
        while TcpStream::connect(("127.0.0.1", listen_port)).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    // Invalid unattended credentials require attention instead of repeated login attempts.
    settings.identity = directory
        .path()
        .join("missing-key")
        .to_string_lossy()
        .into();
    store.start(id.clone(), settings.clone()).unwrap();
    until(|| store.list()[0].status == Status::Attention).await;
    let failed = store.list().remove(0);
    assert!(
        failed.error.contains(&settings.identity),
        "{}",
        failed.error
    );
    assert!(failed.error.contains("Identity file"), "{}", failed.error);
    assert!(store.live(), "Attention states must remain discoverable");
    // An unavailable persistent agent needs its exact socket and corrective field,
    // not just the operating system's unhelpful ENOENT string.
    settings.identity.clear();
    settings.agent_socket = directory
        .path()
        .join("missing-agent.sock")
        .to_string_lossy()
        .into();
    store.start(id.clone(), settings.clone()).unwrap();
    until(|| store.list()[0].status == Status::Attention).await;
    let failed = store.list().remove(0);
    assert!(
        failed.error.contains(&settings.agent_socket),
        "{}",
        failed.error
    );
    assert!(failed.error.contains("Agent socket"), "{}", failed.error);
    assert!(failed.error.contains("tunnel host"), "{}", failed.error);
    store.stop(&id);
    server_task.abort();
    echo_task.abort();
}

#[test]
fn tunnel_admin_operation_cannot_dispatch_unrelated_service_requests() {
    let operation = crate::workspace::Operation::Tunnels {
        request: Box::new(crate::Request::Shutdown),
    };
    let launcher = crate::Launcher {
        executable: "/unused-helper".into(),
        prefix: vec![],
    };
    let error = crate::workspace::local(&operation, launcher).unwrap_err();
    assert!(error.to_string().contains("Only tunnel control"));
}

#[tokio::test]
async fn tunnel_workers_are_bounded_and_stop_independently_during_connection_attempts() {
    // Leave TCP handshakes waiting for an SSH banner to exercise cancellation.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_port = listener.local_addr().unwrap().port();
    let store = Store::default();
    let mut ids = vec![];
    for n in 0..MAX_TUNNELS {
        let id = crate::new_id().unwrap();
        store
            .start(
                id.clone(),
                Spec {
                    relay_port,
                    listen_port: 22022 + n as u16,
                    ..spec()
                },
            )
            .unwrap();
        ids.push(id);
    }
    assert!(
        store
            .start(
                crate::new_id().unwrap(),
                Spec {
                    relay_port,
                    listen_port: 23000,
                    ..spec()
                }
            )
            .unwrap_err()
            .to_string()
            .contains("At most")
    );
    let start = std::time::Instant::now();
    store.stop(&ids[0]);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "Stop must interrupt the SSH handshake"
    );
    assert_eq!(store.list().len(), MAX_TUNNELS - 1);
    assert!(store.live());
    for id in &ids[1..] {
        store.stop(id);
    }
    assert!(!store.live());
}

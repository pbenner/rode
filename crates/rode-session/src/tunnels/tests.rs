//! Real SSH transport without an external ssh client, daemon or display server.
use super::*;
use russh::{
    keys::{Algorithm, PrivateKey},
    server::{self, Server as _},
};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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
        ensure!(
            matches!(address, "127.0.0.1" | ""),
            "Unexpected forwarding address"
        );
        if self.relay.deny_forwarding.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let bind = if address.is_empty() {
            "0.0.0.0"
        } else {
            address
        };
        let Ok(listener) = TcpListener::bind((bind, *port as u16)).await else {
            return Ok(false);
        };
        let handle = session.handle();
        self.relay.handles.lock().unwrap().push(handle.clone());
        let port = *port;
        let address = address.to_owned();
        self.listener = Some(tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let handle = handle.clone();
                let address = address.clone();
                tokio::spawn(async move {
                    if let Ok(channel) = handle
                        .channel_open_forwarded_tcpip(address, port, "127.0.0.1", 54321)
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
fn write_private(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
    std::fs::write(&path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn spec() -> Spec {
    Spec {
        relay_host: "127.0.0.1".into(),
        relay_user: "compute".into(),
        identity: "/fixture/relay-key".into(),
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

#[cfg(target_os = "linux")]
fn local_forward_sockets(port: u16) -> usize {
    let sockets: std::collections::HashSet<_> = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|entry| {
            let target = std::fs::read_link(entry.ok()?.path()).ok()?;
            Some(
                target
                    .to_str()?
                    .strip_prefix("socket:[")?
                    .strip_suffix(']')?
                    .to_owned(),
            )
        })
        .collect();
    let suffix = format!(":{port:04X}");
    std::fs::read_to_string("/proc/self/net/tcp")
        .unwrap()
        .lines()
        .skip(1)
        .filter(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields.len() > 9 && fields[2].ends_with(&suffix) && sockets.contains(fields[9])
        })
        .count()
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn relay_disconnect_releases_forwards_even_when_local_peers_stay_open() {
    let directory = tempfile::tempdir().unwrap();
    let identity = key();
    let identity_path = directory.path().join("identity");
    write_private(
        &identity_path,
        identity
            .to_openssh(keys::ssh_key::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    );
    let host_key = key();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_port = listener.local_addr().unwrap().port();
    let reserved = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen_port = reserved.local_addr().unwrap().port();
    drop(reserved);
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ssh_port = target.local_addr().unwrap().port();
    let relay = Relay {
        allowed: identity.public_key().clone(),
        handles: Default::default(),
        auths: Default::default(),
        deny_forwarding: Default::default(),
    };
    let mut server = relay.clone();
    let config = Arc::new(server::Config {
        keys: vec![host_key.clone()],
        auth_rejection_time: Duration::ZERO,
        ..Default::default()
    });
    let task = tokio::spawn(async move {
        server.run_on_socket(config, &listener).await.unwrap();
    });
    let store = Store::default();
    let id = crate::new_id().unwrap();
    store
        .start(
            id.clone(),
            Spec {
                relay_port,
                listen_port,
                ssh_port,
                identity: identity_path.to_string_lossy().into(),
                relay_key: host_key.public_key().to_openssh().unwrap(),
                ..spec()
            },
        )
        .unwrap();
    let mut clients = vec![];
    let mut local_peers = vec![];
    for _ in 0..3 {
        until(|| store.list()[0].status == Status::Connected).await;
        clients.push(
            TcpStream::connect(("127.0.0.1", listen_port))
                .await
                .unwrap(),
        );
        local_peers.push(
            tokio::time::timeout(Duration::from_secs(3), target.accept())
                .await
                .unwrap()
                .unwrap()
                .0,
        );
        assert_eq!(local_forward_sockets(ssh_port), 1);
        let handles = std::mem::take(&mut *relay.handles.lock().unwrap());
        for handle in handles {
            handle
                .disconnect(
                    russh::Disconnect::ByApplication,
                    "fixture reconnect".into(),
                    "en".into(),
                )
                .await
                .unwrap();
        }
        // Quiet peers deliberately retain their write half after remote EOF.
        // Losing the parent SSH session must still release these local sockets
        // before another connection gets its fresh forwarding-slot budget.
        until(|| local_forward_sockets(ssh_port) == 0).await;
    }
    store.stop(&id);
    assert_eq!(local_forward_sockets(ssh_port), 0);
    drop(local_peers);
    drop(clients);
    task.abort();
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
fn blank_credentials_use_defaults_and_explicit_key_or_agent_settings_are_preserved() {
    let defaults = Spec::default();
    assert!(defaults.identity.is_empty() && defaults.agent_socket.is_empty());
    let mut settings: Spec = serde_json::from_value(serde_json::json!({
        "relay_host": "relay.example.org", "relay_user": "researcher"
    }))
    .unwrap();
    assert!(settings.identity.is_empty() && settings.agent_socket.is_empty());
    settings.validate().unwrap();
    assert!(settings.automatic_identity());
    assert_eq!(
        settings.identity_files(),
        DEFAULT_IDENTITIES.map(rode_host_core::files::expand_home)
    );
    for identity in ["~/.ssh/custom-relay-key", "~/.ssh/id_ed25519"] {
        settings.identity = identity.into();
        settings.validate().unwrap();
        assert!(!settings.automatic_identity());
        assert_eq!(
            settings.identity_files(),
            [rode_host_core::files::expand_home(identity)]
        );
        let restored: Spec =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored, settings);
    }
    settings.identity.clear();
    settings.agent_socket = "/run/user/1000/agent.sock".into();
    settings.validate().unwrap();
    assert!(!settings.automatic_identity());
    assert!(settings.identity_files().is_empty());
    let restored: Spec = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert_eq!(restored, settings);
}

#[test]
fn listener_scope_defaults_to_loopback_and_only_accepts_requested_address_kinds() {
    let legacy: Spec = serde_json::from_value(serde_json::json!({
        "relay_host": "relay.example.org", "relay_user": "researcher"
    }))
    .unwrap();
    assert_eq!(legacy.listen_on, ListenOn::Loopback);
    assert_eq!(legacy.listener(), "127.0.0.1:22022");
    for address in ["", "0.0.0.0", "::", "::1", "192.0.2.1", "2001:db8::1"] {
        assert!(!ListenOn::Loopback.accepts_address(address));
        assert!(ListenOn::AllInterfaces.accepts_address(address));
    }
    assert!(ListenOn::Loopback.accepts_address("127.0.0.1"));
    assert!(!ListenOn::AllInterfaces.accepts_address("not-an-address"));
    let public = Spec {
        listen_on: ListenOn::AllInterfaces,
        ..legacy
    };
    assert_eq!(public.listener(), "*:22022");
    assert_eq!(
        serde_json::from_str::<Spec>(&serde_json::to_string(&public).unwrap()).unwrap(),
        public
    );
    let message = format!(
        "{:#}",
        forwarding_error(&public, russh::Error::RequestDenied)
    );
    assert!(
        message.contains("listener *:22022") && message.contains("GatewayPorts"),
        "{message}"
    );
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
        .is_ok()
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
    write_private(&invalid, private_content);
    let message = format!("{:#}", read_identity(&invalid).unwrap_err());
    assert!(message.contains(invalid.to_str().unwrap()), "{message}");
    assert!(message.contains("decode"), "{message}");
    assert!(!message.contains(private_content));
}

#[test]
fn tunnel_private_keys_reject_shared_permissions_before_decoding() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("identity");
    let encoded = key().to_openssh(keys::ssh_key::LineEnding::LF).unwrap();
    write_private(&path, encoded.as_bytes());
    for mode in [0o600, 0o400, 0o640, 0o644, 0o660, 0o666] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        if mode & 0o077 == 0 {
            assert!(read_identity(&path).is_ok());
        } else {
            let error = read_identity(&path).unwrap_err();
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::PermissionDenied
            );
            let message = format!("{error:#}");
            assert!(message.contains("chmod 600"), "{message}");
            assert!(!message.contains(encoded.as_str()));
        }
    }
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
    let error = result
        .expect("Identity open waited for a FIFO writer")
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::InvalidInput
    );
    let message = format!("{error:#}");
    assert!(message.contains("regular file"), "{message}");
}

#[test]
fn default_ssh_keys_and_explicit_overrides_authenticate_in_an_isolated_home() {
    const CHILD: &str = "RODE_TEST_TUNNEL_HOME";
    if let Some(home) = std::env::var_os(CHILD) {
        assert_eq!(std::env::var_os("HOME"), Some(home.clone()));
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(default_identity_fixture(Path::new(&home)));
        return;
    }
    // The worker must discover keys in its own HOME. Never mutate the test
    // runner's environment or inspect the developer's real SSH credentials.
    let directory = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tunnels::tests::default_ssh_keys_and_explicit_overrides_authenticate_in_an_isolated_home",
            "--nocapture",
        ])
        .env(CHILD, directory.path())
        .env("HOME", directory.path())
        .env("SSH_AUTH_SOCK", directory.path().join("unavailable-agent.sock"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn settled_tunnel(store: &Store, settings: Spec) -> Info {
    let id = crate::new_id().unwrap();
    store.start(id.clone(), settings).unwrap();
    until(|| {
        matches!(
            store.list()[0].status,
            Status::Connected | Status::Attention | Status::HostKeyRequired
        )
    })
    .await;
    let info = store.list().remove(0);
    store.stop(&id);
    if info.status == Status::Connected {
        // Stop closes the client synchronously; the relay releases its listener
        // when it processes that EOF. Wait for release before reusing the port.
        tokio::time::timeout(Duration::from_secs(3), async {
            while TcpListener::bind(("127.0.0.1", info.spec.listen_port))
                .await
                .is_err()
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
    info
}

async fn default_identity_fixture(home: &Path) {
    let ssh = home.join(".ssh");
    std::fs::create_dir(&ssh).unwrap();
    let write_key = |name: &str, identity: &PrivateKey| {
        write_private(
            ssh.join(name),
            identity
                .to_openssh(keys::ssh_key::LineEnding::LF)
                .unwrap()
                .as_bytes(),
        );
    };
    for (name, algorithm) in [
        ("id_rsa", Algorithm::Rsa { hash: None }),
        (
            "id_ecdsa",
            Algorithm::Ecdsa {
                curve: keys::EcdsaCurve::NistP256,
            },
        ),
        ("id_ed25519", Algorithm::Ed25519),
    ] {
        let identity = PrivateKey::random(&mut rand::rng(), algorithm).unwrap();
        write_key(name, &identity);
        let host_key = key();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_port = listener.local_addr().unwrap().port();
        let reserved = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_port = reserved.local_addr().unwrap().port();
        drop(reserved);
        let relay = Relay {
            allowed: identity.public_key().clone(),
            handles: Default::default(),
            auths: Default::default(),
            deny_forwarding: Default::default(),
        };
        let mut server = relay.clone();
        let config = Arc::new(server::Config {
            keys: vec![host_key.clone()],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        let server_task = tokio::spawn(async move {
            server.run_on_socket(config, &listener).await.unwrap();
        });
        let store = Store::default();
        let settings = Spec {
            relay_port,
            listen_port,
            relay_host: "127.0.0.1".into(),
            relay_user: "compute".into(),
            ..Default::default()
        };
        let info = settled_tunnel(&store, settings.clone()).await;
        assert_eq!(info.status, Status::HostKeyRequired, "{info:?}");
        assert_eq!(relay.auths.load(Ordering::SeqCst), 0);
        let settings = Spec {
            relay_key: host_key.public_key().to_openssh().unwrap(),
            ..settings
        };
        let info = settled_tunnel(&store, settings.clone()).await;
        assert_eq!(info.status, Status::Connected, "{name}: {info:?}");
        assert_eq!(relay.auths.load(Ordering::SeqCst), 1);
        if name == "id_ed25519" {
            // Bad and rejected defaults must not prevent the final key working.
            write_private(ssh.join("id_rsa"), "invalid-private-key-must-not-leak");
            write_key("id_ecdsa", &key());
            let info = settled_tunnel(&store, settings.clone()).await;
            assert_eq!(info.status, Status::Connected, "{info:?}");
            let encrypted = key()
                .encrypt(&mut rand::rng(), "fixture-passphrase")
                .unwrap();
            write_key("id_rsa", &encrypted);
            let info = settled_tunnel(&store, settings.clone()).await;
            assert_eq!(info.status, Status::Connected, "{info:?}");
            // Default order is meaningful: stop after the first accepted key.
            write_key("id_rsa", &identity);
            let before = relay.auths.load(Ordering::SeqCst);
            let info = settled_tunnel(&store, settings.clone()).await;
            assert_eq!(info.status, Status::Connected, "{info:?}");
            assert_eq!(relay.auths.load(Ordering::SeqCst), before + 1);
            // A selected key or agent must never silently fall back to defaults.
            for (identity, agent) in [
                ("~/.ssh/id_ecdsa".to_owned(), String::new()),
                (
                    home.join("missing-key").to_string_lossy().into_owned(),
                    String::new(),
                ),
                (
                    String::new(),
                    home.join("missing-agent.sock")
                        .to_string_lossy()
                        .into_owned(),
                ),
            ] {
                let info = settled_tunnel(
                    &store,
                    Spec {
                        identity,
                        agent_socket: agent,
                        ..settings.clone()
                    },
                )
                .await;
                assert_eq!(info.status, Status::Attention, "{info:?}");
            }
            write_private(ssh.join("id_rsa"), "invalid-private-key-must-not-leak");
            std::fs::remove_file(ssh.join("id_ed25519")).unwrap();
            let info = settled_tunnel(&store, settings.clone()).await;
            assert_eq!(info.status, Status::Attention, "{info:?}");
            assert!(
                info.error.contains("Cannot decode") && info.error.contains("rejected"),
                "{info:?}"
            );
            assert!(!info.error.contains("invalid-private-key-must-not-leak"));
            std::fs::remove_file(ssh.join("id_rsa")).unwrap();
            std::fs::remove_file(ssh.join("id_ecdsa")).unwrap();
        } else {
            std::fs::remove_file(ssh.join(name)).unwrap();
        }
        let before = relay.auths.load(Ordering::SeqCst);
        let info = settled_tunnel(&store, settings).await;
        assert_eq!(info.status, Status::Attention, "{info:?}");
        assert!(
            info.error.contains("Default SSH keys") && info.error.contains("Agent socket"),
            "{info:?}"
        );
        assert_eq!(relay.auths.load(Ordering::SeqCst), before);
        server_task.abort();
    }
}

#[tokio::test]
async fn verifies_relay_forwards_reconnects_and_stops_only_its_worker() {
    let directory = tempfile::tempdir().unwrap();
    let identity = key();
    let identity_path = directory.path().join("identity");
    write_private(
        &identity_path,
        identity
            .to_openssh(keys::ssh_key::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    );
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
    // Scope changes replace this worker; wildcard channels may identify the
    // requested wildcard or an actual IPv4/IPv6 interface. They still reach
    // only the fixed compute SSH port and reject unrequested listener ports.
    for listen_on in [ListenOn::AllInterfaces, ListenOn::Loopback] {
        settings.listen_on = listen_on;
        store.start(id.clone(), settings.clone()).unwrap();
        until(|| store.list()[0].status == Status::Connected).await;
        roundtrip(listen_port).await;
        let handle = relay.handles.lock().unwrap().last().unwrap().clone();
        assert!(
            handle
                .channel_open_forwarded_tcpip(
                    "127.0.0.1",
                    u32::from(listen_port) + 1,
                    "127.0.0.1",
                    1
                )
                .await
                .is_err()
        );
        if listen_on == ListenOn::AllInterfaces {
            for address in ["0.0.0.0", "::", "192.0.2.1", "2001:db8::1"] {
                let channel = handle
                    .channel_open_forwarded_tcpip(address, listen_port.into(), "127.0.0.1", 1)
                    .await
                    .unwrap();
                let mut stream = channel.into_stream();
                stream.write_all(b"scope").await.unwrap();
                let mut bytes = [0; 5];
                tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut bytes))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(&bytes, b"scope");
            }
        }
    }
    let handle = relay.handles.lock().unwrap().last().unwrap().clone();
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

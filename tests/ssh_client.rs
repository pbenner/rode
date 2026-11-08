//! Real login PTYs against a private Rust SSH server. No installed SSH tools required.
use rode::{
    ssh::{Backend, Profile, Remote},
    terminal::Terminal,
};
use russh::{
    Channel, ChannelId, MethodKind,
    keys::{Algorithm, EcdsaCurve, PrivateKey},
    server::{self, Server as _},
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
enum Method {
    Password,
    Interactive,
    Key,
    Gssapi,
}
#[derive(Clone)]
struct Peer {
    method: Method,
    calls: Arc<AtomicUsize>,
}
impl server::Server for Peer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}
impl server::Handler for Peer {
    type Error = russh::Error;
    async fn auth_password(
        &mut self,
        _: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(
            if matches!(self.method, Method::Password) && password == "private-password" {
                server::Auth::Accept
            } else {
                server::Auth::reject()
            },
        )
    }
    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _: &str,
        _: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<server::Auth, Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !matches!(self.method, Method::Interactive) {
            return Ok(server::Auth::reject());
        }
        Ok(match response {
            None => server::Auth::Partial {
                name: "Fixture MFA".into(),
                instructions: "Enter the challenge".into(),
                prompts: vec![("One-time code: ".into(), false)].into(),
            },
            Some(mut response) => {
                if response
                    .next()
                    .is_some_and(|bytes| bytes.as_ref() == b"654321")
                {
                    server::Auth::Accept
                } else {
                    server::Auth::reject()
                }
            }
        })
    }
    async fn auth_publickey(
        &mut self,
        _: &str,
        _: server::VerifiedPublicKey<'_>,
    ) -> Result<server::Auth, Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(if matches!(self.method, Method::Key) {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }
    async fn channel_open_session(
        &mut self,
        _: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, b"authenticated".to_vec())?;
        session.exit_status_request(channel, 0)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }
}
struct Fixture {
    root: tempfile::TempDir,
    port: u16,
    key: PrivateKey,
    calls: Arc<AtomicUsize>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(method: Method) -> Self {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        Self::with_host_keys(method, vec![key])
    }
    fn with_host_keys(method: Method, keys: Vec<PrivateKey>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let key = keys[0].clone();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let methods = &[match method {
            Method::Password => MethodKind::Password,
            Method::Interactive => MethodKind::KeyboardInteractive,
            Method::Key => MethodKind::PublicKey,
            Method::Gssapi => MethodKind::GssapiWithMic,
        }][..];
        let config = Arc::new(server::Config {
            keys,
            // Stored RSA keys must also work with modern RSA SHA-2 signatures.
            preferred: russh::Preferred {
                key: russh::Preferred::default()
                    .key
                    .iter()
                    .filter(|&algorithm| *algorithm != Algorithm::Rsa { hash: None })
                    .cloned()
                    .collect::<Vec<_>>()
                    .into(),
                ..Default::default()
            },
            methods: methods.into(),
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let mut peer = Peer {
            method,
            calls: calls.clone(),
        };
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener=tokio::net::TcpListener::from_std(listener).unwrap();
                let server=peer.run_on_socket(config,&listener);let control=server.handle();
                tokio::pin!(server);
                tokio::select!{_=&mut server=>{},_ = stopping=>{control.shutdown("fixture complete".into());let _=server.await;}}
            });
            runtime.shutdown_timeout(Duration::from_millis(100));
        });
        Self {
            root,
            port,
            key,
            calls,
            stop: Some(stop),
            thread: Some(thread),
        }
    }
    fn profile(&self, strict: &str) -> Profile {
        let config = self.root.path().join("config");
        std::fs::write(&config,format!("Host fixture\n HostName 127.0.0.1\n User fixture\n Port {}\n UserKnownHostsFile {}\n GlobalKnownHostsFile {}\n IdentityFile none\n IdentityAgent none\n StrictHostKeyChecking {strict}\n",self.port,self.root.path().join("known_hosts").display(),self.root.path().join("global_hosts").display())).unwrap();
        Profile {
            host: "fixture".into(),
            config: config.to_string_lossy().into(),
            x11_forwarding: false,
            ..Default::default()
        }
    }
    fn trust(&self, key: &PrivateKey) {
        std::fs::write(
            self.root.path().join("known_hosts"),
            format!(
                "[127.0.0.1]:{} {}\n",
                self.port,
                key.public_key().to_openssh().unwrap()
            ),
        )
        .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct Connection {
    remote: Remote,
    login: Option<Terminal>,
}
impl Connection {
    fn new(profile: Profile) -> Self {
        let remote = Remote::new(profile).unwrap();
        assert_eq!(remote.profile.backend, Backend::Russh);
        let login = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
        Self {
            remote,
            login: Some(login),
        }
    }
    fn text(&self) -> String {
        self.login.as_ref().unwrap().contents()
    }
    fn wait(&self, needle: &str) {
        let until = Instant::now() + Duration::from_secs(15);
        while !self.text().contains(needle) {
            assert!(
                Instant::now() < until && self.login.as_ref().unwrap().is_alive(),
                "Waiting for {needle}: {}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn answer(&self, text: &str) {
        self.login
            .as_ref()
            .unwrap()
            .send(format!("{text}\r").into_bytes())
            .unwrap();
    }
    fn ready(&self) {
        let until = Instant::now() + Duration::from_secs(15);
        while !self.remote.connected() {
            assert!(
                Instant::now() < until && self.login.as_ref().unwrap().is_alive(),
                "{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = self.remote.command().arg("fixture").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"authenticated");
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        drop(self.login.take());
        self.remote.cleanup();
    }
}

#[test]
fn rust_ssh_trust_confirmation_password_and_changed_host_refusal() {
    let fixture = Fixture::new(Method::Password);
    {
        let connection = Connection::new(fixture.profile("ask"));
        connection.wait("Type yes:");
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
        connection.answer("yes");
        connection.wait("password:");
        connection.answer("private-password");
        connection.ready();
        assert!(!connection.text().contains("private-password"));
        assert!(Path::new(&connection.remote.profile.config).is_file());
        assert!(
            !std::fs::read_to_string(&connection.remote.profile.config)
                .unwrap()
                .contains("private-password")
        );
    }
    let before = fixture.calls.load(Ordering::SeqCst);
    let replacement = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    fixture.trust(&replacement);
    let connection = Connection::new(fixture.profile("ask"));
    connection.wait("CHANGED");
    assert!(!connection.remote.connected());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), before);
}

#[test]
fn rust_ssh_prefers_known_host_key_types_on_multi_key_servers() {
    let ed25519 = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let ecdsa = PrivateKey::random(
        &mut rand::rng(),
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        },
    )
    .unwrap();
    let rsa = PrivateKey::random(&mut rand::rng(), Algorithm::Rsa { hash: None }).unwrap();
    let fixture =
        Fixture::with_host_keys(Method::Password, vec![ed25519, ecdsa.clone(), rsa.clone()]);
    for trusted in [ecdsa, rsa] {
        fixture.trust(&trusted);
        let before = std::fs::read(fixture.root.path().join("known_hosts")).unwrap();
        let connection = Connection::new(fixture.profile("yes"));
        connection.wait("password:");
        connection.answer("private-password");
        connection.ready();
        assert_eq!(
            std::fs::read(fixture.root.path().join("known_hosts")).unwrap(),
            before
        );
        assert!(!connection.text().contains("CHANGED"));
    }
}

#[test]
fn rust_ssh_rejects_changed_preferred_key_without_trusting_another_type() {
    let algorithm = Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP256,
    };
    let ed25519 = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let current = PrivateKey::random(&mut rand::rng(), algorithm.clone()).unwrap();
    let old = PrivateKey::random(&mut rand::rng(), algorithm).unwrap();
    let fixture = Fixture::with_host_keys(Method::Password, vec![ed25519, current]);
    fixture.trust(&old);
    let before = std::fs::read(fixture.root.path().join("known_hosts")).unwrap();
    let connection = Connection::new(fixture.profile("accept-new"));
    connection.wait("CHANGED");
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    assert!(!connection.remote.connected());
    assert_eq!(
        std::fs::read(fixture.root.path().join("known_hosts")).unwrap(),
        before
    );
}
#[test]
fn rust_ssh_keyboard_interactive_hides_mfa_response() {
    let fixture = Fixture::new(Method::Interactive);
    fixture.trust(&fixture.key);
    let connection = Connection::new(fixture.profile("yes"));
    connection.wait("One-time code:");
    connection.answer("654321");
    connection.ready();
    assert!(!connection.text().contains("654321"));
}

fn gssapi_profile(fixture: &Fixture, options: &str) -> Profile {
    let profile = fixture.profile("yes");
    let config = std::fs::read_to_string(&profile.config).unwrap();
    std::fs::write(
        &profile.config,
        format!("{config}\nGSSAPIAuthentication yes\nGSSAPIDelegateCredentials yes\n{options}\n"),
    )
    .unwrap();
    profile
}

#[test]
fn rust_ssh_gssapi_defaults_allow_supported_authentication() {
    for method in [Method::Password, Method::Interactive, Method::Key] {
        let fixture = Fixture::new(method);
        fixture.trust(&fixture.key);
        let mut profile = gssapi_profile(&fixture, "");
        if matches!(method, Method::Key) {
            let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
            let path = fixture.root.path().join("identity");
            key.write_openssh_file(&path, Default::default()).unwrap();
            profile.identity = path.to_string_lossy().into();
        }
        let connection = Connection::new(profile);
        match method {
            Method::Password => {
                connection.wait("password:");
                connection.answer("private-password");
            }
            Method::Interactive => {
                connection.wait("One-time code:");
                connection.answer("654321");
            }
            Method::Key => {}
            Method::Gssapi => unreachable!(),
        }
        connection.ready();
        // No warning is needed when the server did not offer GSSAPI.
        assert!(!connection.text().contains("GSSAPI/Kerberos"));
    }
}

#[test]
fn rust_ssh_gssapi_defaults_do_not_enable_disabled_authentication() {
    let fixture = Fixture::new(Method::Password);
    fixture.trust(&fixture.key);
    let profile = gssapi_profile(
        &fixture,
        "PasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication no",
    );
    let connection = Connection::new(profile);
    connection.wait("SSH authentication failed");
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    assert!(!connection.remote.connected());
}

#[test]
fn rust_ssh_gssapi_only_server_reports_authentication_configuration() {
    let fixture = Fixture::new(Method::Gssapi);
    fixture.trust(&fixture.key);
    // Keep this protocol-only fixture independent of the developer's tickets.
    // Actual enabled Kerberos logins use the private realm in ssh_kerberos.
    let connection = Connection::new(fixture.profile("yes"));
    connection.wait("check GSSAPIAuthentication");
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    assert!(!connection.remote.connected());
}
#[test]
fn rust_ssh_encrypted_key_uses_hidden_passphrase_prompt() {
    let fixture = Fixture::new(Method::Key);
    fixture.trust(&fixture.key);
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let encrypted = key.encrypt(&mut rand::rng(), "private-passphrase").unwrap();
    let key_path = fixture.root.path().join("encrypted");
    encrypted
        .write_openssh_file(&key_path, Default::default())
        .unwrap();
    let mut profile = fixture.profile("yes");
    profile.identity = key_path.to_string_lossy().into();
    let connection = Connection::new(profile);
    connection.wait("Passphrase for");
    connection.answer("private-passphrase");
    connection.ready();
    assert!(!connection.text().contains("private-passphrase"));
}
#[test]
fn rust_ssh_unknown_host_in_strict_mode_never_attempts_authentication() {
    let fixture = Fixture::new(Method::Password);
    let connection = Connection::new(fixture.profile("yes"));
    connection.wait("Unknown SSH host key");
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    assert!(!connection.remote.connected());
}

#[test]
fn rust_ssh_control_cannot_be_borrowed_by_a_local_sandbox() {
    let fixture = Fixture::new(Method::Password);
    fixture.trust(&fixture.key);
    let connection = Connection::new(fixture.profile("yes"));
    connection.wait("password:");
    connection.answer("private-password");
    connection.ready();
    for backend in [
        rode::sandbox::Backend::Landlock,
        rode::sandbox::Backend::Bwrap,
    ] {
        let project = tempfile::tempdir().unwrap();
        let executable = connection.remote.client_program();
        let plan = rode::sandbox::prepare_with_helper(
            project.path().to_str().unwrap(),
            &rode::sandbox::Options {
                enabled: true,
                backend,
                read_only: vec![executable.to_string_lossy().into_owned()],
                read_write: vec![
                    connection
                        .remote
                        .socket
                        .parent()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                ],
                ..Default::default()
            },
            Path::new(env!("CARGO_BIN_EXE_rode-sessiond")),
        )
        .unwrap();
        if !plan.active {
            eprintln!("Control-boundary sandbox unavailable: {}", plan.label());
            assert_ne!(
                std::env::var("RODE_TEST_REQUIRE_SANDBOX").as_deref(),
                Ok("1")
            );
            continue;
        }
        let prefix = plan.execution_prefix("").unwrap();
        let output = std::process::Command::new(&prefix[0])
            .args(&prefix[1..])
            .arg(&executable)
            .args(connection.remote.channel_args(false))
            .arg("fixture")
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "Sandbox reached SSH control: {output:?}"
        );
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(
            connection.remote.connected(),
            "Sandbox probe damaged the connection"
        );
    }
}

#[test]
fn rust_ssh_fast_exit_drains_reply_while_input_is_pending() {
    let fixture = Fixture::new(Method::Password);
    fixture.trust(&fixture.key);
    let connection = Connection::new(fixture.profile("yes"));
    connection.wait("password:");
    connection.answer("private-password");
    connection.ready();
    for _ in 0..20 {
        let mut command = connection.remote.command();
        command.arg("fixture");
        let output = rode::project::run(
            command,
            vec![b'x'; 2 * 1024 * 1024],
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(output.success(), "{output:?}");
        assert_eq!(output.stdout, "authenticated");
    }
}

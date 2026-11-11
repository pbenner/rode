//! Explicit, daemon-owned reverse SSH forwards to this host's existing sshd.
//!
//! Only the authenticated host administration channel may manage tunnels. Relay
//! listeners default to loopback; wider access requires an explicit selection.
//! Forwarded bytes always go to a fixed local SSH port.
//! The desktop still authenticates the compute host independently over that route.
//! Workers retain no desktop attachment and never execute an ssh client binary.
use anyhow::{Context, Result, bail, ensure};
use russh::{client, keys};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpStream,
    sync::{Semaphore, watch},
};

const MAX_TUNNELS: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(15);
// OpenSSH's default IdentityFile order, restricted to file-backed key types this
// unattended worker can sign with. Security keys and encrypted keys need an agent.
// https://man.openbsd.org/ssh_config.5#IdentityFile
const DEFAULT_IDENTITIES: [&str; 3] = ["~/.ssh/id_rsa", "~/.ssh/id_ecdsa", "~/.ssh/id_ed25519"];

/// Requested relay listener scope. The relay's SSH policy may further restrict it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenOn {
    /// Request a listener on the relay's IPv4 loopback interface only.
    #[default]
    Loopback,
    /// Request every address family/interface supported by the relay.
    AllInterfaces,
}
impl ListenOn {
    /// User-visible listener scope, without claiming the relay honoured the request.
    pub fn label(self) -> &'static str {
        match self {
            Self::Loopback => "Localhost only",
            Self::AllInterfaces => "All network interfaces",
        }
    }

    fn address(self) -> &'static str {
        match self {
            Self::Loopback => "127.0.0.1",
            // RFC 4254 §7.1 requests all supported address families with an
            // empty bind address; 0.0.0.0 would request IPv4 only.
            Self::AllInterfaces => "",
        }
    }

    fn accepts_address(self, address: &str) -> bool {
        match self {
            Self::Loopback => address == "127.0.0.1",
            // Servers may report the original wildcard or the actual IPv4/
            // IPv6 destination. The requested port is checked independently.
            Self::AllInterfaces => {
                address.is_empty() || address == "*" || address.parse::<std::net::IpAddr>().is_ok()
            }
        }
    }
}

/// Read this machine's hostname for display without starting a process or doing
/// DNS/filesystem work. An unavailable name must not be mistaken for a remote host.
pub fn local_hostname() -> Option<String> {
    let mut bytes = [0_u8; 256];
    // SAFETY: gethostname receives a writable buffer of the stated length. Read
    // only that buffer, even on platforms that omit the trailing NUL on truncation.
    if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return None;
    }
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let hostname = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
    (!hostname.is_empty() && !hostname.chars().any(char::is_control)).then_some(hostname)
}

/// Host-side tunnel settings. Paths and relay addresses are resolved on the compute host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spec {
    /// User-visible name, independent of the service identity.
    pub name: String,
    /// Literal relay hostname/IP (not an OpenSSH configuration alias).
    pub relay_host: String,
    /// Relay's SSH server port.
    pub relay_port: u16,
    /// SSH login on the relay.
    pub relay_user: String,
    /// Unencrypted private-key path on the compute host. Both credential fields
    /// blank try default SSH key files; an explicit agent alone stays agent-only.
    pub identity: String,
    /// Optional persistent SSH-agent socket on the compute host. Never inherited implicitly.
    pub agent_socket: String,
    /// Explicitly trusted relay public key in OpenSSH format. Blank requests trust confirmation.
    pub relay_key: String,
    /// Relay listener scope; loopback is the default for new configurations.
    pub listen_on: ListenOn,
    /// Stable listener port on the relay. Zero/dynamic ports are not accepted.
    pub listen_port: u16,
    /// This compute host's existing SSH server, always reached through 127.0.0.1.
    pub ssh_port: u16,
}
impl Default for Spec {
    fn default() -> Self {
        Self {
            name: "Compute tunnel".into(),
            relay_host: String::new(),
            relay_port: 22,
            relay_user: String::new(),
            identity: String::new(),
            agent_socket: String::new(),
            relay_key: String::new(),
            listen_on: ListenOn::default(),
            listen_port: 22022,
            ssh_port: 22,
        }
    }
}
impl Spec {
    /// Requested relay listener for diagnostics; `*` means all interfaces.
    pub fn listener(&self) -> String {
        let address = match self.listen_on {
            ListenOn::Loopback => "127.0.0.1",
            ListenOn::AllInterfaces => "*",
        };
        format!("{address}:{}", self.listen_port)
    }

    /// Bound all control data before allocating a worker or opening a connection.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.name.trim().is_empty()
                && self.name.len() <= 64
                && !self.name.chars().any(char::is_control),
            "Enter a tunnel name of 1–64 printable characters"
        );
        for (name, value) in [
            ("relay host", &self.relay_host),
            ("relay user", &self.relay_user),
        ] {
            ensure!(
                !value.is_empty()
                    && value.len() <= 253
                    && !value.starts_with('-')
                    && !value.chars().any(|c| c.is_control() || c.is_whitespace())
                    && !value.contains(['/', '@']),
                "Invalid {name}; enter a literal hostname and a separate username"
            );
        }
        ensure!(
            self.relay_port > 0 && self.listen_port > 0 && self.ssh_port > 0,
            "Tunnel ports must be between 1 and 65535"
        );
        for path in [&self.identity, &self.agent_socket] {
            ensure!(
                path.len() <= 4096
                    && !path.chars().any(char::is_control)
                    && (path.is_empty() || path.starts_with('/') || path.starts_with("~/")),
                "Identity and agent paths must be absolute or start with ~/"
            );
        }
        ensure!(
            self.relay_key.len() <= 16384,
            "Relay host key exceeds its limit"
        );
        if !self.relay_key.is_empty() {
            keys::PublicKey::from_openssh(&self.relay_key).context("Invalid relay host key")?;
        }
        Ok(())
    }

    fn automatic_identity(&self) -> bool {
        self.identity.is_empty() && self.agent_socket.is_empty()
    }

    fn identity_files(&self) -> Vec<PathBuf> {
        if !self.identity.is_empty() {
            return vec![rode_host_core::files::expand_home(&self.identity)];
        }
        if !self.agent_socket.is_empty() {
            return vec![];
        }
        DEFAULT_IDENTITIES
            .iter()
            .map(|path| rode_host_core::files::expand_home(path))
            .collect()
    }
}
/// Observable daemon state; attention states do not retry authentication or accept changed keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Establishing or authenticating the relay connection.
    Connecting,
    /// Listening on the relay and forwarding to the compute sshd.
    Connected,
    /// Network failure; waiting before another attempt.
    Reconnecting,
    /// A new or changed relay key requires explicit verification and restart.
    HostKeyRequired,
    /// Authentication/configuration needs attention on the compute host.
    Attention,
}
impl Status {
    /// Short label used by the tunnel manager.
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Reconnecting => "Reconnecting",
            Self::HostKeyRequired => "Verify relay host key",
            Self::Attention => "Needs attention",
        }
    }
}
/// Bounded tunnel metadata; never contains private keys, passwords or agent capabilities.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    /// Stable service-owned tunnel identity.
    pub id: String,
    /// Hostname of the service owning this tunnel.
    pub hostname: String,
    /// Captured host-side settings.
    pub spec: Spec,
    /// Current connection state.
    pub status: Status,
    /// Last connection failure or required action.
    pub error: String,
    /// Most recently presented relay key; trust requires a separate explicit Start request.
    pub observed_key: String,
    /// SHA-256 fingerprint of the presented relay key.
    pub fingerprint: String,
    /// Number of connection attempts, including the initial connection.
    pub attempts: u64,
}
struct Entry {
    info: Arc<Mutex<Info>>,
    cancel: watch::Sender<bool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Entry {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
/// Service-owned workers. An explicitly started tunnel keeps the daemon alive until stopped.
#[derive(Default)]
pub(crate) struct Store(Mutex<BTreeMap<String, Entry>>);
impl Store {
    pub fn live(&self) -> bool {
        !self.0.lock().unwrap().is_empty()
    }
    pub fn list(&self) -> Vec<Info> {
        self.0
            .lock()
            .unwrap()
            .values()
            .map(|e| e.info.lock().unwrap().clone())
            .collect()
    }
    pub fn stop(&self, id: &str) {
        self.0.lock().unwrap().remove(id);
    }
    pub fn start(&self, id: String, spec: Spec) -> Result<Info> {
        ensure!(crate::valid_id(&id), "Invalid tunnel identity");
        spec.validate()?;
        let mut entries = self.0.lock().unwrap();
        if let Some(entry) = entries.get(&id) {
            let info = entry.info.lock().unwrap();
            if info.spec == spec
                && matches!(
                    info.status,
                    Status::Connecting | Status::Connected | Status::Reconnecting
                )
            {
                return Ok(info.clone());
            }
        }
        ensure!(
            entries.contains_key(&id) || entries.len() < MAX_TUNNELS,
            "At most {MAX_TUNNELS} tunnels may be retained"
        );
        if let Some((name, listener)) = entries.iter().find_map(|(other_id, e)| {
            let other = &e.info.lock().unwrap().spec;
            (*other_id != id
                && (
                    other.relay_host.as_str(),
                    other.relay_port,
                    other.listen_port,
                ) == (spec.relay_host.as_str(), spec.relay_port, spec.listen_port))
                .then(|| (other.name.clone(), other.listener()))
        }) {
            bail!(
                "Tunnel {name:?} already reserves listener {listener} on relay {}. Manage that tunnel in Tunnels, or choose a different Relay port under Port forwarding.",
                relay_endpoint(&spec)
            );
        }
        // Cancel and join the previous worker before replacing its listener.
        entries.remove(&id);
        let info = Arc::new(Mutex::new(Info {
            id: id.clone(),
            hostname: local_hostname().unwrap_or_default(),
            spec,
            status: Status::Connecting,
            error: String::new(),
            observed_key: String::new(),
            fingerprint: String::new(),
            attempts: 0,
        }));
        let state = info.clone();
        let (cancel, mut cancelled) = watch::channel(false);
        let worker = std::thread::Builder::new()
            .name("rode-tunnel".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => {
                        runtime.block_on(async {
                            tokio::select! {
                                _ = cancelled.changed() => {},
                                _ = supervise(state) => {},
                            }
                        });
                        runtime.shutdown_timeout(Duration::from_millis(100));
                    }
                    Err(error) => attention(
                        &state,
                        &format!("Cannot create tunnel runtime on the tunnel host: {error}"),
                    ),
                }
            })
            .context("Cannot start the SSH tunnel worker on the tunnel host")?;
        let result = info.lock().unwrap().clone();
        entries.insert(
            id,
            Entry {
                info,
                cancel,
                worker: Some(worker),
            },
        );
        Ok(result)
    }
}
fn attention(info: &Mutex<Info>, message: &str) {
    let mut state = info.lock().unwrap();
    state.status = Status::Attention;
    state.error = message
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(2048)
        .collect();
}
async fn supervise(info: Arc<Mutex<Info>>) {
    let mut delay = 1;
    loop {
        {
            let mut state = info.lock().unwrap();
            state.status = Status::Connecting;
            state.attempts = state.attempts.saturating_add(1);
        }
        let result = connected(info.clone()).await;
        let mut blocked = false;
        {
            let mut state = info.lock().unwrap();
            if matches!(state.status, Status::HostKeyRequired | Status::Attention) {
                blocked = true;
            } else {
                if state.status == Status::Connected {
                    delay = 1;
                }
                state.status = Status::Reconnecting;
                state.error = result
                    .err()
                    .map(|e| format!("{e:#}"))
                    .unwrap_or_else(|| "Relay connection ended".into())
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\n')
                    .take(2048)
                    .collect();
            }
        }
        if blocked {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}
struct Handler {
    info: Arc<Mutex<Info>>,
    spec: Spec,
    slots: Arc<Semaphore>,
    disconnected: watch::Receiver<()>,
}
impl client::Handler for Handler {
    type Error = anyhow::Error;
    async fn check_server_key(&mut self, key: &keys::PublicKeyOrCertificate) -> Result<bool> {
        if key.certificate().is_some() {
            attention(
                &self.info,
                "Tunnel relay host certificates are not supported; use a pinned plain host key",
            );
            bail!("Unsupported relay host certificate");
        }
        let key = key.public_key();
        let mut info = self.info.lock().unwrap();
        info.observed_key = key.to_openssh()?;
        info.fingerprint = key.fingerprint(keys::HashAlg::Sha256).to_string();
        if self.spec.relay_key.is_empty()
            || keys::PublicKey::from_openssh(&self.spec.relay_key)?.key_data() != key.key_data()
        {
            info.status = Status::HostKeyRequired;
            info.error = if self.spec.relay_key.is_empty() {
                "Verify the relay fingerprint before trusting this key."
            } else {
                "Relay host key CHANGED. Verify the relay before replacing its trusted key."
            }
            .into();
            return Ok(false);
        }
        Ok(true)
    }
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        address: &str,
        port: u32,
        _origin: &str,
        _origin_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<()> {
        if !self.spec.listen_on.accepts_address(address) || port != u32::from(self.spec.listen_port)
        {
            return Ok(());
        }
        let Ok(slot) = self.slots.clone().try_acquire_owned() else {
            return Ok(());
        };
        let ssh_port = self.spec.ssh_port;
        let mut disconnected = self.disconnected.clone();
        tokio::spawn(async move {
            let _slot = slot;
            tokio::select! {
                _ = disconnected.changed() => {},
                _ = async {
                    let Ok(Ok(mut local)) =
                        tokio::time::timeout(TIMEOUT, TcpStream::connect(("127.0.0.1", ssh_port))).await
                    else {
                        return;
                    };
                    reply.accept().await;
                    let _ = tokio::io::copy_bidirectional(&mut channel.into_stream(), &mut local).await;
                } => {},
            }
        });
        Ok(())
    }
}
async fn connected(info: Arc<Mutex<Info>>) -> Result<()> {
    let spec = info.lock().unwrap().spec.clone();
    let relay = relay_endpoint(&spec);
    let mut preferred = russh::Preferred::default();
    if !spec.relay_key.is_empty() {
        let key = keys::PublicKey::from_openssh(&spec.relay_key)?;
        // Negotiate the trusted key type when the relay offers several host keys.
        let algorithm = key.algorithm();
        let mut algorithms = preferred.key.to_vec();
        algorithms.sort_by_key(|a| {
            a.as_str() != algorithm.as_str()
                && !(algorithm.as_str() == "ssh-rsa" && a.as_str().starts_with("rsa-sha2-"))
        });
        preferred.key = algorithms.into();
    }
    let config = Arc::new(client::Config {
        preferred,
        keepalive_interval: Some(Duration::from_secs(10)),
        keepalive_max: 3,
        channel_buffer_size: 8,
        window_size: 256 * 1024,
        maximum_packet_size: 32768,
        ..Default::default()
    });
    // Dropping this connection future closes the watch channel. Every forward
    // then releases its local socket, even if that peer never finishes its EOF.
    let (_connection, disconnected) = watch::channel(());
    let handler = Handler {
        info: info.clone(),
        spec: spec.clone(),
        slots: Arc::new(Semaphore::new(32)),
        disconnected,
    };
    let stream = tokio::time::timeout(
        TIMEOUT,
        TcpStream::connect((spec.relay_host.as_str(), spec.relay_port)),
    )
    .await
    .with_context(|| format!("TCP connection to relay {relay} timed out"))?
    .with_context(|| {
        format!(
            "Cannot connect to relay {relay} from the tunnel host; check Relay host and SSH port"
        )
    })?;
    let mut session =
        tokio::time::timeout(TIMEOUT, client::connect_stream(config, stream, handler))
            .await
            .with_context(|| format!("SSH handshake with relay {relay} timed out"))?
            .with_context(|| format!("SSH handshake with relay {relay} failed"))?;
    let authentication = tokio::time::timeout(TIMEOUT, authenticate(&mut session, &spec, &info))
        .await
        .with_context(|| {
            format!(
                "Authentication as {} on relay {relay} timed out",
                spec.relay_user
            )
        })?;
    authentication.with_context(|| {
        format!(
            "Cannot authenticate as {} on relay {relay}",
            spec.relay_user
        )
    })?;
    tokio::time::timeout(TIMEOUT, session.tcpip_forward(spec.listen_on.address(), spec.listen_port.into())).await
        .with_context(|| format!("SSH login to relay {relay} succeeded, but the request for tunnel listener {} timed out. The relay did not confirm whether it created the listener. RODE will reconnect and retry.", spec.listener()))?
        .map_err(|error| forwarding_error(&spec, error))?;
    {
        let mut state = info.lock().unwrap();
        state.status = Status::Connected;
        state.error.clear();
    }
    session
        .await
        .with_context(|| format!("Relay {relay} disconnected"))?;
    Ok(())
}

fn relay_endpoint(spec: &Spec) -> String {
    if spec.relay_host.contains(':') && !spec.relay_host.starts_with('[') {
        format!("[{}]:{}", spec.relay_host, spec.relay_port)
    } else {
        format!("{}:{}", spec.relay_host, spec.relay_port)
    }
}

fn forwarding_error(spec: &Spec, error: russh::Error) -> anyhow::Error {
    let relay = relay_endpoint(spec);
    let port = spec.listen_port;
    let listener = spec.listener();
    // SSH_MSG_REQUEST_FAILURE carries no reason. Do not claim a port conflict
    // without evidence, or mislabel a lost connection as a server refusal.
    let message = if matches!(error, russh::Error::RequestDenied) {
        format!(
            "SSH login as {} to relay {relay} succeeded, but the relay refused tunnel listener {listener}.\n\
             SSH did not report why. Possible causes: the port is already in use (including another tunnel), or forwarding is restricted on the relay.\n\
             On the relay, run: ss -ltnp 'sport = :{port}' (use sudo to see all owners).\n\
             An sshd-session listener usually belongs to an existing SSH tunnel. Stop that tunnel at its source, or choose a different Relay port under Port forwarding.\n\
             If the port is free, ask the relay administrator to check GatewayPorts (clientspecified for all interfaces), AllowTcpForwarding, DisableForwarding, PermitListen and authorized_keys restrictions.\n\
             RODE will retry automatically. Stop this tunnel before testing the same port with ssh -R.\n\
             SSH detail",
            spec.relay_user
        )
    } else {
        format!(
            "SSH login to relay {relay} succeeded, but the request for tunnel listener {listener} could not complete. RODE will reconnect and retry.\nSSH detail"
        )
    };
    anyhow::Error::new(error).context(message)
}

async fn authenticate(
    session: &mut client::Handle<Handler>,
    spec: &Spec,
    info: &Mutex<Info>,
) -> Result<()> {
    let hash = session
        .best_supported_rsa_hash()
        .await
        .context("Cannot negotiate SSH authentication algorithms")?
        .flatten();
    if !spec.agent_socket.is_empty() {
        let path = rode_host_core::files::expand_home(&spec.agent_socket);
        let mut agent = keys::agent::client::AgentClient::connect_uds(&path)
            .await
            .with_context(|| format!("Cannot connect to SSH agent socket {} on the tunnel host; check Agent socket and that its persistent agent is running", path.display()))
            .inspect_err(|e| attention(info, &format!("{e:#}")))?;
        if let Some(binding) = session.agent_session_binding() {
            let _ = agent.bind_session(binding, false).await;
        }
        let identities = agent.request_identities().await.with_context(|| {
            format!(
                "Cannot list keys from SSH agent socket {} on the tunnel host",
                path.display()
            )
        })?;
        ensure!(
            identities.len() <= 128,
            "Agent returned too many identities"
        );
        for identity in identities {
            let result = match identity {
                keys::agent::AgentIdentity::PublicKey { key, .. } => {
                    session
                        .authenticate_publickey_with(&spec.relay_user, key, hash, &mut agent)
                        .await?
                }
                keys::agent::AgentIdentity::Certificate { certificate, .. } => {
                    session
                        .authenticate_certificate_with(
                            &spec.relay_user,
                            certificate,
                            hash,
                            &mut agent,
                        )
                        .await?
                }
            };
            if result.success() {
                return Ok(());
            }
        }
    }
    let automatic = spec.automatic_identity();
    let mut failures = vec![];
    for path in spec.identity_files() {
        if !path.is_absolute() {
            let message = "Cannot resolve SSH identity files on the tunnel host: HOME must be an absolute directory, or set an absolute Identity file path";
            attention(info, message);
            bail!(message);
        }
        let source = path.clone();
        let loaded = tokio::task::spawn_blocking(move || read_identity(&source))
            .await
            .context("SSH identity reader failed on the tunnel host")?;
        let key = match loaded {
            Ok(key) => key,
            Err(error) if automatic => {
                // Missing defaults are normal. An unreadable/encrypted first key
                // must not hide a later usable key, or leak its private contents.
                if !error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                {
                    failures.push(format!("{error:#}"));
                }
                continue;
            }
            Err(error) => {
                attention(info, &format!("{error:#}"));
                return Err(error);
            }
        };
        let result = session
            .authenticate_publickey(
                &spec.relay_user,
                keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash),
            )
            .await?;
        if result.success() {
            return Ok(());
        }
        if automatic {
            failures.push(format!("{}: key rejected by the relay", path.display()));
        }
    }
    if automatic {
        let mut message = format!(
            "Default SSH keys could not authenticate to the relay from the tunnel host. Tried {}. Set Identity file to an unencrypted private key, or Agent socket to a persistent agent (required for encrypted or hardware-backed keys).",
            DEFAULT_IDENTITIES.join(", ")
        );
        for failure in failures {
            message.push('\n');
            message.push_str(&failure);
        }
        attention(info, &message);
        bail!(message);
    }
    attention(
        info,
        "Relay rejected the configured key/agent; unattended tunnels require public-key authentication",
    );
    bail!("Relay authentication rejected")
}

fn read_identity(path: &Path) -> Result<keys::PrivateKey> {
    // Reject FIFOs/devices after opening without waiting for their peer. A timed
    // out spawn_blocking task cannot otherwise cancel a blocking FIFO open.
    let file = rode_host_core::locking::open_private_key(path)
        .with_context(|| format!("Cannot open SSH identity file {} on the tunnel host; check Identity file, or clear it and select a persistent Agent socket", path.display()))?;
    let mut encoded = zeroize::Zeroizing::new(String::new());
    file.take(65537)
        .read_to_string(&mut encoded)
        .with_context(|| {
            format!(
                "Cannot read SSH identity file {} on the tunnel host",
                path.display()
            )
        })?;
    ensure!(
        encoded.len() <= 65536,
        "SSH identity file {} on the tunnel host exceeds 64 KiB",
        path.display()
    );
    keys::decode_secret_key(&encoded, None).with_context(|| format!(
        "Cannot decode SSH identity file {} on the tunnel host; encrypted keys require an explicitly configured persistent Agent socket", path.display()
    ))
}

#[cfg(test)]
mod tests;

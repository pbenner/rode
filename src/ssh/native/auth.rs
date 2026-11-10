//! Host-key policy and terminal authentication. Credentials never enter profiles or IPC.
use super::*;
use base64::Engine;
use hmac::{Hmac, Mac};
use russh::{
    MethodKind,
    client::{AuthResult, KeyboardInteractiveAuthResponse},
    keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey},
};
use std::sync::OnceLock;
use zeroize::Zeroizing;

pub(super) struct TerminalMode(Option<libc::termios>);
impl TerminalMode {
    fn change(raw: bool, echo: bool) -> Result<Self> {
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut original) } != 0 {
            return Ok(Self(None));
        }
        let mut mode = original;
        if raw {
            unsafe { libc::cfmakeraw(&mut mode) };
        } else if !echo {
            mode.c_lflag &= !libc::ECHO;
        }
        ensure!(
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &mode) } == 0,
            "Cannot configure SSH login terminal"
        );
        Ok(Self(Some(original)))
    }
    pub fn raw() -> Result<Self> {
        Self::change(true, false)
    }
}
impl Drop for TerminalMode {
    fn drop(&mut self) {
        if let Some(original) = self.0 {
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &original) };
        }
    }
}
pub(super) fn visible(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(8192)
        .collect()
}
async fn prompt(label: &str, echo: bool) -> Result<Zeroizing<String>> {
    let label = visible(label);
    tokio::task::spawn_blocking(move || {
        ensure!(
            unsafe { libc::isatty(0) } == 1,
            "SSH authentication requires the login terminal"
        );
        let _mode = TerminalMode::change(false, echo)?;
        eprint!("{label}");
        std::io::stderr().flush()?;
        let mut result = Zeroizing::new(String::new());
        let mut bytes = Zeroizing::new(Vec::new());
        for byte in std::io::stdin().lock().bytes().take(4097) {
            let byte = byte?;
            if byte == b'\n' || byte == b'\r' {
                break;
            }
            bytes.push(byte);
        }
        if !echo {
            eprintln!();
        }
        ensure!(bytes.len() <= 4096, "SSH response exceeds limit");
        result.push_str(std::str::from_utf8(&bytes)?);
        Ok(result)
    })
    .await?
}

fn host_matches(token: &str, patterns: &str) -> bool {
    let mut matched = false;
    for pattern in patterns.split(',') {
        if let Some(hash) = pattern.strip_prefix("|1|") {
            if let Some((salt, expected)) = hash.split_once('|')
                && let (Ok(salt), Ok(expected)) = (
                    base64::engine::general_purpose::STANDARD.decode(salt),
                    base64::engine::general_purpose::STANDARD.decode(expected),
                )
                && let Ok(mut mac) = Hmac::<sha1::Sha1>::new_from_slice(&salt)
            {
                mac.update(token.as_bytes());
                matched |= mac.verify_slice(&expected).is_ok();
            }
        } else if let Some(negative) = pattern.strip_prefix('!') {
            if config::wildcard(negative, token) {
                return false;
            }
        } else {
            matched |= config::wildcard(pattern, token);
        }
    }
    matched
}

fn for_host_keys(
    bytes: &[u8],
    token: &str,
    mut visit: impl FnMut(Option<&str>, PublicKey) -> Result<()>,
) -> Result<()> {
    for line in std::str::from_utf8(bytes)?.lines() {
        let mut words = line.split_whitespace();
        let Some(first) = words.next() else { continue };
        if first.starts_with('#') {
            continue;
        }
        let (marker, hosts) = if first.starts_with('@') {
            (
                Some(first),
                words.next().context("Invalid known_hosts marker")?,
            )
        } else {
            (None, first)
        };
        if !host_matches(token, hosts) {
            continue;
        }
        let algorithm = words.next().context("Invalid known_hosts algorithm")?;
        let data = words.next().context("Invalid known_hosts key")?;
        let recorded = PublicKey::from_openssh(&format!("{algorithm} {data}"))?;
        visit(marker, recorded)?;
    }
    Ok(())
}

fn check_bytes(bytes: &[u8], token: &str, key: &PublicKey) -> Result<(bool, bool)> {
    let (mut known, mut found) = (false, false);
    for_host_keys(bytes, token, |marker, recorded| {
        if marker == Some("@revoked") {
            ensure!(
                recorded.key_data() != key.key_data(),
                "SSH host key is revoked for {token}"
            );
        } else if marker == Some("@cert-authority") {
            bail!(
                "SSH host uses certificate-authority policy; select OpenSSH for this configuration"
            );
        } else {
            ensure!(marker.is_none(), "Unsupported known_hosts marker");
            found = true;
            known |= recorded.key_data() == key.key_data();
        }
        Ok(())
    })?;
    Ok((known, found))
}
fn read_hosts(path: &Path) -> Result<Vec<u8>> {
    Ok(crate::persistence::read(path, 8 * 1024 * 1024)?.unwrap_or_default())
}

fn preferred_host_keys(config: &config::Config) -> Result<russh::Preferred> {
    let mut preferred = russh::Preferred::default();
    let mut algorithms: Vec<_> = preferred
        .key
        .iter()
        .cloned()
        .map(|key| (key, false))
        .collect();
    for path in &config.known_hosts {
        for_host_keys(&read_hosts(path)?, &config.host_token(), |marker, key| {
            // Revocations and certificate authorities are not trusted plain keys.
            // The negotiated key still goes through the full verification below.
            if marker.is_none() {
                let recorded = key.algorithm();
                for (algorithm, known) in &mut algorithms {
                    *known |= *algorithm == recorded
                        || (matches!(algorithm, keys::Algorithm::Rsa { .. })
                            && matches!(recorded, keys::Algorithm::Rsa { .. }));
                }
            }
            Ok(())
        })?;
    }
    // Prefer key types already recorded for this host, preserving the default
    // order within each group. RSA SHA-2 signatures use the same stored RSA key.
    // Reorder only: never enable an algorithm absent from the client's defaults.
    algorithms.sort_by_key(|(_, known)| !known);
    preferred.key = algorithms
        .into_iter()
        .map(|(key, _)| key)
        .collect::<Vec<_>>()
        .into();
    Ok(preferred)
}
fn secret(path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>> {
    let file = match rode_host_core::locking::open_private_key(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1024 * 1024, "SSH private key exceeds 1 MiB");
    Ok(Some(bytes))
}
fn identity_matches(path: &Path, identity: &keys::agent::AgentIdentity) -> bool {
    // Agent-only identities commonly name the public file itself; ordinary
    // private-key identities use an adjacent .pub file without loading secrets.
    [
        path.to_owned(),
        PathBuf::from(format!("{}.pub", path.to_string_lossy())),
    ]
    .iter()
    .any(|path| {
        crate::persistence::read(path, 64 * 1024)
            .ok()
            .flatten()
            .and_then(|bytes| {
                std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(|text| PublicKey::from_openssh(text).ok())
            })
            .is_some_and(|key| key.key_data() == identity.public_key().key_data())
    })
}

fn check(config: &config::Config, key: &PublicKey) -> Result<bool> {
    let (mut known, mut found) = (false, false);
    for path in &config.known_hosts {
        let (matched, present) = check_bytes(&read_hosts(path)?, &config.host_token(), key)?;
        known |= matched;
        found |= present;
    }
    ensure!(
        !found || known,
        "SSH host key CHANGED for {}. Refusing connection; verify the host before updating known_hosts.",
        config.host_token()
    );
    Ok(known)
}
fn remember(config: &config::Config, key: &PublicKey) -> Result<()> {
    let path = config
        .known_hosts
        .first()
        .context("Missing user known_hosts file")?;
    crate::persistence::transaction(path, |existed| {
        if check(config, key)? {
            return Ok(());
        }
        let mut bytes = read_hosts(path)?;
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        let token = config.host_token();
        let token = if config.hash_hosts {
            let mut salt = [0; 20];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut salt)?;
            let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&salt)?;
            mac.update(token.as_bytes());
            format!(
                "|1|{}|{}",
                base64::engine::general_purpose::STANDARD.encode(salt),
                base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
            )
        } else {
            token
        };
        bytes.extend_from_slice(format!("{token} {}\n", key.to_openssh()?).as_bytes());
        crate::persistence::replace(path, &bytes, existed)
    })
}

pub(super) struct Handler {
    pub config: config::Config,
    pub x11: Option<forwarding::X11>,
    pub forwarding_slots: Arc<tokio::sync::Semaphore>,
    agent_binding: Arc<OnceLock<keys::agent::AgentSessionBinding>>,
}
impl client::Handler for Handler {
    type Error = anyhow::Error;
    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool> {
        ensure!(
            key.certificate().is_none(),
            "SSH host certificates require the OpenSSH backend"
        );
        let key = key.public_key();
        if check(&self.config, &key)? {
            return Ok(true);
        }
        ensure!(
            self.config.strict != "yes",
            "Unknown SSH host key for {} (StrictHostKeyChecking=yes)",
            self.config.host_token()
        );
        if !["accept-new", "no", "off"].contains(&self.config.strict.as_str()) {
            ensure!(
                !self.config.batch,
                "Unknown SSH host key in BatchMode for {}",
                self.config.host_token()
            );
            let answer = prompt(
                &format!(
                    "Unknown host {}.\n{} fingerprint {}\nTrust and save this host key? Type yes: ",
                    self.config.host_token(),
                    key.algorithm(),
                    key.fingerprint(HashAlg::Sha256)
                ),
                true,
            )
            .await?;
            ensure!(answer.as_str() == "yes", "SSH host key was not accepted");
        }
        remember(&self.config, &key)?;
        Ok(true)
    }
    async fn auth_banner(&mut self, banner: &str, _session: &mut client::Session) -> Result<()> {
        eprintln!("{}", visible(banner));
        Ok(())
    }
    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<()> {
        if !self.config.forward_agent {
            return Ok(());
        }
        let Some(path) = self.config.agent.clone() else {
            return Ok(());
        };
        let Some(binding) = self.agent_binding.get().cloned() else {
            return Ok(());
        };
        let Ok(slot) = self.forwarding_slots.clone().try_acquire_owned() else {
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            let _slot = slot;
            let connected = tokio::time::timeout(IO_TIMEOUT, async {
                let mut agent = keys::agent::client::AgentClient::connect_uds(path).await?;
                // Mark this socket as forwarded before exposing it to the server.
                // Legacy agents reject this extension; like OpenSSH, retain their
                // unrestricted-key support after consuming that failure response.
                match agent.bind_session(&binding, true).await {
                    Ok(()) | Err(keys::Error::AgentFailure) => {}
                    Err(error) => return Err(error),
                }
                Ok(agent.into_inner())
            })
            .await;
            if let Ok(Ok(mut socket)) = connected {
                let mut stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
            }
        });
        Ok(())
    }
    async fn server_channel_open_x11(
        &mut self,
        channel: russh::Channel<client::Msg>,
        _address: &str,
        _port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<()> {
        let Some(x11) = self.x11.clone() else {
            return Ok(());
        };
        let Ok(slot) = self.forwarding_slots.clone().try_acquire_owned() else {
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            let _slot = slot;
            if let Err(error) = x11.forward(channel.into_stream()).await {
                eprintln!("X11 forwarding failed: {error:#}");
            }
        });
        Ok(())
    }
}

async fn authenticate(session: &mut Session, config: &config::Config) -> Result<()> {
    let mut result = session.authenticate_none(&config.user).await?;
    if result.success() {
        return Ok(());
    }
    if config.gssapi && allowed(&result, MethodKind::GssapiWithMic) {
        match gssapi::authenticate(session, config).await {
            Ok(next) => {
                result = next;
                if result.success() {
                    eprintln!("Authenticated using GSSAPI/Kerberos.");
                    return Ok(());
                }
                eprintln!(
                    "Kerberos authentication did not complete login; trying other enabled methods."
                );
            }
            Err(error) => eprintln!(
                "GSSAPI/Kerberos: {}; trying other enabled authentication methods.",
                visible(&format!("{error:#}"))
            ),
        }
    }
    if config.publickey && allowed(&result, MethodKind::PublicKey) {
        // A previous method (for example Kerberos) may already have supplied
        // one factor. Only a successful key factor should stop key attempts.
        let mut key_satisfied = false;
        let hash = session.best_supported_rsa_hash().await?.flatten();
        if let Some(path) = &config.agent
            && let Ok(mut agent) = keys::agent::client::AgentClient::connect_uds(path).await
        {
            if let Some(binding) = session.agent_session_binding() {
                let _ = agent.bind_session(binding, false).await;
            }
            if let Ok(identities) = agent.request_identities().await {
                ensure!(
                    identities.len() <= 128,
                    "SSH agent returned too many identities"
                );
                for identity in identities {
                    if config.identities_only
                        && !config
                            .keys
                            .iter()
                            .any(|path| identity_matches(path, &identity))
                    {
                        continue;
                    }
                    let attempt = match identity {
                        keys::agent::AgentIdentity::PublicKey { key, .. } => {
                            session
                                .authenticate_publickey_with(&config.user, key, hash, &mut agent)
                                .await
                        }
                        keys::agent::AgentIdentity::Certificate { certificate, .. } => {
                            session
                                .authenticate_certificate_with(
                                    &config.user,
                                    certificate,
                                    hash,
                                    &mut agent,
                                )
                                .await
                        }
                    };
                    if let Ok(next) = attempt {
                        result = next;
                        if result.success() {
                            return Ok(());
                        }
                        key_satisfied = matches!(
                            &result,
                            AuthResult::Failure {
                                partial_success: true,
                                ..
                            }
                        );
                    }
                    if key_satisfied {
                        break;
                    }
                }
            }
        }
        if !key_satisfied {
            for path in &config.keys {
                let Some(bytes) = secret(path)? else {
                    continue;
                };
                let encoded = std::str::from_utf8(&bytes)?;
                let key = match keys::decode_secret_key(encoded, None) {
                    Ok(key) => key,
                    Err(_) if !config.batch => {
                        let password =
                            prompt(&format!("Passphrase for {}: ", path.display()), false).await?;
                        keys::decode_secret_key(encoded, Some(password.as_str()))
                            .context("Cannot decrypt SSH identity")?
                    }
                    Err(error) => {
                        eprintln!("Cannot load SSH identity {}: {error}", path.display());
                        continue;
                    }
                };
                let cert_path = PathBuf::from(format!("{}-cert.pub", path.to_string_lossy()));
                result = if cert_path.is_file() {
                    let certificate = keys::Certificate::from_openssh(std::str::from_utf8(
                        &crate::persistence::read(&cert_path, 64 * 1024)?
                            .context("Missing SSH certificate")?,
                    )?)?;
                    session
                        .authenticate_openssh_cert(&config.user, Arc::new(key), certificate)
                        .await?
                } else {
                    session
                        .authenticate_publickey(
                            &config.user,
                            PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                        )
                        .await?
                };
                if result.success() {
                    return Ok(());
                }
                if matches!(
                    &result,
                    AuthResult::Failure {
                        partial_success: true,
                        ..
                    }
                ) {
                    break;
                }
            }
        }
    }
    let (mut challenges, mut passwords) = (0, 0);
    for _ in 0..6 {
        if config.batch {
            break;
        }
        if config.interactive && allowed(&result, MethodKind::KeyboardInteractive) && challenges < 3
        {
            challenges += 1;
            result = keyboard_auth(session, config).await?;
        } else if config.password && allowed(&result, MethodKind::Password) && passwords < 3 {
            passwords += 1;
            let password = prompt(
                &format!("{}@{} password: ", config.user, config.host),
                false,
            )
            .await?;
            result = session
                .authenticate_password(&config.user, password.as_str())
                .await?;
        } else {
            break;
        }
        if result.success() {
            return Ok(());
        }
    }
    if allowed(&result, MethodKind::GssapiWithMic) {
        bail!(
            "SSH authentication failed for {}@{}; check GSSAPIAuthentication, the system Kerberos library, ticket cache and host service principal, or select OpenSSH.",
            config.user,
            config.host
        );
    }
    bail!(
        "SSH authentication failed for {}@{}; check identity, agent and authentication settings",
        config.user,
        config.host
    )
}
async fn keyboard_auth(session: &mut Session, config: &config::Config) -> Result<AuthResult> {
    let mut response = session
        .authenticate_keyboard_interactive_start(&config.user, None::<String>)
        .await?;
    for _ in 0..16 {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(AuthResult::Success),
            KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                return Ok(AuthResult::Failure {
                    remaining_methods,
                    partial_success,
                });
            }
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                ensure!(prompts.len() <= 16, "Too many SSH authentication prompts");
                eprintln!("{}\n{}", visible(&name), visible(&instructions));
                let mut responses = vec![];
                for item in prompts {
                    responses.push(prompt(&item.prompt, item.echo).await?);
                }
                response = session
                    .authenticate_keyboard_interactive_respond(
                        responses.iter().map(|s| s.to_string()).collect(),
                    )
                    .await?;
            }
        }
    }
    bail!("SSH keyboard-interactive challenge limit exceeded")
}
fn allowed(result: &AuthResult, method: MethodKind) -> bool {
    matches!(result,AuthResult::Failure {remaining_methods,..} if remaining_methods.contains(&method))
}

pub(super) async fn connect(
    profile: &super::super::Profile,
    target: config::Config,
    x11: Option<forwarding::X11>,
) -> Result<(Session, Vec<Session>)> {
    fn append(
        profile: &super::super::Profile,
        config: config::Config,
        chain: &mut Vec<config::Config>,
        depth: usize,
    ) -> Result<()> {
        ensure!(
            depth <= 8 && chain.len() < 8,
            "SSH ProxyJump chain exceeds eight hosts or contains a cycle"
        );
        if !config.jump.is_empty() && config.jump != "none" {
            for jump in config.jump.split(',') {
                append(
                    profile,
                    config::Config::resolve(profile, jump, false)?,
                    chain,
                    depth + 1,
                )?;
            }
        }
        chain.push(config);
        Ok(())
    }
    let mut chain = vec![];
    append(profile, target, &mut chain, 0)?;
    let count = chain.len();
    let mut sessions: Vec<Session> = vec![];
    for (index, mut config) in chain.into_iter().enumerate() {
        let route = profile.tunnel.as_ref().filter(|_| index + 1 == count);
        if let Some(route) = route {
            config.host_key_alias = route.host_key_alias.clone();
        }
        let (address, port) = route.map_or((config.host.as_str(), config.port), |r| {
            ("127.0.0.1", r.port)
        });
        eprintln!(
            "Connecting to {}:{}{}…",
            config.host,
            config.port,
            if index + 1 < count {
                " (jump host)"
            } else {
                ""
            }
        );
        let options = Arc::new(client::Config {
            preferred: preferred_host_keys(&config)?,
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            channel_buffer_size: 16,
            window_size: 1024 * 1024,
            maximum_packet_size: 32768,
            ..Default::default()
        });
        let agent_binding = Arc::new(OnceLock::new());
        let handler = Handler {
            config: config.clone(),
            x11: if index + 1 == count {
                x11.clone()
            } else {
                None
            },
            forwarding_slots: Arc::new(tokio::sync::Semaphore::new(32)),
            agent_binding: agent_binding.clone(),
        };
        let mut session = if let Some(previous) = sessions.last() {
            let channel = tokio::time::timeout(
                IO_TIMEOUT,
                previous.channel_open_direct_tcpip(address, port.into(), "127.0.0.1", 0),
            )
            .await??;
            tokio::time::timeout(
                Duration::from_secs(120),
                client::connect_stream(options, channel.into_stream(), handler),
            )
            .await??
        } else {
            let stream =
                tokio::time::timeout(IO_TIMEOUT, tokio::net::TcpStream::connect((address, port)))
                    .await
                    .context("SSH connection timed out")?
                    .with_context(|| {
                        format!("SSH connect to {} port {}", config.host, config.port)
                    })?;
            tokio::time::timeout(
                Duration::from_secs(120),
                client::connect_stream(options, stream, handler),
            )
            .await??
        };
        if let Some(binding) = session.agent_session_binding() {
            let _ = agent_binding.set(binding.clone());
        }
        tokio::time::timeout(
            Duration::from_secs(180),
            authenticate(&mut session, &config),
        )
        .await??;
        sessions.push(session);
    }
    let target = sessions.pop().context("No SSH destination")?;
    Ok((target, sessions))
}

#[cfg(test)]
mod tests {
    use super::*;
    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const OTHER: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G2sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";
    #[test]
    fn host_keys_match_hashes_reject_revocation_and_preserve_other_hosts() {
        let key = PublicKey::from_openssh(KEY).unwrap();
        let bytes = format!("# retained\nexample {KEY}\n[localhost]:2222 {OTHER}\n");
        assert_eq!(
            check_bytes(bytes.as_bytes(), "example", &key).unwrap(),
            (true, true)
        );
        assert_eq!(
            check_bytes(bytes.as_bytes(), "[localhost]:2222", &key).unwrap(),
            (false, true)
        );
        assert!(
            check_bytes(
                format!("@revoked example {KEY}").as_bytes(),
                "example",
                &key
            )
            .is_err()
        );
        assert!(
            check_bytes(
                format!("@cert-authority example {OTHER}").as_bytes(),
                "example",
                &key
            )
            .is_err()
        );
        let salt = [42; 20];
        let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&salt).unwrap();
        mac.update(b"example");
        let hashed = format!(
            "|1|{}|{}",
            base64::engine::general_purpose::STANDARD.encode(salt),
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
        );
        assert!(host_matches("example", &hashed));
        assert!(!host_matches("other", &hashed));
        assert!(!host_matches("example", "*,!example"));
    }
    #[test]
    fn remembering_hosts_merges_and_never_overwrites_a_changed_key() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("known_hosts");
        let config_file = root.path().join("config");
        std::fs::write(
            &config_file,
            format!(
                "UserKnownHostsFile {}\nGlobalKnownHostsFile {}\n",
                path.display(),
                root.path().join("global").display()
            ),
        )
        .unwrap();
        let profile = super::super::super::Profile {
            host: "example".into(),
            config: config_file.to_string_lossy().into(),
            ..Default::default()
        };
        let config = config::Config::resolve(&profile, "example", true).unwrap();
        std::fs::write(&path, format!("# keep\nother {OTHER}\n")).unwrap();
        let key = PublicKey::from_openssh(KEY).unwrap();
        remember(&config, &key).unwrap();
        let saved = std::fs::read(&path).unwrap();
        assert!(String::from_utf8_lossy(&saved).starts_with("# keep\nother "));
        assert!(check(&config, &key).unwrap());
        remember(&config, &key).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        assert!(remember(&config, &PublicKey::from_openssh(OTHER).unwrap()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
    }

    #[test]
    fn host_key_preferences_respect_aliases_hashes_global_files_and_markers() {
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("user");
        let global = root.path().join("global");
        let config_file = root.path().join("config");
        std::fs::write(&config_file, format!(
            "Port 2222\nHostKeyAlias trusted-alias\nUserKnownHostsFile {}\nGlobalKnownHostsFile {}\n",
            user.display(), global.display()
        )).unwrap();
        let profile = super::super::super::Profile {
            host: "example".into(),
            config: config_file.to_string_lossy().into(),
            ..Default::default()
        };
        let config = config::Config::resolve(&profile, "example", true).unwrap();
        let algorithm = keys::Algorithm::Ecdsa {
            curve: keys::EcdsaCurve::NistP256,
        };
        let key = keys::PrivateKey::random(&mut rand::rng(), algorithm.clone()).unwrap();
        let public = key.public_key().to_openssh().unwrap();
        let defaults = russh::Preferred::default().key;
        for prefix in [
            "other",
            "*,!trusted-alias",
            "@revoked trusted-alias",
            "@cert-authority trusted-alias",
        ] {
            std::fs::write(&user, format!("{prefix} {public}\n")).unwrap();
            assert_eq!(
                preferred_host_keys(&config).unwrap().key,
                defaults,
                "{prefix}"
            );
        }
        std::fs::write(&user, format!("trusted-alias {public}\n")).unwrap();
        let plain = preferred_host_keys(&config).unwrap().key;
        assert_eq!(plain[0], algorithm);
        let mut expected = vec![algorithm.clone()];
        expected.extend(defaults.iter().filter(|&key| *key != algorithm).cloned());
        assert_eq!(plain.as_ref(), expected);
        let salt = [42; 20];
        let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&salt).unwrap();
        mac.update(b"trusted-alias");
        let token = format!(
            "|1|{}|{}",
            base64::engine::general_purpose::STANDARD.encode(salt),
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
        );
        std::fs::write(&user, "# no matching user entry\n").unwrap();
        std::fs::write(&global, format!("{token} {public}\n")).unwrap();
        assert_eq!(preferred_host_keys(&config).unwrap().key, plain);
    }
}

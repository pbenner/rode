//! Bounded, non-executing OpenSSH configuration resolution for the native client.
use super::*;
use std::collections::BTreeMap;

/// Resolved native SSH endpoint, trust/authentication policy and forwarding settings.
/// Configuration is captured before connecting and remains distinct from UI profile spelling.
#[derive(Clone, Debug)]
pub(super) struct Config {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub keys: Vec<PathBuf>,
    pub known_hosts: Vec<PathBuf>,
    pub host_key_alias: String,
    pub strict: String,
    pub batch: bool,
    pub identities_only: bool,
    pub agent: Option<PathBuf>,
    pub forward_agent: bool,
    pub jump: String,
    pub environment: Vec<(String, String)>,
    pub password: bool,
    pub interactive: bool,
    pub publickey: bool,
    pub gssapi: bool,
    pub gssapi_delegate: bool,
    pub hash_hosts: bool,
    pub xauth: String,
    pub x11_trusted: bool,
    pub x11_timeout: u64,
}

/// SSH patterns have only '*' and '?'; brackets in IPv6/known-host tokens are literal.
pub(super) fn wildcard(pattern: &str, text: &str) -> bool {
    match_pattern(pattern, text, true)
}

fn match_pattern(pattern: &str, text: &str, ignore_case: bool) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut i, mut j, mut star, mut retry) = (0, 0, None, 0);
    while j < t.len() {
        if i < p.len()
            && (p[i] == b'?' || p[i] == t[j] || (ignore_case && p[i].eq_ignore_ascii_case(&t[j])))
        {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            retry = j;
        } else if let Some(s) = star {
            i = s + 1;
            retry += 1;
            j = retry;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}

fn patterns(patterns: &[String], host: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        if let Some(negative) = pattern.strip_prefix('!') {
            if wildcard(negative, host) {
                return false;
            }
        } else {
            matched |= wildcard(pattern, host);
        }
    }
    matched
}

#[derive(Default)]
struct Parser {
    values: BTreeMap<String, Vec<String>>,
    bytes: usize,
    files: usize,
}
impl Parser {
    fn file(
        &mut self,
        path: &Path,
        host: &str,
        active: &mut bool,
        depth: usize,
        required: bool,
        system: bool,
    ) -> Result<()> {
        ensure!(
            depth <= 8 && self.files < 64,
            "SSH config Include limit exceeded"
        );
        let bytes = match crate::persistence::open_regular(path, false) {
            Ok(file) => {
                let mut bytes = vec![];
                std::io::Read::take(file, 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                bytes
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => return Ok(()),
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Cannot read SSH config {}", path.display()));
            }
        };
        self.bytes += bytes.len();
        self.files += 1;
        ensure!(self.bytes <= 1024 * 1024, "SSH configuration exceeds 1 MiB");
        for (number, line) in std::str::from_utf8(&bytes)?.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let end = line
                .find(|c: char| c.is_whitespace() || c == '=')
                .unwrap_or(line.len());
            let key = line[..end].to_ascii_lowercase();
            let rest = line[end..]
                .trim_start()
                .strip_prefix('=')
                .unwrap_or(line[end..].trim_start())
                .trim();
            let values = shell_words::split(rest).with_context(|| {
                format!("Invalid SSH config at {}:{}", path.display(), number + 1)
            })?;
            ensure!(!values.is_empty(), "Missing SSH config value for {key}");
            if key == "host" {
                *active = patterns(&values, host);
                continue;
            }
            if key == "match" {
                bail!(
                    "Rust SSH does not support Match blocks; select OpenSSH for this configuration"
                );
            }
            if !*active {
                continue;
            }
            if key == "include" {
                for value in values {
                    let expanded = crate::files::expand_home(&value);
                    let expanded = if expanded.is_absolute() {
                        expanded
                    } else if system {
                        Path::new("/etc/ssh").join(expanded)
                    } else {
                        crate::files::expand_home("~/.ssh").join(expanded)
                    };
                    for entry in glob::glob(&expanded.to_string_lossy())?.take(65) {
                        self.file(&entry?, host, active, depth + 1, true, system)?;
                    }
                }
                continue;
            }
            match key.as_str() {
                "hostname"
                | "user"
                | "port"
                | "proxyjump"
                | "identityagent"
                | "identitiesonly"
                | "userknownhostsfile"
                | "globalknownhostsfile"
                | "stricthostkeychecking"
                | "batchmode"
                | "forwardagent"
                | "passwordauthentication"
                | "kbdinteractiveauthentication"
                | "challengeresponseauthentication"
                | "pubkeyauthentication"
                | "gssapiauthentication"
                | "gssapidelegatecredentials"
                | "hostkeyalias"
                | "hashknownhosts"
                | "xauthlocation"
                | "forwardx11trusted"
                | "forwardx11timeout"
                | "setenv" => {
                    self.values.entry(key).or_insert(values);
                }
                "identityfile" => {
                    self.values.entry(key).or_default().extend(values);
                }
                "sendenv" => {
                    let patterns = self.values.entry(key).or_default();
                    for value in values {
                        ensure!(!value.contains('='), "Invalid SSH SendEnv pattern");
                        if let Some(remove) = value.strip_prefix('-') {
                            patterns.retain(|pattern| !match_pattern(remove, pattern, false));
                        } else {
                            patterns.push(value);
                        }
                    }
                }
                // These control the external client's presentation/process management,
                // or optional optimisations. RODE owns those lifetimes itself.
                "controlmaster"
                | "controlpath"
                | "controlpersist"
                | "loglevel"
                | "tcpkeepalive"
                | "serveraliveinterval"
                | "serveralivecountmax"
                | "connecttimeout"
                | "connectionattempts"
                | "compression"
                | "addkeystoagent"
                | "forwardx11"
                | "requesttty"
                | "exitonforwardfailure" => {}
                "canonicalizehostname" | "verifyhostkeydns" | "checkhostip" | "updatehostkeys" => {
                    ensure!(
                        values == ["no"],
                        "Rust SSH does not implement {key}; select OpenSSH for this configuration"
                    );
                }
                _ => bail!(
                    "Rust SSH does not implement {key}; select OpenSSH for this configuration"
                ),
            }
        }
        Ok(())
    }
}

fn endpoint(value: &str) -> Result<(Option<String>, String, Option<u16>)> {
    let (user, host) = value
        .rsplit_once('@')
        .map_or((None, value), |(u, h)| (Some(u.to_owned()), h));
    let (host, port) = if let Some(inner) = host.strip_prefix('[') {
        let (host, tail) = inner
            .split_once(']')
            .context("Invalid bracketed SSH host")?;
        (
            host,
            if tail.is_empty() {
                None
            } else {
                Some(
                    tail.strip_prefix(':')
                        .context("Invalid SSH port")?
                        .parse::<u16>()?,
                )
            },
        )
    } else if host.matches(':').count() == 1 {
        let (host, port) = host.split_once(':').context("Invalid SSH host")?;
        (host, Some(port.parse::<u16>()?))
    } else {
        (host, None)
    };
    ensure!(!host.is_empty() && port != Some(0), "Invalid SSH endpoint");
    Ok((user, host.to_owned(), port))
}

impl Config {
    pub fn resolve(
        profile: &super::super::Profile,
        destination: &str,
        target: bool,
    ) -> Result<Self> {
        let (explicit_user, alias, explicit_port) = endpoint(destination)?;
        let mut parser = Parser::default();
        if profile.config.is_empty() {
            parser.file(
                &crate::files::expand_home("~/.ssh/config"),
                &alias,
                &mut true,
                0,
                false,
                false,
            )?;
            parser.file(
                Path::new("/etc/ssh/ssh_config"),
                &alias,
                &mut true,
                0,
                false,
                true,
            )?;
        } else {
            parser.file(
                &crate::files::expand_home(&profile.config),
                &alias,
                &mut true,
                0,
                true,
                false,
            )?;
        }
        let get = |key: &str, default: &str| {
            parser
                .values
                .get(key)
                .and_then(|v| v.first())
                .cloned()
                .unwrap_or_else(|| default.into())
        };
        let yes = |key: &str, default: bool| -> Result<bool> {
            let value = get(key, if default { "yes" } else { "no" });
            match value.as_str() {
                "yes" => Ok(true),
                "no" => Ok(false),
                _ => bail!("Unsupported {key} value {value}; select OpenSSH"),
            }
        };
        let local_user =
            std::env::var("USER").context("Set an SSH username (USER is unavailable)")?;
        let user = if target && !profile.user.is_empty() {
            profile.user.clone()
        } else {
            explicit_user.unwrap_or_else(|| get("user", &local_user))
        };
        let host = get("hostname", &alias).replace("%h", &alias);
        let port = if target && !profile.port.is_empty() {
            profile.port.parse()?
        } else {
            explicit_port.unwrap_or(get("port", "22").parse()?)
        };
        ensure!(port > 0, "Invalid SSH port");
        let expand = |value: &str| -> Result<PathBuf> {
            let mut result = String::new();
            let mut chars = value.chars();
            while let Some(c) = chars.next() {
                if c != '%' {
                    result.push(c);
                    continue;
                }
                result.push_str(&match chars.next() {
                    Some('%') => "%".into(),
                    Some('d') => crate::files::expand_home("~")
                        .to_string_lossy()
                        .into_owned(),
                    Some('h') => host.clone(),
                    Some('n') => alias.clone(),
                    Some('r') => user.clone(),
                    Some('u') => local_user.clone(),
                    Some('p') => port.to_string(),
                    _ => bail!("Unsupported SSH path token; select OpenSSH"),
                });
            }
            Ok(crate::files::expand_home(&result))
        };
        let mut keys = if target && !profile.identity.is_empty() {
            vec![expand(&profile.identity)?]
        } else {
            vec![]
        };
        for value in parser.values.get("identityfile").into_iter().flatten() {
            if value != "none" {
                keys.push(expand(value)?);
            }
        }
        if keys.is_empty() && !parser.values.contains_key("identityfile") {
            for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
                keys.push(expand(&format!("~/.ssh/{name}"))?);
            }
        }
        ensure!(keys.len() <= 32, "Too many SSH identities");
        let mut known_hosts = vec![];
        for (key, default) in [
            ("userknownhostsfile", vec!["~/.ssh/known_hosts".into()]),
            (
                "globalknownhostsfile",
                vec!["/etc/ssh/ssh_known_hosts".into()],
            ),
        ] {
            for value in parser.values.get(key).unwrap_or(&default) {
                ensure!(
                    value != "none" && value != "/dev/null",
                    "Rust SSH requires known-host verification; select a known_hosts file"
                );
                known_hosts.push(expand(value)?);
            }
        }
        ensure!(known_hosts.len() <= 16, "Too many known-hosts files");
        let agent = match get("identityagent", "SSH_AUTH_SOCK").as_str() {
            "none" => None,
            "SSH_AUTH_SOCK" => std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from),
            path => Some(expand(path)?),
        };
        let strict = get("stricthostkeychecking", "ask");
        ensure!(
            ["yes", "ask", "accept-new", "no", "off"].contains(&strict.as_str()),
            "Invalid StrictHostKeyChecking"
        );
        let mut environment = BTreeMap::new();
        for pattern in parser.values.get("sendenv").into_iter().flatten() {
            for (name, value) in std::env::vars() {
                if match_pattern(pattern, &name, false) && value.len() <= 16384 {
                    environment.insert(name, value);
                }
            }
        }
        let mut explicit_environment = BTreeMap::new();
        for value in parser.values.get("setenv").into_iter().flatten() {
            let (name, value) = value.split_once('=').context("Invalid SSH SetEnv")?;
            ensure!(!name.is_empty(), "Invalid SSH SetEnv name");
            explicit_environment
                .entry(name.into())
                .or_insert(value.into());
        }
        environment.extend(explicit_environment);
        ensure!(
            environment.len() <= 64
                && environment
                    .iter()
                    .map(|(k, v)| k.len() + v.len())
                    .sum::<usize>()
                    <= 64 * 1024,
            "SSH environment exceeds limit"
        );
        Ok(Self {
            host,
            user,
            port,
            keys,
            known_hosts,
            host_key_alias: get("hostkeyalias", ""),
            strict,
            batch: yes("batchmode", false)?,
            identities_only: yes("identitiesonly", false)?,
            agent,
            forward_agent: yes("forwardagent", false)?,
            jump: if target && !profile.jump.is_empty() {
                profile.jump.clone()
            } else {
                get("proxyjump", "")
            },
            environment: environment.into_iter().collect(),
            password: yes("passwordauthentication", true)?,
            interactive: yes(
                "kbdinteractiveauthentication",
                yes("challengeresponseauthentication", true)?,
            )?,
            publickey: yes("pubkeyauthentication", true)?,
            gssapi: yes("gssapiauthentication", false)?,
            gssapi_delegate: yes("gssapidelegatecredentials", false)?,
            hash_hosts: yes("hashknownhosts", true)?,
            xauth: get("xauthlocation", "xauth"),
            x11_trusted: match (target, profile.x11_trust.override_value()) {
                (true, Some(trusted)) => trusted,
                _ => yes("forwardx11trusted", false)?,
            },
            x11_timeout: get("forwardx11timeout", "1200")
                .parse::<u64>()
                .context("Rust SSH ForwardX11Timeout must be seconds")?
                .min(86400),
        })
    }
    pub fn host_token(&self) -> String {
        let host = if self.host_key_alias.is_empty() {
            &self.host
        } else {
            &self.host_key_alias
        };
        if self.port == 22 || !self.host_key_alias.is_empty() {
            host.clone()
        } else {
            format!("[{host}]:{}", self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn x11_trust_overrides_only_the_target_and_preserves_config_defaults() {
        use crate::ssh::{Profile, X11Trust};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        for configured in [false, true] {
            std::fs::write(
                &path,
                format!(
                    "ForwardX11Trusted {}\n",
                    if configured { "yes" } else { "no" }
                ),
            )
            .unwrap();
            for trust in [X11Trust::Config, X11Trust::Untrusted, X11Trust::Trusted] {
                let profile = Profile {
                    host: "host".into(),
                    config: path.to_string_lossy().into(),
                    x11_trust: trust,
                    ..Default::default()
                };
                assert_eq!(
                    Config::resolve(&profile, "host", true).unwrap().x11_trusted,
                    trust.override_value().unwrap_or(configured)
                );
                assert_eq!(
                    Config::resolve(&profile, "jump", false)
                        .unwrap()
                        .x11_trusted,
                    configured
                );
            }
        }
        std::fs::write(&path, "").unwrap();
        let profile = Profile {
            host: "host".into(),
            config: path.to_string_lossy().into(),
            ..Default::default()
        };
        assert!(!Config::resolve(&profile, "host", true).unwrap().x11_trusted);
    }
    #[test]
    fn config_include_precedence_negation_and_literal_values() {
        let root = tempfile::tempdir().unwrap();
        let included = root.path().join("hosts");
        std::fs::write(&included,"Host cluster !blocked\n HostName 127.0.0.1\n Port 2201\n User included\n IdentityFile '/tmp/key with spaces'\nHost *\n Port 2222\n").unwrap();
        let main = root.path().join("config");
        std::fs::write(
            &main,
            format!(
                "User first\nInclude '{}'\nHost *\n User last\n",
                included.display()
            ),
        )
        .unwrap();
        let profile = super::super::super::Profile {
            host: "cluster".into(),
            config: main.to_string_lossy().into(),
            ..Default::default()
        };
        let resolved = Config::resolve(&profile, "cluster", true).unwrap();
        assert_eq!(
            (&*resolved.host, &*resolved.user, resolved.port),
            ("127.0.0.1", "first", 2201)
        );
        assert_eq!(resolved.keys, [PathBuf::from("/tmp/key with spaces")]);
        let explicit = Config::resolve(&profile, "name@cluster:2205", true).unwrap();
        assert_eq!((&*explicit.user, explicit.port), ("name", 2205));
        assert!(!patterns(&["*".into(), "!blocked".into()], "blocked"));
        assert!(wildcard("[::1]:22", "[::1]:22"));
    }
    #[test]
    fn config_rejects_executable_options_and_include_cycles() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        let profile = super::super::super::Profile {
            host: "host".into(),
            config: path.to_string_lossy().into(),
            ..Default::default()
        };
        for value in [
            "ProxyCommand ssh proxy",
            "Match exec true\n User wrong",
            "StrictHostKeyChecking invalid",
            "UserKnownHostsFile /dev/null",
            "GSSAPIAuthentication invalid",
            "GSSAPIDelegateCredentials invalid",
        ] {
            std::fs::write(&path, value).unwrap();
            assert!(Config::resolve(&profile, "host", true).is_err(), "{value}");
        }
        std::fs::write(&path, format!("Include {}", path.display())).unwrap();
        assert!(
            Config::resolve(&profile, "host", true)
                .unwrap_err()
                .to_string()
                .contains("Include limit")
        );
    }

    #[test]
    fn gssapi_defaults_follow_host_and_include_precedence() {
        let root = tempfile::tempdir().unwrap();
        let included = root.path().join("defaults");
        std::fs::write(
            &included,
            "Host *\n GSSAPIAuthentication yes\n GSSAPIDelegateCredentials yes\n",
        )
        .unwrap();
        let path = root.path().join("config");
        std::fs::write(&path, format!("Host keys-only\n GSSAPIAuthentication no\n GSSAPIDelegateCredentials no\nHost *\n Include '{}'\n", included.display())).unwrap();
        let profile = super::super::super::Profile {
            host: "cluster".into(),
            config: path.to_string_lossy().into(),
            ..Default::default()
        };
        assert!(Config::resolve(&profile, "cluster", true).unwrap().gssapi);
        assert!(Config::resolve(&profile, "jump", false).unwrap().gssapi);
        assert!(!Config::resolve(&profile, "keys-only", true).unwrap().gssapi);
        assert!(
            Config::resolve(&profile, "cluster", true)
                .unwrap()
                .gssapi_delegate
        );
        assert!(
            !Config::resolve(&profile, "keys-only", true)
                .unwrap()
                .gssapi_delegate
        );
    }

    #[test]
    fn environment_patterns_can_be_removed_and_preserve_case_and_precedence() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        std::fs::write(&path,
            "SendEnv LC_* LANG SECRET_*\nSendEnv -SECRET_* -LC_*\nSetEnv LANG=first LANG=duplicate\nSetEnv LANG=last\n"
        ).unwrap();
        let mut parser = Parser::default();
        parser
            .file(&path, "host", &mut true, 0, true, false)
            .unwrap();
        assert_eq!(parser.values["sendenv"], ["LANG"]);
        assert!(match_pattern("LC_*", "LC_TIME", false));
        assert!(!match_pattern("LC_*", "lc_secret", false));
        let profile = super::super::super::Profile {
            host: "host".into(),
            config: path.to_string_lossy().into(),
            ..Default::default()
        };
        let config = Config::resolve(&profile, "host", true).unwrap();
        assert_eq!(config.environment, [("LANG".into(), "first".into())]);
    }
    #[test]
    fn x11_timeout_zero_disables_expiry_without_rewriting_positive_values() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        let profile = super::super::super::Profile {
            host: "host".into(),
            config: path.to_string_lossy().into(),
            ..Default::default()
        };
        for timeout in [0, 1, 1200, 86400] {
            std::fs::write(&path, format!("ForwardX11Timeout {timeout}\n")).unwrap();
            let resolved = Config::resolve(&profile, "host", true).unwrap();
            assert_eq!(resolved.x11_timeout, timeout);
        }
    }
}

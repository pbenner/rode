//! Operation-scoped HTTPS credentials. Secrets cross only stdin/RPC and a sealed
//! anonymous descriptor; Git's configured credential writers are bypassed.
use super::{Output, git_command, run};
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

const LIMIT: usize = 256 * 1024;

/// Sanitized Git result and the next HTTPS challenge, if another origin needs credentials.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    /// Command output with supplied secrets removed.
    pub output: Output,
    /// Canonical HTTPS origin extracted before diagnostic redaction.
    pub challenge: Option<String>,
}

/// In-memory credentials for exactly one HTTPS origin. Debug output is redacted.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// Canonical HTTPS origin shown to the user, without a path or user information.
    pub origin: String,
    /// Username supplied for this operation only.
    pub username: String,
    /// Password or personal access token supplied for this operation only.
    pub password: String,
}
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials([redacted])")
    }
}
impl Drop for Credentials {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}
impl Credentials {
    /// Reject protocol injection and oversized values before starting Git.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            origin(&self.origin).as_deref() == Some(&self.origin),
            "Invalid Git authentication origin"
        );
        for (value, limit) in [(&self.username, 4096), (&self.password, 16384)] {
            ensure!(
                !value.is_empty() && value.len() <= limit && !value.contains(['\r', '\n', '\0']),
                "Git credentials must be nonempty single-line values within their size limits"
            );
        }
        Ok(())
    }
}

/// Canonical host and port for an HTTPS URL, without disclosing embedded credentials.
pub fn origin(value: &str) -> Option<String> {
    if value.len() > 8192 || value.contains(['\r', '\n', '\0']) {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    (url.scheme() == "https" && url.host_str().is_some())
        .then(|| url.origin().ascii_serialization())
}

/// Only explicit network operations may receive this credential channel.
pub fn network(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some("fetch" | "pull" | "push" | "clone" | "ls-remote")
    ) || (args.first().map(String::as_str) == Some("submodule")
        && args.get(1).map(String::as_str) == Some("update"))
}

/// Extract Git's failed HTTPS authentication context without keeping URL user information.
pub fn challenge(error: &str) -> Option<String> {
    for prefix in [
        "could not read Username for '",
        "could not read Password for '",
        "Authentication failed for '",
    ] {
        if let Some((_, suffix)) = error.split_once(prefix)
            && let Some((url, _)) = suffix.split_once('\'')
            && let Some(origin) = origin(url)
        {
            return Some(origin);
        }
    }
    None
}

fn redact(text: &str, credentials: &[Credentials]) -> String {
    let mut secrets = Zeroizing::new(Vec::new());
    for credential in credentials {
        secrets.push(STANDARD.encode(format!("{}:{}", credential.username, credential.password)));
        for value in [&credential.username, &credential.password] {
            secrets.push(value.clone());
            secrets.push(
                serde_json::to_string(value)
                    .unwrap()
                    .trim_matches('"')
                    .to_owned(),
            );
            for lower in [false, true] {
                secrets.push(
                    value
                        .bytes()
                        .map(|b| {
                            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                                (b as char).to_string()
                            } else if lower {
                                format!("%{b:02x}")
                            } else {
                                format!("%{b:02X}")
                            }
                        })
                        .collect::<String>(),
                );
            }
        }
    }
    secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
    let mut result = text.to_owned();
    for secret in secrets.iter() {
        if !secret.is_empty() {
            result = result.replace(secret.as_str(), "*");
        }
    }
    result
}

/// Run a credentialed command without invoking persistent credential helpers or
/// exposing credentials in argv, environment variables or diagnostic output.
pub fn execute(root: &Path, args: &[String], credentials: &[Credentials]) -> Result<Reply> {
    ensure!(
        network(args) && args.len() <= 128 && args.iter().map(String::len).sum::<usize>() <= 65536,
        "Invalid authenticated Git network command"
    );
    ensure!(
        !credentials.is_empty() && credentials.len() <= 8,
        "Invalid number of Git authentication origins"
    );
    for credential in credentials {
        credential.validate()?;
    }
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("Memory-only Git authentication requires Linux");
    #[cfg(target_os = "linux")]
    {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::process::CommandExt,
        };
        let bytes = Zeroizing::new(serde_json::to_vec(credentials)?);
        ensure!(
            bytes.len() <= LIMIT,
            "Git authentication exceeds its memory limit"
        );
        let flags = libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING;
        let mut fd =
            unsafe { libc::memfd_create(c"rode-git-auth".as_ptr(), flags | libc::MFD_NOEXEC_SEAL) };
        // Old kernels lack NOEXEC_SEAL. Match the session capability's fallback:
        // retry unsupported flags only, never a permission or seccomp denial.
        if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL) {
            fd = unsafe { libc::memfd_create(c"rode-git-auth".as_ptr(), flags) };
        }
        ensure!(fd >= 0, "Cannot create anonymous Git authentication memory");
        let mut memory = unsafe { std::fs::File::from_raw_fd(fd) };
        memory.write_all(&bytes)?;
        ensure!(
            unsafe {
                libc::fcntl(
                    fd,
                    libc::F_ADD_SEALS,
                    libc::F_SEAL_SEAL
                        | libc::F_SEAL_SHRINK
                        | libc::F_SEAL_GROW
                        | libc::F_SEAL_WRITE,
                )
            } == 0,
            "Cannot seal Git authentication memory"
        );
        let executable = crate::command::current_executable()?;
        let source = memory.as_raw_fd();
        let helper = format!(
            "!{} --git-credential {source}",
            crate::command::quote(
                executable
                    .to_str()
                    .context("Git helper path must be UTF-8")?
            )
        );
        let mut command = git_command(root);
        command.args([
            "-c",
            "credential.helper=",
            "-c",
            &format!("credential.helper={helper}"),
        ]);
        command.args([
            "-c",
            "trace2.normalTarget=0",
            "-c",
            "trace2.perfTarget=0",
            "-c",
            "trace2.eventTarget=0",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
        ]);
        command
            .env("GIT_ASKPASS", "/bin/false")
            .env("SSH_ASKPASS", "/bin/false");
        for (name, _) in std::env::vars_os() {
            let key = name.to_string_lossy();
            if key.starts_with("GIT_TRACE") || key == "GIT_CURL_VERBOSE" {
                command.env_remove(name);
            }
        }
        command.args(args);
        // The parent's descriptor stays CLOEXEC, including during concurrent GUI
        // launches. Only this fork's Git process inherits the sealed data.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(source, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let result = run(command, vec![], Duration::from_secs(120));
        match result {
            Ok(mut output) => {
                let challenge = (!output.success())
                    .then(|| challenge(&output.stderr))
                    .flatten();
                output.stdout = redact(&output.stdout, credentials);
                output.stderr = redact(&output.stderr, credentials);
                Ok(Reply { output, challenge })
            }
            Err(error) => Err(anyhow::anyhow!(redact(&format!("{error:#}"), credentials))),
        }
    }
}

/// Private credential-helper entry point. `store` and `erase` are ignored;
/// `get` supplies credentials only for the exact displayed HTTPS origin.
pub fn helper(args: &[String]) -> Result<()> {
    ensure!(args.len() == 2, "Invalid Git credential helper invocation");
    let mut input = Zeroizing::new(Vec::new());
    std::io::stdin().take(65537).read_to_end(&mut input)?;
    ensure!(
        input.len() <= 65536,
        "Git credential context exceeds its limit"
    );
    if args[1] != "get" {
        return Ok(());
    }
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("Memory-only Git authentication requires Linux");
    #[cfg(target_os = "linux")]
    {
        use std::os::{fd::FromRawFd, unix::fs::FileExt};
        let fd: i32 = args[0]
            .parse()
            .context("Invalid Git credential descriptor")?;
        ensure!(
            fd >= 3 && unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0,
            "Invalid Git credential descriptor"
        );
        let memory = unsafe { std::fs::File::from_raw_fd(fd) };
        let required =
            libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
        let seals = unsafe { libc::fcntl(fd, libc::F_GET_SEALS) };
        ensure!(
            seals >= 0 && seals & required == required,
            "Git credential descriptor is not sealed memory"
        );
        let length = memory.metadata()?.len() as usize;
        ensure!(length <= LIMIT, "Git credential memory exceeds its limit");
        let mut bytes = Zeroizing::new(vec![0; length]);
        memory.read_exact_at(&mut bytes, 0)?;
        let credentials: Vec<Credentials> =
            serde_json::from_slice(&bytes).context("Invalid Git credential memory")?;
        let text = std::str::from_utf8(&input).context("Invalid Git credential context")?;
        let fields: std::collections::BTreeMap<_, _> = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect();
        let protocol = fields.get("protocol").copied().unwrap_or_default();
        let host = fields.get("host").copied().unwrap_or_default();
        ensure!(
            !host.contains(['/', '@', '?', '#', '\\']),
            "Invalid Git credential host"
        );
        let requested = origin(&format!("{protocol}://{host}"));
        if let Some(credential) = credentials
            .iter()
            .find(|c| Some(c.origin.as_str()) == requested.as_deref())
        {
            credential.validate()?;
            let mut output = std::io::stdout().lock();
            writeln!(
                output,
                "username={}\npassword={}\n",
                credential.username, credential.password
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenges_keep_only_the_https_origin_and_port() {
        for error in [
            "fatal: could not read Username for 'https://git.example.test:8443': terminal prompts disabled",
            "fatal: could not read Password for 'https://old-user@git.example.test:8443/repo': terminal prompts disabled",
            "fatal: Authentication failed for 'https://old-user:old-pass@git.example.test:8443/repo/'",
        ] {
            assert_eq!(
                challenge(error).as_deref(),
                Some("https://git.example.test:8443")
            );
        }
        for error in [
            "Permission denied (publickey)",
            "fatal: Authentication failed for 'http://example.test/repo'",
            "could not read Username for 'file:///tmp/repo'",
        ] {
            assert!(challenge(error).is_none());
        }
    }

    #[test]
    fn credentials_reject_protocol_injection_and_noncanonical_origins() {
        let mut credentials = Credentials {
            origin: "https://git.example.test".into(),
            username: "literal user ' $()".into(),
            password: "literal password = $()".into(),
        };
        credentials.validate().unwrap();
        assert!(!format!("{credentials:?}").contains(&credentials.password));
        for invalid in ["", "line\nbreak", "line\rbreak", "nul\0byte"] {
            credentials.username = invalid.into();
            assert!(credentials.validate().is_err());
        }
        credentials.username = "user".into();
        for invalid in [
            "https://user@git.example.test",
            "https://git.example.test/repo",
            "http://git.example.test",
            "https://git.example.test?query",
        ] {
            credentials.origin = invalid.into();
            assert!(credentials.validate().is_err());
        }
    }

    #[test]
    fn diagnostic_redaction_covers_raw_url_and_basic_encoded_credentials() {
        let credentials = Credentials {
            origin: "https://example.test".into(),
            username: "private user".into(),
            password: "private/token".into(),
        };
        let encoded = STANDARD.encode("private user:private/token");
        let output = redact(
            &format!(
                "private user private/token private%20user private%2Ftoken private%2ftoken {encoded}"
            ),
            &[credentials],
        );
        assert_eq!(output, "* * * * * *");
    }

    #[test]
    fn credentials_are_never_attached_to_local_or_config_commands() {
        for command in ["fetch", "push", "pull", "clone", "ls-remote"] {
            assert!(network(&[command.into()]));
        }
        for command in ["config", "commit", "status", "credential", "-c"] {
            assert!(!network(&[command.into()]));
        }
        assert!(network(&["submodule".into(), "update".into()]));
        assert!(!network(&["submodule".into(), "status".into()]));
    }
}

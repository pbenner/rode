//! Select installed or desktop-supplied helpers and pin verified remote generations.
use super::*;
use std::path::PathBuf;

pub(super) struct Selected {
    pub executable: String,
    pub digest: String,
    pub info: Option<wire::helper::Info>,
}

/// Send a fixed ASCII script before its binary stdin payload. `head` consumes
/// exactly the frame, avoiding shell read-ahead into the uploaded executable.
/// The sentinel preserves trailing newlines and detects truncated script input.
fn streamed_script(script: &str, arguments: &[&str], payload: Vec<u8>) -> (String, Vec<u8>) {
    assert!(script.is_ascii());
    let loader = "code=$(head -c \"$1\" && printf .) || exit; [ \"${#code}\" -eq \"$(($1 + 1))\" ] || exit 1; shift; eval \"${code%.}\"";
    let mut command = format!("exec sh -c {} sh {}", ssh::quote(loader), script.len());
    for argument in arguments {
        command.push(' ');
        command.push_str(&ssh::quote(argument));
    }
    let mut input = Vec::with_capacity(script.len() + payload.len());
    input.extend_from_slice(script.as_bytes());
    input.extend(payload);
    (command, input)
}

fn discovery(
    remote: &ssh::Remote,
    path: &str,
    timeout: Duration,
    output_limit: usize,
) -> Result<Vec<u8>> {
    ensure!(
        !remote.native.closed.load(Ordering::Acquire),
        "SSH disconnected during helper discovery"
    );
    let mut command = Command::new(remote.client_program());
    let (script, input) = streamed_script(include_str!("../installed.sh"), &[path], vec![]);
    command.args(remote.channel_args(false)).arg(script);
    rode_host_core::command::run_bounded_limited(command, input, timeout, output_limit)
}

fn probe(
    remote: &ssh::Remote,
    path: &str,
    architecture: &str,
    timeout: Duration,
) -> Result<wire::helper::Installed> {
    let output = discovery(remote, path, timeout, wire::helper::MAX_INFO)?;
    let installed: wire::helper::Installed = serde_json::from_slice(&output)
        .context("Installed helper does not provide valid --host-info metadata")?;
    installed
        .validate("linux", architecture, rode_session::PROTOCOL)
        .map_err(anyhow::Error::msg)?;
    Ok(installed)
}

fn installed(remote: &ssh::Remote, architecture: &str) -> Result<Option<wire::helper::Installed>> {
    if !remote.profile.remote_helper.is_empty() {
        return probe(
            remote,
            &remote.profile.remote_helper,
            architecture,
            Duration::from_secs(15),
        )
        .map(Some);
    }
    let started = Instant::now();
    let output = discovery(remote, "--list", Duration::from_secs(10), 32 * 1024)?;
    let list = std::str::from_utf8(&output)?
        .strip_prefix("RODE-HELPERS\0")
        .context("Invalid installed helper discovery response")?;
    ensure!(
        list.is_empty() || list.ends_with('\0'),
        "Truncated installed helper candidate list"
    );
    let paths = list.split_terminator('\0').collect::<Vec<_>>();
    ensure!(paths.len() <= 5, "Too many installed helper candidates");
    let mut seen = std::collections::HashSet::new();
    let mut errors = vec![];
    for path in paths {
        ensure!(
            path.starts_with('/') && path.len() <= 4096 && !path.chars().any(char::is_control),
            "Invalid installed helper candidate path"
        );
        if !seen.insert(path) {
            continue;
        }
        let Some(remaining) = Duration::from_secs(30).checked_sub(started.elapsed()) else {
            bail!("Installed helper discovery timed out");
        };
        match probe(
            remote,
            path,
            architecture,
            remaining.min(Duration::from_secs(10)),
        ) {
            Ok(helper) => return Ok(Some(helper)),
            Err(error) => errors.push(format!("{path}: {error:#}")),
        }
    }
    if !errors.is_empty() {
        bail!("No compatible installed helper: {}", errors.join("; "));
    }
    Ok(None)
}

pub(super) fn select(remote: &ssh::Remote, architecture: &str) -> Result<Selected> {
    ensure!(
        matches!(architecture, "x86_64" | "aarch64"),
        "Native remote helper supports Linux x86_64 and aarch64"
    );
    let explicit = !remote.profile.remote_helper.is_empty();
    // Preserve explicit desktop artifact overrides; a per-host remote path is
    // more specific and must either succeed or fail without another selection.
    let discover = explicit
        || (std::env::var_os("RODE_HOST_HELPER").is_none()
            && std::env::var_os("RODE_SESSION_HELPER").is_none());
    let mut rejected = None;
    if discover {
        match installed(remote, architecture) {
            Ok(Some(installed)) => {
                let executable = publish(
                    remote,
                    &installed.digest,
                    installed.size,
                    &installed.executable,
                    vec![],
                )?;
                return Ok(Selected {
                    executable,
                    digest: installed.digest,
                    info: Some(installed.info),
                });
            }
            Ok(None) => {}
            Err(error) if explicit => {
                return Err(error.context("Cannot use the configured remote helper"));
            }
            Err(error) => rejected = Some(format!("Installed helper rejected: {error:#}")),
        }
    }
    let bytes = artifact(architecture).with_context(|| rejected.unwrap_or_else(|| {
        if discover {
            "No compatible installed helper was found; install rode-sessiond remotely or supply a matching desktop artifact".into()
        } else {
            "Cannot load the explicitly configured desktop helper".into()
        }
    }))?;
    let digest = files::digest(&bytes);
    let executable = publish(remote, &digest, bytes.len() as u64, "", bytes)?;
    Ok(Selected {
        executable,
        digest,
        info: None,
    })
}

fn publish(
    remote: &ssh::Remote,
    digest: &str,
    size: u64,
    source: &str,
    bytes: Vec<u8>,
) -> Result<String> {
    ensure!(
        !remote.native.closed.load(Ordering::Acquire),
        "SSH disconnected during helper discovery"
    );
    let mut install = Command::new(remote.client_program());
    let (script, input) = streamed_script(
        include_str!("../bootstrap.sh"),
        &[digest, &size.to_string(), source],
        bytes,
    );
    install.args(remote.channel_args(false)).arg(script);
    let output = rode_host_core::command::run_bounded_limited(
        install,
        input,
        Duration::from_secs(60),
        4097,
    )?;
    let executable = std::str::from_utf8(&output)?
        .trim_end_matches('\n')
        .to_owned();
    ensure!(
        executable.starts_with('/')
            && executable.len() <= 4096
            && !executable.chars().any(char::is_control),
        "Invalid native helper deployment response"
    );
    Ok(executable)
}

fn artifact(architecture: &str) -> Result<Vec<u8>> {
    ensure!(
        matches!(architecture, "x86_64" | "aarch64"),
        "Native remote helper supports Linux x86_64 and aarch64"
    );
    let executable = std::env::current_exe()?;
    let bin = executable
        .parent()
        .context("Missing executable directory")?;
    let override_path = std::env::var_os("RODE_HOST_HELPER")
        .or_else(|| std::env::var_os("RODE_SESSION_HELPER"))
        .map(PathBuf::from);
    let mut candidates = vec![];
    if let Some(path) = override_path {
        candidates.push(if path.is_dir() {
            path.join(architecture).join("rode-sessiond")
        } else {
            path
        });
    } else {
        candidates.push(
            bin.join("host-helpers")
                .join(architecture)
                .join("rode-sessiond"),
        );
        if let Some(target) = bin.parent() {
            candidates.push(target.join(format!(
                "{architecture}-unknown-linux-musl/release/rode-sessiond-standalone"
            )));
        }
        if architecture == std::env::consts::ARCH {
            candidates.extend([bin.join("rode-sessiond-static"), bin.join("rode-sessiond")]);
        }
    }
    let path = candidates.into_iter().find(|p| p.is_file()).with_context(|| format!("No packaged native helper for the remote architecture. Run cargo xtask build-helper {architecture} or set RODE_HOST_HELPER to a matching release helper; no download or compiler is started automatically."))?;
    let bytes = rode_host_core::persistence::read(&path, wire::helper::MAX_EXECUTABLE as usize)?
        .context("Missing native helper")?;
    ensure!(
        bytes.len() >= 20 && bytes.starts_with(b"\x7fELF\x02\x01"),
        "Native helper must be a little-endian 64-bit Linux ELF executable"
    );
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    ensure!(
        (architecture == "x86_64" && machine == 62)
            || (architecture == "aarch64" && machine == 183),
        "Selected helper does not match the remote architecture"
    );
    Ok(bytes)
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn script_stream_preserves_literal_arguments_and_binary_payload() {
        let script = format!(
            "# {}\nprintf '%s\\000' \"$1\"\ncat\n",
            "large fixed script ".repeat(8192)
        );
        let payload = (0..32768).map(|n| n as u8).collect::<Vec<_>>();
        let argument = "literal 'quotes' $HOME\n🦀";
        let (line, input) = streamed_script(&script, &[argument], payload.clone());
        assert!(line.len() < 512);
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &line]);
        let output =
            rode_host_core::command::run_bounded(command, input, Duration::from_secs(5)).unwrap();
        let mut expected = argument.as_bytes().to_vec();
        expected.push(0);
        expected.extend(payload);
        assert_eq!(output, expected);
    }

    #[test]
    fn truncated_script_is_rejected_before_execution() {
        let root = tempfile::tempdir().unwrap();
        let script = "touch marker\n# trailing frame data\n";
        let (line, mut input) = streamed_script(script, &[], vec![]);
        input.truncate("touch marker\n".len());
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &line]).current_dir(root.path());
        assert!(
            rode_host_core::command::run_bounded(command, input, Duration::from_secs(5)).is_err()
        );
        assert!(!root.path().join("marker").exists());
    }
}

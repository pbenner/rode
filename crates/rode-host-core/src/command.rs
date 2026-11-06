//! Literal quoting, executable resolution and bounded host-process capture.
//!
//! This runner serves local helpers and remote transport commands. Keep errors
//! tied to the actual executable, preserve binary stdout, and bound stderr
//! independently; only the caller knows whether a missing tool is OpenSSH.
use anyhow::{Context, Result, bail};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
/// Quote one argument for a POSIX remote shell, including embedded single quotes.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Concurrently drain both pipes and stdin with the default 64 MiB stdout bound.
pub fn run_bounded(command: Command, input: Vec<u8>, timeout: Duration) -> Result<Vec<u8>> {
    run_bounded_limited(command, input, timeout, 64 * 1024 * 1024)
}

/// Run a host command with a caller-specific stdout cap, enforced while reading.
/// Stderr retains its separate 64 KiB diagnostic limit.
pub fn run_bounded_limited(
    mut command: Command,
    input: Vec<u8>,
    timeout: Duration,
    output_limit: usize,
) -> Result<Vec<u8>> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Could not start {program}"))?;
    let mut stdin = child.stdin.take().context("Missing SSH stdin")?;
    let stdout = child.stdout.take().context("Missing SSH stdout")?;
    let stderr = child.stderr.take().context("Missing SSH stderr")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out_tx = tx.clone();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(output_limit.saturating_add(1) as u64)
            .read_to_end(&mut bytes);
        let _ = out_tx.send((true, bytes, result));
    });
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stderr.take(64 * 1024 + 1).read_to_end(&mut bytes);
        let _ = tx.send((false, bytes, result));
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{program} timed out; the operation did not finish within its deadline.");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut output = Vec::new();
    let mut error = Vec::new();
    for _ in 0..2 {
        let (is_out, bytes, result) = rx
            .recv_timeout(Duration::from_secs(2))
            .with_context(|| format!("{program} output exceeded its limit or did not close"))?;
        result?;
        if is_out {
            output = bytes;
        } else {
            error = bytes;
        }
    }
    if output.len() > output_limit {
        bail!("{program} response exceeded its size limit.");
    }
    if !status.success() {
        let detail = String::from_utf8_lossy(&error);
        if !detail.trim().is_empty() {
            bail!("{}", detail.trim());
        }
        bail!("{program} failed ({status})");
    }
    Ok(output)
}

/// Detach an owned child group from the caller’s controlling terminal.
pub fn new_session(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // The post-fork hook only calls the async-signal-safe setsid syscall and
    // captures errno on failure; it does not allocate or acquire Rust locks.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Pin the running executable across replacement.
pub fn current_executable() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        Ok(PathBuf::from(format!("/proc/{}/exe", std::process::id())))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(std::env::current_exe()?)
    }
}
/// Generate a random opaque terminal identifier from the host's entropy source.
pub fn new_id() -> Result<String> {
    let mut bytes = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!(
        "s-{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
/// Resolve literal program paths on the execution host, without a shell.
pub fn resolve_program(root: &std::path::Path, program: &str) -> PathBuf {
    if program.contains('/') || program.starts_with('~') {
        let path = crate::files::expand_home(program);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    } else {
        crate::sandbox::find_program(program)
            .or_else(|| {
                let local = crate::files::expand_home("~/.local/bin").join(program);
                local.is_file().then_some(local)
            })
            .unwrap_or_else(|| PathBuf::from(program))
    }
}

/// Set only by a transient helper when its owning input channel closes.
pub static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_command_failures_identify_the_executable_and_keep_silent_exits_informative() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-helper");
        let error =
            run_bounded(Command::new(&missing), vec![], Duration::from_secs(5)).unwrap_err();
        assert!(error.to_string().contains(missing.to_str().unwrap()));
        assert!(!error.to_string().contains("OpenSSH"));

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 7"]);
        let error = run_bounded(command, vec![], Duration::from_secs(5)).unwrap_err();
        assert!(error.to_string().contains("/bin/sh"));
        assert!(error.to_string().contains("7"));

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'setup failed\\n' >&2; exit 1"]);
        let error = run_bounded(command, vec![], Duration::from_secs(5)).unwrap_err();
        assert_eq!(error.to_string(), "setup failed");
    }

    #[test]
    fn small_ssh_output_limits_preserve_binary_data_and_reject_overflow() {
        let data = vec![0, 255, 10, 42];
        let output = run_bounded_limited(
            Command::new("/bin/cat"),
            data.clone(),
            Duration::from_secs(5),
            data.len(),
        )
        .unwrap();
        assert_eq!(output, data);
        let error = run_bounded_limited(Command::new("/bin/cat"), data, Duration::from_secs(5), 3)
            .unwrap_err();
        assert!(error.to_string().contains("size limit"), "{error:#}");
    }
}

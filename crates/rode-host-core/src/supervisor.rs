//! Native stdio supervisor for one transient project process. No listener or persistent owner.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, Read, Write},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Structured launch metadata sent before the child's own input stream.
#[derive(Serialize, Deserialize)]
pub struct Spec {
    #[serde(default)]
    /// Expect periodic empty lines from a desktop that owns this transient view.
    pub heartbeat: bool,
    /// Captured runtime overrides and literal values.
    pub environment: crate::environment::Settings,
    /// Canonical project root on this host.
    pub root: String,
    /// Captured outer execution policy followed by the environment provider.
    pub prefix: Vec<String>,
    /// Literal target executable and arguments.
    pub argv: Vec<String>,
    /// Selected project interpreter, resolved on this host.
    pub python: String,
    /// Use the selected interpreter as the executable.
    pub python_program: bool,
    #[serde(default)]
    /// Launch the built-in native debugger coordinator.
    pub native_debug: bool,
}

extern "C" fn interrupted(_: libc::c_int) {
    crate::command::CANCELLED.store(true, Ordering::Release);
}
/// Install simple cancellation handlers in a dedicated helper process, never in the GUI.
pub fn signals() -> Result<()> {
    for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = interrupted as *const () as usize;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
        }
        ensure!(
            unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } == 0,
            "Cannot install helper cancellation handler"
        );
    }
    Ok(())
}

/// Forward bounded lines while retaining a live supervisor for the complete child lifetime.
pub fn serve() -> Result<()> {
    let mut input = std::io::BufReader::new(crate::connection::Input::stdin()?);
    let mut bytes = vec![];
    input
        .by_ref()
        .take(256 * 1024 + 1)
        .read_until(b'\n', &mut bytes)?;
    ensure!(
        bytes.len() <= 256 * 1024 && bytes.last() == Some(&b'\n'),
        "Invalid process launch request"
    );
    let mut spec: Spec = serde_json::from_slice(&bytes)?;
    let lease = input.get_ref().lease();
    if !spec.heartbeat {
        input.get_mut().unbounded();
    }
    spec.environment.validate()?;
    ensure!(
        !spec.argv.is_empty()
            && spec.argv.len() <= 64
            && spec.argv.iter().map(String::len).sum::<usize>() <= 128 * 1024,
        "Invalid process argument vector"
    );
    ensure!(
        spec.prefix.len() <= 256
            && spec.prefix.iter().map(String::len).sum::<usize>() <= 128 * 1024,
        "Invalid process sandbox prefix"
    );
    let root = crate::files::expand_home(&spec.root).canonicalize()?;
    let python = crate::project::python_path(&root, &spec.python);
    if spec.native_debug || spec.argv[0] == "@rode/vault" {
        spec.argv = vec![
            std::env::current_exe()?.to_string_lossy().into_owned(),
            if spec.native_debug {
                "--debug-helper"
            } else {
                "--vault-helper"
            }
            .into(),
        ];
    } else {
        spec.argv[0] = if spec.python_program {
            python.clone()
        } else if spec.environment.mise
            && !spec.argv[0].contains('/')
            && !spec.argv[0].starts_with('~')
        {
            spec.argv[0].clone().into()
        } else {
            crate::command::resolve_program(&root, &spec.argv[0])
        }
        .to_string_lossy()
        .into_owned();
    }
    let argv = spec.prefix.into_iter().chain(spec.argv).collect::<Vec<_>>();
    let mut command = crate::command::from_argv(&argv)?;
    command
        .current_dir(&root)
        .env("NO_COLOR", "1")
        .envs(crate::project::environment_variables(
            &python.to_string_lossy(),
        ))
        .stdin(Stdio::piped());
    crate::command::new_session(&mut command);
    signals()?;
    let until = Instant::now() + Duration::from_secs(1);
    let mut child = loop {
        ensure!(
            !crate::command::CANCELLED.load(Ordering::Acquire),
            "Process launch cancelled"
        );
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(10))
            }
            result => break result.context("Cannot start project process")?,
        }
    };
    let pid = child.id() as i32;
    let mut child_input = child.stdin.take().context("Missing child stdin")?;
    let closed = Arc::new(AtomicBool::new(false));
    let writer_closed = closed.clone();
    std::thread::spawn(move || {
        let _ = (|| -> Result<()> {
            loop {
                let mut line = vec![];
                input
                    .by_ref()
                    .take(16 * 1024 * 1024 + 1)
                    .read_until(b'\n', &mut line)?;
                if line.is_empty() {
                    return Ok(());
                }
                ensure!(
                    line.len() <= 16 * 1024 * 1024,
                    "Process request exceeds 16 MiB"
                );
                if spec.heartbeat && line == b"\n" {
                    continue;
                }
                child_input.write_all(&line)?;
                child_input.flush()?;
            }
        })();
        writer_closed.store(true, Ordering::Release);
    });
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if closed.load(Ordering::Acquire)
            || crate::command::CANCELLED.load(Ordering::Acquire)
            || (spec.heartbeat && lease.expired())
        {
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
            let until = Instant::now() + Duration::from_secs(1);
            while Instant::now() < until && child.try_wait()?.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    ensure!(status.success(), "Project process exited with {status}");
    Ok(())
}

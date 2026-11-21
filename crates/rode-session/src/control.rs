//! Authenticated control client, service discovery and serialized service startup.
//!
//! A request's startup policy never replaces authentication. Discovery cannot
//! create a daemon, and an authentication denial never proves the service is absent.
//! Existing jobs are left intact when a control connection cannot be established.
#[cfg(target_os = "linux")]
use crate::{control_address, control_auth};
use crate::{
    framing::{recv_json, send_json},
    protocol::{PROTOCOL, Request, Response},
    runtime_directory::{directory, private_directory_at, runtime_path},
};
use anyhow::{Context, Result, ensure};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::{
    fs,
    os::unix::{fs::MetadataExt, net::UnixStream, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Executable identity used to start this client's matching service generation.
#[derive(Clone)]
pub struct Launcher {
    /// Native helper executable path, which may refer to the running image after an upgrade.
    pub executable: PathBuf,
    /// Arguments inserted before the service subcommand, such as `--session-service`.
    pub prefix: Vec<String>,
}

/// Client-side availability handling, separate from server authorization.
/// Discovery and creation may retry connection failures; other operations never
/// start a daemon or replay a control request.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StartupPolicy {
    Discovery,
    StartIfMissing,
    ExistingOnly,
}

impl StartupPolicy {
    fn for_request(request: &Request) -> Self {
        match request {
            Request::List | Request::Agents | Request::Runtimes | Request::Tunnels => {
                Self::Discovery
            }
            Request::StartTunnel { .. }
            | Request::Create { .. }
            | Request::CreateAgent { .. }
            | Request::CreateRuntime { .. } => Self::StartIfMissing,
            _ => Self::ExistingOnly,
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(super) struct ControlAuthentication;

#[cfg(target_os = "linux")]
impl std::fmt::Display for ControlAuthentication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Cannot authenticate terminal control")
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(super) struct ControlDisconnected;

#[cfg(target_os = "linux")]
impl std::fmt::Display for ControlDisconnected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Terminal service disconnected during control authentication")
    }
}

#[cfg(target_os = "linux")]
fn connect_authenticated(
    dir: &Path,
    address: &std::os::unix::net::SocketAddr,
) -> Result<UnixStream> {
    let mut stream = UnixStream::connect_addr(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    control_auth::authenticate(&mut stream, dir)
        .map_err(|error| control_authentication_error(&stream, error))
        .context(ControlAuthentication)?;
    Ok(stream)
}

#[cfg(target_os = "linux")]
pub(super) fn control_authentication_error(
    stream: &UnixStream,
    error: anyhow::Error,
) -> anyhow::Error {
    // An exiting service can lose access to its proc descriptor before the
    // kernel closes its sockets. Retry only after that connected peer has
    // actually disconnected; a live service's capability denial stays fatal.
    if error.downcast_ref::<std::io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
        )
    }) {
        let mut fd = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLRDHUP,
            revents: 0,
        };
        if unsafe { libc::poll(&mut fd, 1, 100) } > 0
            && fd.revents & (libc::POLLHUP | libc::POLLRDHUP) != 0
        {
            return error.context(ControlDisconnected);
        }
    }
    error
}

/// Authenticate control and require the current protocol in a private runtime directory.
pub fn connect_control(dir: &Path) -> Result<UnixStream> {
    let meta = fs::symlink_metadata(dir)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
        "Unsafe session directory"
    );
    #[cfg(target_os = "linux")]
    let mut stream = connect_authenticated(dir, &control_address(&meta)?)
        .context("Cannot access terminal control; use a client outside the project sandbox")?;
    #[cfg(not(target_os = "linux"))]
    let mut stream = UnixStream::connect(dir.join("socket"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    send_json(&mut stream, &Request::Protocol)?;
    ensure!(
        matches!(recv_json::<_, Response>(&mut stream)?.checked()?, Response::Protocol(version) if version == PROTOCOL),
        "Terminal service protocol mismatch; use the matching service in a new runtime directory"
    );
    Ok(stream)
}

/// Perform a blocking control request, starting a matching service when needed. Service
/// creation is serialized; only the current runtime directory is used.
pub fn request(request: &Request, launcher: &Launcher) -> Result<Response> {
    let policy = StartupPolicy::for_request(request);
    let until = Instant::now()
        + if policy == StartupPolicy::Discovery {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(10)
        };
    loop {
        let result = request_current(request, launcher);
        let retry = policy != StartupPolicy::ExistingOnly
            && result.as_ref().err().is_some_and(|error| {
                #[cfg(target_os = "linux")]
                if error.downcast_ref::<ControlDisconnected>().is_some() {
                    return true;
                }
                error.to_string() == "Session service is stopping"
                    || error.chain().any(|cause| {
                        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
                            matches!(
                                io.kind(),
                                std::io::ErrorKind::ConnectionRefused
                                    | std::io::ErrorKind::ConnectionReset
                                    | std::io::ErrorKind::BrokenPipe
                                    | std::io::ErrorKind::UnexpectedEof
                            )
                        })
                    })
            });
        if !retry || Instant::now() >= until {
            return result;
        }
        // Last-view cleanup or End may race the daemon unlinking its socket.
        // Retry only the discovery/creation requests selected above.
        std::thread::sleep(Duration::from_millis(50));
    }
}
pub(super) fn service_offline(error: &anyhow::Error) -> bool {
    // A missing capability or proc descriptor is an authentication failure,
    // not evidence that the listener and its jobs have gone away.
    #[cfg(target_os = "linux")]
    if error.downcast_ref::<ControlAuthentication>().is_some() {
        return false;
    }
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            )
        })
    })
}
fn request_current(request: &Request, launcher: &Launcher) -> Result<Response> {
    let response = match request_once(request, launcher, &directory()?) {
        Err(error) if service_offline(&error) => match request {
            Request::List => Response::Sessions(vec![]),
            Request::Agents => Response::Agents(vec![]),
            Request::Runtimes => Response::Runtimes(vec![]),
            Request::Tunnels => Response::Tunnels(vec![]),
            Request::StopTunnel { .. }
            | Request::End { .. }
            | Request::EndAgent { .. }
            | Request::EndRuntime { .. }
            | Request::Shutdown => Response::Ok,
            _ => return Err(error),
        },
        result => result?,
    };
    Ok(match response {
        Response::Sessions(mut infos) => {
            infos.retain(|info| info.alive);
            infos.sort_by_key(|info| info.created);
            Response::Sessions(infos)
        }
        Response::Agents(mut infos) => {
            infos.retain(|info| info.alive);
            infos.sort_by_key(|info| info.created);
            Response::Agents(infos)
        }
        Response::Runtimes(mut infos) => {
            infos.sort_by_key(|info| info.created);
            Response::Runtimes(infos)
        }
        response => response,
    })
}
pub(super) fn request_once(request: &Request, launcher: &Launcher, dir: &Path) -> Result<Response> {
    let mut stream = match connect_control(dir) {
        Ok(stream) => stream,
        Err(error) => {
            match StartupPolicy::for_request(request) {
                StartupPolicy::Discovery => {
                    return Err(error)
                        .context("Session service is unavailable; existing jobs were not touched");
                }
                StartupPolicy::ExistingOnly => {
                    return Err(error)
                        .context("Session service is offline; no session was created or ended.");
                }
                StartupPolicy::StartIfMissing => {}
            }
            if !service_offline(&error) {
                return Err(error);
            }
            let directory = private_directory_at(dir)?;
            let log = rode_host_core::locking::open_private(
                &runtime_path(dir, &directory).join("service.log"),
                rode_host_core::locking::PrivateFile::Append,
            )
            .with_context(|| {
                format!(
                    "Cannot open private session log {}",
                    dir.join("service.log").display()
                )
            })?;
            let mut command = Command::new(&launcher.executable);
            command
                .args(&launcher.prefix)
                .arg("serve")
                .env("RODE_SESSION_DIR", dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(log);
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = command.spawn().with_context(|| {
                format!(
                    "Cannot start native session service executable {}",
                    launcher.executable.display()
                )
            })?;
            // Reap our daemon if it exits while this client remains alive.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(stream) = connect_control(dir) {
                    break stream;
                }
                ensure!(
                    Instant::now() < until,
                    "Terminal service did not start; check {}",
                    dir.join("service.log").display()
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    send_json(&mut stream, request)?;
    recv_json::<_, Response>(&mut stream)?.checked()
}

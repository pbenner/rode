//! A headless, per-user PTY and coding-agent service. GUI clients may disconnect
//! without ending explicitly persistent jobs. The only listener is an owner-only
//! Unix domain socket.
//!
//! Control requests use tagged JSON. Attachments use a separate framed stream
//! for terminal bytes, resize and lifetime updates. A session owns the PTY and
//! spawning thread; views own only attachments. Nonpersistent shells end after
//! their last view closes, while persistent shells survive disconnection.
//! Creation and idle shutdown share synchronization so an empty service cannot
//! race a newly accepted shell. Never unlink the stable service lock file.
//!
//! # Design decisions
//!
//! PTYs belong to a small headless host process because a GUI or SSH attachment
//! cannot own a shell that must survive its own exit. A native service removes
//! the tmux/screen prerequisite and lets multiple editor windows share views.
//! The cost is a separate helper deployment and a protocol/lifetime contract.
//! An owner-only Unix socket keeps this local to the host's user account without
//! adding an application network listener or a system-wide service.
//!
//! Persistence is a property of the session, not a particular window: closing
//! one of several views must not end another view's shell. Explicit termination
//! and last-view cleanup therefore differ. Capability probing and separate
//! service generations preserve running jobs during editor upgrades instead of
//! restarting a service merely to obtain newer behavior.
//!
//! Agent controllers share this host lifetime but use a single controlling view
//! rather than PTY-style shared input. Their complete protocol/history state
//! comes from `rode-agent-core`, keeping remote helpers independent of the GUI.
//! Pending questions wait without a view; persistence never broadens policy.
#![warn(missing_docs)]
use anyhow::{Context, Result, bail, ensure};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod agents;
mod runtimes;

/// Runtime-directory generation; distinct from optional protocol feature levels.
pub const VERSION: u32 = 1;
/// Feature level that supports explicit persistence toggles.
pub const PERSISTENCE_PROTOCOL: u32 = 2;
/// Current feature level, including automatic host-owned prompt queue dispatch.
pub const PROTOCOL: u32 = 9;
/// Explanation shown for older services whose lifetime policy cannot be changed.
pub const LEGACY_PERSISTENCE_HELP: &str = "This terminal uses an older service and keeps its original lifetime setting. Start a new terminal to use the Persist toggle; existing jobs keep running.";
const MAX_FRAME: usize = 4 * 1024 * 1024;
const MAX_SESSIONS: usize = 32;
const MAX_CLIENTS: usize = 8;
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Relaunch this running image, including after a Linux upgrade unlinks it.
/// A parent-qualified proc path also works when a PTY launcher forks first.
pub fn current_executable() -> Result<PathBuf> {
    let path = std::env::current_exe().context("Cannot locate the running executable")?;
    #[cfg(target_os = "linux")]
    {
        let running = PathBuf::from(format!("/proc/{}/exe", std::process::id()));
        let image = fs::metadata(&running)?;
        if !fs::metadata(&path)
            .is_ok_and(|file| file.dev() == image.dev() && file.ino() == image.ino())
        {
            return Ok(running);
        }
    }
    Ok(path)
}
/// Host-service metadata for one terminal; no live PTY handle is transferred to clients.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    /// Opaque native session identifier.
    pub id: String,
    /// User-visible session name.
    pub name: String,
    /// Project working directory on the service host.
    pub root: String,
    /// Fingerprint of the creating workspace's actual sandbox plan.
    pub policy: String,
    /// Display label for the actual launch mode.
    pub mode: String,
    /// Whether the shell is still running.
    pub alive: bool,
    /// Number of currently attached terminal views.
    pub clients: usize,
    /// Creation time in Unix seconds.
    pub created: u64,
    /// Exit status when known; completed records are normally reaped promptly.
    pub exit_code: Option<u32>,
    /// Keep running with no attached views; legacy records default to their historical
    /// persistent behavior.
    #[serde(default = "persistent_default")]
    pub persistent: bool,
    /// Whether this service can change persistence in place.
    #[serde(default)]
    pub persistence_supported: bool,
    /// Scheduler job name for a managed interactive allocation, retained across
    /// renames and shared attachments. Absence also supports older services.
    #[serde(default)]
    pub allocation: Option<String>,
}
fn persistent_default() -> bool {
    true
}
/// Owner-only Unix-socket control protocol, serialized as JSON with an `action` tag.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    /// Probe backend history deletion before creating a browser in an older service.
    SupportsHistoryDeletion,
    /// Probe host-owned notebook/debugger controllers without starting work.
    SupportsRuntimes,
    /// Probe prompt dispatch that continues without a controlling desktop view.
    SupportsAutomaticQueue,
    /// Discover retained notebook and debugger runtimes.
    Runtimes,
    /// Create a captured host-owned runtime.
    CreateRuntime {
        /// Immutable launch and lifetime context.
        spec: Box<rode_runtime_core::service::Spec>,
    },
    /// Attach the exclusive controlling desktop.
    AttachRuntime {
        /// Native runtime identity.
        id: String,
        /// Original workspace policy fingerprint.
        policy: String,
    },
    /// End a runtime and discard retained output.
    EndRuntime {
        /// Native runtime identity.
        id: String,
        /// Original workspace policy fingerprint.
        policy: String,
    },
    /// List running sessions without keeping an otherwise idle service alive.
    List,
    /// Probe persistence support before sending newer requests to legacy services.
    SupportsPersistence,
    /// Probe allocation metadata before creating work in an older service.
    SupportsAllocations,
    /// Probe native agent-controller support without starting an agent.
    SupportsAgents,
    /// List live agents without starting a service or inference.
    Agents,
    /// Create a host-owned conversation controller on explicit Send/History.
    CreateAgent {
        /// Captured and validated host launch context.
        spec: Box<rode_agent_core::service::Spec>,
    },
    /// Attach the sole controlling desktop to a live agent.
    AttachAgent {
        /// Native agent identity.
        id: String,
        /// Original policy fingerprint.
        policy: String,
    },
    /// Explicitly terminate one agent and its scoped process tree.
    EndAgent {
        /// Native agent identity.
        id: String,
        /// Original policy fingerprint.
        policy: String,
    },
    /// Create a supervised PTY shell with explicit lifetime and environment.
    Create {
        /// Unique identifier for the new session.
        id: String,
        /// Initial user-visible session name.
        name: String,
        /// Initial working directory on the service host.
        root: String,
        /// Workspace policy fingerprint checked by later mutations/attachments.
        policy: String,
        /// Actual launch mode label for discovery.
        mode: String,
        /// Executable and literal arguments, including any enforcing sandbox prefix.
        argv: Vec<String>,
        /// Startup shell input sent once, never replayed when reattaching.
        #[serde(default)]
        initial_input: String,
        /// Environment captured from the creating caller.
        env: BTreeMap<String, String>,
        /// Whether to retain the shell after its final view disconnects.
        #[serde(default = "persistent_default")]
        persistent: bool,
        /// Unique scheduler job name, if this terminal owns an allocation.
        #[serde(default)]
        allocation: Option<String>,
    },
    /// Attach a streaming view after verifying the workspace policy fingerprint.
    Attach {
        /// Session to attach.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
    },
    /// Rename a terminal without restarting its shell.
    Rename {
        /// Session to rename.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
        /// Replacement display name.
        name: String,
    },
    /// Change last-view lifetime without restarting the shell.
    SetPersistence {
        /// Session to update.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
        /// Desired persistence after the final view disconnects.
        persistent: bool,
    },
    /// Explicitly terminate one session and its owned descendants.
    End {
        /// Session to terminate.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
    },
    /// Stop the service only when no running sessions prevent shutdown.
    Shutdown,
}
/// Tagged JSON reply to a session-service control request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Response {
    /// Discoverable retained notebook/debugger controllers.
    Runtimes(Vec<rode_runtime_core::service::Info>),
    /// Created notebook/debugger identity.
    Runtime(Box<rode_runtime_core::service::Info>),
    /// Snapshot of discoverable running terminals.
    Sessions(Vec<Info>),
    /// Live native agent controllers.
    Agents(Vec<rode_agent_core::service::Info>),
    /// Created native agent metadata.
    Agent(rode_agent_core::service::Info),
    /// Metadata for the created or updated terminal.
    Session(Info),
    /// Successful operation without a result payload.
    Ok,
    /// User-visible service error; use [`Self::checked`] to propagate it.
    Error(String),
}
impl Response {
    /// Convert a service error reply into an `anyhow` error.
    pub fn checked(self) -> Result<Self> {
        if let Self::Error(error) = &self {
            bail!("{error}");
        }
        Ok(self)
    }
}
/// Executable identity used to start this client's matching service generation.
#[derive(Clone)]
pub struct Launcher {
    /// Native helper executable path, which may refer to the running image after an upgrade.
    pub executable: PathBuf,
    /// Arguments inserted before the service subcommand, such as `--session-service`.
    pub prefix: Vec<String>,
}

/// Resolve the host runtime directory, honoring `RODE_SESSION_DIR`; ownership validation occurs
/// on use.
pub fn directory() -> PathBuf {
    std::env::var_os("RODE_SESSION_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!("/tmp/rode-session-v{VERSION}-{}", unsafe {
                libc::geteuid()
            }))
        })
}
fn private_directory() -> Result<PathBuf> {
    let path = directory();
    private_directory_at(&path)?;
    Ok(path)
}
fn private_directory_at(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute(),
        "Terminal runtime directory must be absolute"
    );
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
        "Session directory must be an owner-only directory: {}",
        path.display()
    );
    Ok(())
}
fn socket() -> PathBuf {
    directory().join("socket")
}
fn frame<W: Write>(out: &mut W, kind: u8, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() < MAX_FRAME, "Session message exceeds 4 MiB");
    out.write_all(&((bytes.len() + 1) as u32).to_be_bytes())?;
    out.write_all(&[kind])?;
    out.write_all(bytes)?;
    out.flush()?;
    Ok(())
}
fn read_frame<R: Read>(input: &mut R) -> Result<(u8, Vec<u8>)> {
    let mut size = [0; 4];
    input.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    ensure!(size > 0 && size <= MAX_FRAME, "Invalid session frame size");
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes)?;
    let kind = bytes.remove(0);
    Ok((kind, bytes))
}
fn send_json<W: Write, T: Serialize>(out: &mut W, value: &T) -> Result<()> {
    frame(out, b'J', &serde_json::to_vec(value)?)
}
fn recv_json<R: Read, T: serde::de::DeserializeOwned>(input: &mut R) -> Result<T> {
    let (kind, bytes) = read_frame(input)?;
    ensure!(kind == b'J', "Expected a session control message");
    Ok(serde_json::from_slice(&bytes)?)
}
fn connect_at(dir: &Path) -> Result<UnixStream> {
    let meta = fs::symlink_metadata(dir)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
        "Unsafe session directory"
    );
    let stream = UnixStream::connect(dir.join("socket"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    Ok(stream)
}
/// Perform a blocking control request, starting a matching service when needed. Service
/// creation is serialized; existing legacy sessions remain separately discoverable.
pub fn request(request: &Request, launcher: &Launcher) -> Result<Response> {
    let until = Instant::now()
        + if matches!(request, Request::List | Request::Agents | Request::Runtimes) {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(10)
        };
    loop {
        let result = request_routed(request, launcher);
        let retry = matches!(
            request,
            Request::Create { .. }
                | Request::CreateAgent { .. }
                | Request::CreateRuntime { .. }
                | Request::List
                | Request::Agents
                | Request::Runtimes
        ) && result.as_ref().err().is_some_and(|error| {
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
        // Retry idempotent Create and read-only List across that transition.
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn service_directories() -> [PathBuf; 7] {
    let primary = directory();
    [
        primary.clone(),
        primary.join("protocol-2"),
        primary.join("protocol-3"),
        primary.join("protocol-6"),
        primary.join("protocol-7"),
        primary.join("protocol-8"),
        primary.join(format!("protocol-{PROTOCOL}")),
    ]
}
fn agents_at(dir: &Path, launcher: &Launcher) -> Result<Vec<rode_agent_core::service::Info>> {
    match request_once(&Request::Agents, launcher, dir) {
        Ok(Response::Agents(infos)) => Ok(infos),
        Err(error)
            if service_offline(&error)
                || error.to_string().contains("unknown variant `agents`") =>
        {
            Ok(vec![])
        }
        Err(error) => Err(error),
        _ => bail!("Unexpected background-agent listing"),
    }
}
fn runtimes_at(dir: &Path, launcher: &Launcher) -> Result<Vec<rode_runtime_core::service::Info>> {
    match request_once(&Request::Runtimes, launcher, dir) {
        Ok(Response::Runtimes(infos)) => Ok(infos),
        Err(error)
            if service_offline(&error)
                || error.to_string().contains("unknown variant `runtimes`") =>
        {
            Ok(vec![])
        }
        Err(error) => Err(error),
        _ => bail!("Unexpected runtime listing"),
    }
}
fn creation_directory(launcher: &Launcher) -> Result<PathBuf> {
    let primary = directory();
    let compatible = primary.join(format!("protocol-{PROTOCOL}"));
    if compatible.join("socket").exists() {
        return Ok(compatible);
    }
    if primary.join("socket").exists() {
        for probe in [Request::SupportsRuntimes, Request::SupportsAutomaticQueue] {
            match request_once(&probe, launcher, &primary) {
                Ok(Response::Ok) => {}
                Err(error) if service_offline(&error) => break,
                Err(error)
                    if error
                        .to_string()
                        .contains("unknown variant `supports_runtimes`")
                        || error
                            .to_string()
                            .contains("unknown variant `supports_automatic_queue`") =>
                {
                    return Ok(compatible);
                }
                Err(error) => return Err(error),
                _ => bail!("Unexpected native service capability response"),
            }
        }
    }
    Ok(primary)
}
fn list_at(dir: &Path, launcher: &Launcher) -> Result<Vec<Info>> {
    let response = match request_once(&Request::List, launcher, dir) {
        // A crashed service may leave a socket behind. It cannot supply
        // sessions, but must not hide a working compatible service. Never
        // unlink its socket/lock here; startup uses the singleton flock.
        Err(error) if service_offline(&error) => return Ok(vec![]),
        result => result?,
    };
    let Response::Sessions(infos) = response else {
        bail!("Unexpected terminal service listing");
    };
    Ok(infos)
}
fn locate(id: &str, launcher: &Launcher) -> Result<Option<(PathBuf, Info)>> {
    for dir in service_directories() {
        if let Some(info) = list_at(&dir, launcher)?
            .into_iter()
            .find(|info| info.id == id)
        {
            return Ok(Some((dir, info)));
        }
    }
    Ok(None)
}
fn service_offline(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            )
        })
    })
}
fn request_routed(request: &Request, launcher: &Launcher) -> Result<Response> {
    match request {
        Request::Runtimes => {
            let mut infos = vec![];
            for dir in service_directories() {
                infos.extend(runtimes_at(&dir, launcher)?);
            }
            infos.sort_by_key(|i| i.created);
            Ok(Response::Runtimes(infos))
        }
        Request::CreateRuntime { spec } => {
            for dir in service_directories() {
                if runtimes_at(&dir, launcher)?
                    .iter()
                    .any(|info| info.id == spec.id)
                {
                    return request_once(request, launcher, &dir);
                }
            }
            request_once(request, launcher, &creation_directory(launcher)?)
        }
        Request::AttachRuntime { id, .. } | Request::EndRuntime { id, .. } => {
            for dir in service_directories() {
                if runtimes_at(&dir, launcher)?
                    .iter()
                    .any(|info| info.id == *id)
                {
                    return request_once(request, launcher, &dir);
                }
            }
            ensure!(
                matches!(request, Request::EndRuntime { .. }),
                "Runtime is no longer available; refresh the running list"
            );
            Ok(Response::Ok)
        }
        Request::List => {
            let mut infos = vec![];
            for dir in service_directories() {
                infos.extend(list_at(&dir, launcher)?);
            }
            infos.retain(|info| info.alive);
            infos.sort_by_key(|info| info.created);
            Ok(Response::Sessions(infos))
        }
        Request::Agents => {
            let mut infos = vec![];
            for dir in service_directories() {
                infos.extend(agents_at(&dir, launcher)?);
            }
            infos.retain(|info| info.alive);
            infos.sort_by_key(|info| info.created);
            Ok(Response::Agents(infos))
        }
        Request::Create { id, .. } => {
            for dir in service_directories() {
                if list_at(&dir, launcher)?.iter().any(|info| info.id == *id) {
                    return request_once(request, launcher, &dir);
                }
            }
            request_once(request, launcher, &creation_directory(launcher)?)
        }
        Request::CreateAgent { spec } => {
            for dir in service_directories() {
                if agents_at(&dir, launcher)?
                    .iter()
                    .any(|info| info.id == spec.id)
                {
                    return request_once(request, launcher, &dir);
                }
            }
            request_once(request, launcher, &creation_directory(launcher)?)
        }
        Request::AttachAgent { id, .. } | Request::EndAgent { id, .. } => {
            for dir in service_directories() {
                if agents_at(&dir, launcher)?.iter().any(|info| info.id == *id) {
                    return request_once(request, launcher, &dir);
                }
            }
            ensure!(
                matches!(request, Request::EndAgent { .. }),
                "Background agent is no longer available; refresh the list"
            );
            Ok(Response::Ok)
        }
        Request::Shutdown => {
            ensure!(
                matches!(request_routed(&Request::Runtimes,launcher)?,Response::Runtimes(infos) if infos.is_empty()),
                "End notebook/debugger runtimes before stopping the service"
            );
            ensure!(
                matches!(request_routed(&Request::List, launcher)?, Response::Sessions(infos) if infos.is_empty()),
                "End running terminals before stopping the service"
            );
            ensure!(
                matches!(request_routed(&Request::Agents, launcher)?, Response::Agents(infos) if infos.is_empty()),
                "End running agents before stopping the service"
            );
            for dir in service_directories() {
                if dir.join("socket").exists() {
                    match request_once(request, launcher, &dir) {
                        Err(error) if service_offline(&error) => {}
                        result => {
                            result?;
                        }
                    }
                }
            }
            Ok(Response::Ok)
        }
        Request::SupportsPersistence
        | Request::SupportsAllocations
        | Request::SupportsAgents
        | Request::SupportsHistoryDeletion
        | Request::SupportsRuntimes
        | Request::SupportsAutomaticQueue => request_once(request, launcher, &directory()),
        Request::Attach { id, .. }
        | Request::Rename { id, .. }
        | Request::SetPersistence { id, .. }
        | Request::End { id, .. } => {
            let Some((dir, info)) = locate(id, launcher)? else {
                ensure!(
                    matches!(request, Request::End { .. }),
                    "Terminal session is no longer available; refresh the session list"
                );
                return Ok(Response::Ok);
            };
            ensure!(
                !matches!(request, Request::SetPersistence { .. }) || info.persistence_supported,
                "{LEGACY_PERSISTENCE_HELP}"
            );
            request_once(request, launcher, &dir)
        }
    }
}
fn request_once(request: &Request, launcher: &Launcher, dir: &Path) -> Result<Response> {
    let mut stream = match connect_at(dir) {
        Ok(stream) => stream,
        Err(error) => {
            if matches!(request, Request::List | Request::Agents | Request::Runtimes) {
                if !dir.join("socket").exists() {
                    return Ok(if matches!(request, Request::Runtimes) {
                        Response::Runtimes(vec![])
                    } else if matches!(request, Request::Agents) {
                        Response::Agents(vec![])
                    } else {
                        Response::Sessions(vec![])
                    });
                }
                return Err(error)
                    .context("Session service is unavailable; existing jobs were not touched");
            }
            if !matches!(
                request,
                Request::Create { .. }
                    | Request::CreateAgent { .. }
                    | Request::CreateRuntime { .. }
            ) {
                return Err(error)
                    .context("Session service is offline; no session was created or ended.");
            }
            // A compatibility service has its own socket and stable lock.
            // Its parent is the already validated primary runtime directory.
            if dir != directory() {
                private_directory()?;
            }
            private_directory_at(dir)?;
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(dir.join("service.log"))?;
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
            let mut child = command
                .spawn()
                .context("Cannot start native terminal service")?;
            // Reap our daemon if it exits while this client remains alive.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(stream) = connect_at(dir) {
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
fn valid_id(id: &str) -> bool {
    id.len() == 34 && id.starts_with("s-") && id[2..].bytes().all(|b| b.is_ascii_hexdigit())
}
fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control)
}
/// Generate a random opaque terminal identifier from the host's entropy source.
pub fn new_id() -> Result<String> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!(
        "s-{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}

struct Viewer {
    send: mpsc::SyncSender<(u8, Vec<u8>)>,
    rows: u16,
    cols: u16,
    close: UnixStream,
}
struct State {
    info: Info,
    parser: vt100::Parser,
    viewers: HashMap<u64, Viewer>,
    created: Instant,
}
struct Session {
    state: Mutex<State>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    input: Mutex<Option<mpsc::SyncSender<Vec<u8>>>>,
    stopped: Arc<AtomicBool>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    stop: Mutex<UnixStream>,
}
impl Session {
    fn info(&self) -> Info {
        let state = self.state.lock().unwrap();
        let mut info = state.info.clone();
        info.clients = state.viewers.len();
        info
    }
    fn check(&self, policy: &str) -> Result<()> {
        ensure!(
            self.state.lock().unwrap().info.policy == policy,
            "Session belongs to a different project/sandbox policy. Open its original project and policy before attaching or changing it."
        );
        Ok(())
    }
    fn broadcast(state: &mut State, kind: u8, data: &[u8]) {
        state.viewers.retain(|_, viewer| {
            if viewer.send.try_send((kind, data.to_vec())).is_err() {
                let _ = viewer.close.shutdown(std::net::Shutdown::Both);
                false
            } else {
                true
            }
        });
    }
    fn close_pty(&self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.stop.lock().unwrap().write_all(&[1]);
        self.input.lock().unwrap().take();
        self.master.lock().unwrap().take();
    }
    fn resize(&self) {
        let mut state = self.state.lock().unwrap();
        if state.viewers.is_empty() {
            return;
        }
        let rows = state
            .viewers
            .values()
            .map(|v| v.rows)
            .min()
            .unwrap()
            .clamp(2, 200);
        let cols = state
            .viewers
            .values()
            .map(|v| v.cols)
            .min()
            .unwrap()
            .clamp(10, 400);
        if state.parser.screen().size() != (rows, cols) {
            state.parser.screen_mut().set_size(rows, cols);
            let mut size = rows.to_be_bytes().to_vec();
            size.extend(cols.to_be_bytes());
            Self::broadcast(&mut state, b'S', &size);
            let snapshot = state.parser.screen().state_formatted();
            for chunk in snapshot.chunks(8192) {
                Self::broadcast(&mut state, b'O', chunk);
            }
            if let Some(master) = self.master.lock().unwrap().as_ref() {
                let _ = master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
        }
    }
}
struct Server {
    runtimes: runtimes::Store,
    agents: agents::Store,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    last_exit: Mutex<Instant>,
    stop: AtomicBool,
    next_viewer: AtomicU64,
}
impl Server {
    fn stop_if_idle(&self, idle_since: &mut Option<Instant>) {
        // Use the same lock as Create's stop check and insertion. A terminal
        // starting concurrently must either keep this service alive or retry
        // against the next service, never be accepted while it is exiting.
        let sessions = self.sessions.lock().unwrap();
        if !sessions.is_empty() || self.agents.live() || self.runtimes.live() {
            *idle_since = None;
            return;
        }
        let since = idle_since.get_or_insert_with(Instant::now);
        *since = (*since).max(*self.last_exit.lock().unwrap());
        if since.elapsed() >= IDLE_TIMEOUT {
            self.stop.store(true, Ordering::Release);
        }
    }
    fn end_unattached(&self, id: &str, startup_timeout: bool) {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(session) = sessions.get(id).cloned() else {
            return;
        };
        {
            let state = session.state.lock().unwrap();
            if state.info.persistent
                || !state.viewers.is_empty()
                || (startup_timeout && state.created.elapsed() < Duration::from_secs(30))
            {
                return;
            }
            // Prevent a racing Attach from entering a session being closed.
            session.stopped.store(true, Ordering::Release);
        }
        sessions.remove(id);
        if sessions.is_empty() && !self.agents.live() && !self.runtimes.live() {
            self.stop.store(true, Ordering::Release);
        }
        drop(sessions);
        let _ = session.killer.lock().unwrap().kill();
        session.close_pty();
    }
    fn find(&self, id: &str, policy: &str) -> Result<Arc<Session>> {
        ensure!(valid_id(id), "Invalid session ID");
        let session = self
            .sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("Terminal session no longer exists")?;
        session.check(policy)?;
        Ok(session)
    }
    fn create(self: &Arc<Self>, request: Request) -> Result<Info> {
        let Request::Create {
            id,
            name,
            root,
            policy,
            mode,
            argv,
            initial_input,
            env,
            persistent,
            allocation,
        } = request
        else {
            unreachable!()
        };
        ensure!(
            !self.stop.load(Ordering::Acquire),
            "Session service is stopping"
        );
        ensure!(
            valid_id(&id) && valid_name(&name),
            "Invalid terminal ID/name"
        );
        ensure!(
            allocation
                .as_ref()
                .is_none_or(|name| name == &format!("rode-{id}")),
            "Allocation name must match its terminal ID"
        );
        ensure!(
            Path::new(&root).is_absolute() && root.len() <= 4096,
            "Invalid terminal root"
        );
        ensure!(
            policy.len() == 64 && policy.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid policy fingerprint"
        );
        ensure!(
            !argv.is_empty()
                && argv.len() <= 256
                && argv.iter().map(String::len).sum::<usize>() <= 64 * 1024,
            "Invalid terminal command size"
        );
        ensure!(
            env.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() <= 64 * 1024,
            "Terminal environment exceeds 64 KiB"
        );
        ensure!(
            initial_input.len() <= 64 * 1024,
            "Terminal startup input exceeds 64 KiB"
        );
        let mut sessions = self.sessions.lock().unwrap();
        ensure!(
            !self.stop.load(Ordering::Acquire),
            "Session service is stopping"
        );
        // Choose automatic names under the singleton lock so simultaneous
        // windows and reattachments cannot create indistinguishable tabs.
        let mut name = name;
        if let Some(prefix) = ["Terminal", "Slurm"].into_iter().find(|prefix| {
            name.strip_prefix(&format!("{prefix} "))
                .and_then(|n| n.parse::<u64>().ok())
                .is_some()
        }) {
            let mut number = 1u64;
            while sessions
                .values()
                .any(|session| session.info().name == format!("{prefix} {number}"))
            {
                number += 1;
            }
            name = format!("{prefix} {number}");
        }
        if let Some(existing) = sessions.get(&id) {
            existing.check(&policy)?;
            let info = existing.info();
            ensure!(
                info.root == root,
                "Session ID belongs to a different project"
            );
            return Ok(info);
        }
        ensure!(
            sessions.len() < MAX_SESSIONS,
            "Terminal limit reached (32 live sessions). End an unused terminal first."
        );
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let server = self.clone();
        // The spawning thread must stay alive while bwrap runs: its
        // --die-with-parent signal follows the parent thread on Linux.
        std::thread::spawn(move || {
            let setup = (|| -> Result<_> {
                let pair = portable_pty::native_pty_system().openpty(PtySize {
                    rows: 24,
                    cols: 100,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;
                let mut command = CommandBuilder::new(&argv[0]);
                command.args(&argv[1..]);
                command.cwd(&root);
                // Each caller supplies a complete environment snapshot. Never
                // leak the first window's environment into later sessions.
                command.env_clear();
                for (k, v) in env {
                    command.env(k, v);
                }
                command.env("TERM", "xterm-256color");
                command.env("COLORTERM", "truecolor");
                let child = pair.slave.spawn_command(command)?;
                drop(pair.slave);
                let fd = pair
                    .master
                    .as_raw_fd()
                    .context("Native terminal has no file descriptor")?;
                let fd = unsafe { libc::dup(fd) };
                ensure!(fd >= 0, "Cannot duplicate terminal descriptor");
                let reader = unsafe { File::from_raw_fd(fd) };
                let write_fd = unsafe { libc::dup(pair.master.as_raw_fd().unwrap()) };
                ensure!(write_fd >= 0, "Cannot duplicate terminal input");
                let mut input_file = unsafe { File::from_raw_fd(write_fd) };
                let flags = unsafe { libc::fcntl(write_fd, libc::F_GETFL) };
                ensure!(
                    unsafe { libc::fcntl(write_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
                    "Cannot configure terminal input"
                );
                let (input, input_rx) = mpsc::sync_channel::<Vec<u8>>(128);
                let stopped = Arc::new(AtomicBool::new(false));
                let stop_input = stopped.clone();
                std::thread::spawn(move || {
                    while let Ok(bytes) = input_rx.recv() {
                        let mut offset = 0;
                        while offset < bytes.len() && !stop_input.load(Ordering::Acquire) {
                            match input_file.write(&bytes[offset..]) {
                                Ok(0) => return,
                                Ok(n) => offset += n,
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                    let mut fd = libc::pollfd {
                                        fd: write_fd,
                                        events: libc::POLLOUT,
                                        revents: 0,
                                    };
                                    unsafe {
                                        libc::poll(&mut fd, 1, 100);
                                    }
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                                Err(_) => return,
                            }
                        }
                    }
                });
                if !initial_input.is_empty() {
                    // Queued before publishing the session, so this runs after
                    // login startup and before any attached client's commands.
                    input.try_send(initial_input.into_bytes())?;
                }
                let (stop, stop_read) = UnixStream::pair()?;
                let info = Info {
                    id,
                    name,
                    root,
                    policy,
                    mode,
                    alive: true,
                    clients: 0,
                    created: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                    exit_code: None,
                    persistent,
                    persistence_supported: true,
                    allocation,
                };
                let session = Arc::new(Session {
                    state: Mutex::new(State {
                        info,
                        parser: vt100::Parser::new(24, 100, 1000),
                        viewers: HashMap::new(),
                        created: Instant::now(),
                    }),
                    master: Mutex::new(Some(pair.master)),
                    input: Mutex::new(Some(input)),
                    stopped,
                    killer: Mutex::new(child.clone_killer()),
                    stop: Mutex::new(stop),
                });
                Ok((session, reader, stop_read, child))
            })();
            match setup {
                Err(error) => {
                    let _ = ready_tx.send(Err(format!("{error:#}")));
                }
                Ok((session, mut reader, stop_read, mut child)) => {
                    let read_session = session.clone();
                    let (drained_tx, drained_rx) = mpsc::sync_channel(1);
                    std::thread::spawn(move || {
                        let mut data = [0; 8192];
                        loop {
                            let mut fds = [
                                libc::pollfd {
                                    fd: reader.as_raw_fd(),
                                    events: libc::POLLIN,
                                    revents: 0,
                                },
                                libc::pollfd {
                                    fd: stop_read.as_raw_fd(),
                                    events: libc::POLLIN,
                                    revents: 0,
                                },
                            ];
                            let polled = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
                            if polled < 0 {
                                if std::io::Error::last_os_error().kind()
                                    == std::io::ErrorKind::Interrupted
                                {
                                    continue;
                                }
                                break;
                            }
                            if fds[1].revents != 0 {
                                break;
                            }
                            let n = match reader.read(&mut data) {
                                Err(error)
                                    if error.kind() == std::io::ErrorKind::WouldBlock
                                        || error.kind() == std::io::ErrorKind::Interrupted =>
                                {
                                    continue;
                                }
                                Ok(0) | Err(_) => break,
                                Ok(n) => n,
                            };
                            let mut state = read_session.state.lock().unwrap();
                            state.parser.process(&data[..n]);
                            Session::broadcast(&mut state, b'O', &data[..n]);
                        }
                        let _ = drained_tx.send(());
                    });
                    let _ = ready_tx.send(Ok(session.clone()));
                    let status = child.wait();
                    // Drain final output before publishing exit. Descendants may
                    // retain the slave, so bound this wait and keep the spawning
                    // supervisor thread alive for bwrap PDEATHSIG semantics.
                    let _ = drained_rx.recv_timeout(Duration::from_millis(200));
                    session.close_pty();
                    let mut sessions = server.sessions.lock().unwrap();
                    let mut state = session.state.lock().unwrap();
                    state.info.alive = false;
                    state.info.exit_code = status.ok().map(|s| s.exit_code());
                    Session::broadcast(&mut state, b'X', b"\r\n[terminal session ended]\r\n");
                    *server.last_exit.lock().unwrap() = Instant::now();
                    // Attached views own their final output. The daemon only
                    // retains live sessions, releasing this slot even while
                    // another persistent shell keeps the service running.
                    if sessions
                        .get(&state.info.id)
                        .is_some_and(|current| Arc::ptr_eq(current, &session))
                    {
                        sessions.remove(&state.info.id);
                    }
                }
            }
        });
        let session = ready_rx
            .recv_timeout(Duration::from_secs(10))
            .context("Terminal startup timed out")?
            .map_err(anyhow::Error::msg)?;
        let info = session.info();
        sessions.insert(info.id.clone(), session);
        Ok(info)
    }
}
fn snapshot(state: &mut State) -> Vec<u8> {
    let screen = state.parser.screen_mut();
    screen.set_scrollback(usize::MAX);
    let mut remaining = screen.scrollback();
    let rows = screen.size().0 as usize;
    let mut history = VecDeque::new();
    let mut bytes = 0;
    while remaining > 0 {
        screen.set_scrollback(remaining);
        for row in screen
            .rows_formatted(0, screen.size().1)
            .take(remaining.min(rows))
        {
            let mut row = row;
            row.extend(b"\r\n");
            bytes += row.len();
            history.push_back(row);
            while bytes > 512 * 1024 {
                if let Some(old) = history.pop_front() {
                    bytes -= old.len();
                } else {
                    break;
                }
            }
        }
        remaining = remaining.saturating_sub(rows);
    }
    screen.set_scrollback(0);
    let mut data = b"\x1b[0m\x1b[2J\x1b[H".to_vec();
    for row in history {
        data.extend(row);
    }
    data.extend(screen.state_formatted());
    data
}
fn handle(server: &Arc<Server>, mut stream: UnixStream) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        ensure!(
            unsafe {
                libc::getsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PEERCRED,
                    (&mut cred as *mut libc::ucred).cast(),
                    &mut len,
                )
            } == 0
                && cred.uid == unsafe { libc::geteuid() },
            "Session client must have the same UID"
        );
        // Workspace tools must not gain host execution by reaching an exposed
        // runtime socket. The broker/attach clients run in the host namespace.
        ensure!(
            fs::read_link(format!("/proc/{}/ns/mnt", cred.pid))?
                == fs::read_link("/proc/self/ns/mnt")?,
            "Terminal control must originate outside the project sandbox"
        );
    }
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let request: Request = recv_json(&mut stream)?;
    if let Request::AttachRuntime { id, policy } = &request {
        return server.runtimes.attach(stream, id, policy);
    }
    if let Request::AttachAgent { id, policy } = &request {
        return server.agents.attach(stream, id, policy);
    }
    if let Request::Attach { id, policy } = &request {
        let session = server.find(id, policy)?;
        let viewer = server.next_viewer.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(128);
        {
            let mut state = session.state.lock().unwrap();
            ensure!(
                !session.stopped.load(Ordering::Acquire) || !state.info.alive,
                "Terminal session is closing"
            );
            ensure!(
                state.viewers.len() < MAX_CLIENTS,
                "At most 8 clients can share a terminal"
            );
            send_json(&mut stream, &Response::Session(state.info.clone()))?;
            frame(&mut stream, b'P', &[u8::from(state.info.persistent)])?;
            let (rows, cols) = state.parser.screen().size();
            let mut size = rows.to_be_bytes().to_vec();
            size.extend(cols.to_be_bytes());
            frame(&mut stream, b'S', &size)?;
            let replay = snapshot(&mut state);
            for bytes in replay.chunks(8192) {
                frame(&mut stream, b'O', bytes)?;
            }
            if !state.info.alive {
                frame(&mut stream, b'X', b"\r\n[terminal session ended]\r\n")?;
                return Ok(());
            }
            let (rows, cols) = state.parser.screen().size();
            state.viewers.insert(
                viewer,
                Viewer {
                    send: tx,
                    rows,
                    cols,
                    close: stream.try_clone()?,
                },
            );
        }
        let mut writer = stream.try_clone()?;
        std::thread::spawn(move || {
            while let Ok((kind, bytes)) = rx.recv() {
                if frame(&mut writer, kind, &bytes).is_err() {
                    break;
                }
            }
            let _ = writer.shutdown(std::net::Shutdown::Both);
        });
        stream.set_read_timeout(None)?;
        while let Ok((kind, bytes)) = read_frame(&mut stream) {
            match kind {
                b'I' => {
                    let input = session.input.lock().unwrap();
                    if let Some(input) = input.as_ref()
                        && input.try_send(bytes).is_err()
                    {
                        break;
                    }
                }
                b'R' if bytes.len() == 4 => {
                    let rows = u16::from_be_bytes([bytes[0], bytes[1]]).clamp(2, 200);
                    let cols = u16::from_be_bytes([bytes[2], bytes[3]]).clamp(10, 400);
                    if let Some(v) = session.state.lock().unwrap().viewers.get_mut(&viewer) {
                        v.rows = rows;
                        v.cols = cols;
                    }
                    session.resize();
                }
                b'D' => break,
                _ => break,
            }
        }
        session.state.lock().unwrap().viewers.remove(&viewer);
        session.resize();
        server.end_unattached(id, false);
        return Ok(());
    }
    let response = (|| -> Result<Response> {
        Ok(match request {
            Request::SupportsPersistence
            | Request::SupportsAllocations
            | Request::SupportsAgents
            | Request::SupportsHistoryDeletion
            | Request::SupportsRuntimes
            | Request::SupportsAutomaticQueue => Response::Ok,
            Request::Runtimes => Response::Runtimes(server.runtimes.list()),
            Request::CreateRuntime { spec } => {
                let _sessions = server.sessions.lock().unwrap();
                ensure!(
                    !server.stop.load(Ordering::Acquire),
                    "Session service is stopping"
                );
                Response::Runtime(Box::new(server.runtimes.create(*spec)?))
            }
            Request::EndRuntime { id, policy } => {
                server.runtimes.end(&id, &policy)?;
                Response::Ok
            }
            Request::AttachRuntime { .. } => unreachable!(),
            Request::Agents => Response::Agents(server.agents.list()),
            Request::CreateAgent { spec } => {
                let _creation = server.sessions.lock().unwrap();
                ensure!(
                    !server.stop.load(Ordering::Acquire),
                    "Session service is stopping"
                );
                Response::Agent(server.agents.create(*spec)?)
            }
            Request::EndAgent { id, policy } => {
                server.agents.end(&id, &policy)?;
                Response::Ok
            }
            Request::AttachAgent { .. } => unreachable!(),
            Request::List => {
                let sessions = server.sessions.lock().unwrap();
                let mut infos: Vec<_> = sessions.values().map(|session| session.info()).collect();
                infos.sort_by_key(|info| info.created);
                Response::Sessions(infos)
            }
            request @ Request::Create { .. } => Response::Session(server.create(request)?),
            Request::Rename { id, policy, name } => {
                ensure!(
                    valid_name(&name),
                    "Terminal name must contain 1–64 printable characters"
                );
                let session = server.find(&id, &policy)?;
                session.state.lock().unwrap().info.name = name;
                Response::Session(session.info())
            }
            Request::SetPersistence {
                id,
                policy,
                persistent,
            } => {
                let session = server.find(&id, &policy)?;
                {
                    let mut state = session.state.lock().unwrap();
                    ensure!(
                        !session.stopped.load(Ordering::Acquire),
                        "Terminal session has ended"
                    );
                    state.info.persistent = persistent;
                    Session::broadcast(&mut state, b'P', &[u8::from(persistent)]);
                }
                let info = session.info();
                server.end_unattached(&id, false);
                Response::Session(info)
            }
            Request::End { id, policy } => {
                let session = {
                    let sessions = server.sessions.lock().unwrap();
                    let Some(session) = sessions.get(&id).cloned() else {
                        return Ok(Response::Ok);
                    };
                    session.check(&policy)?;
                    session
                };
                if session.info().alive {
                    let _ = session.killer.lock().unwrap().kill();
                }
                session.close_pty();
                let mut sessions = server.sessions.lock().unwrap();
                if sessions
                    .get(&id)
                    .is_some_and(|current| Arc::ptr_eq(current, &session))
                {
                    sessions.remove(&id);
                }
                if sessions.is_empty() && !server.agents.live() && !server.runtimes.live() {
                    server.stop.store(true, Ordering::Release);
                }
                Response::Ok
            }
            Request::Shutdown => {
                ensure!(
                    !server.runtimes.live(),
                    "End notebook/debugger runtimes before stopping the service"
                );
                ensure!(
                    !server.agents.live(),
                    "End running agents before stopping the service"
                );
                ensure!(
                    server
                        .sessions
                        .lock()
                        .unwrap()
                        .values()
                        .all(|session| !session.info().alive),
                    "End running terminals before stopping the service"
                );
                server.stop.store(true, Ordering::Release);
                Response::Ok
            }
            Request::Attach { .. } => unreachable!(),
        })
    })();
    let response = response.unwrap_or_else(|error| Response::Error(format!("{error:#}")));
    send_json(&mut stream, &response)
}

/// Run the headless owner-only Unix-socket server until stopped or idle with no live shells.
/// The service owns PTYs independently of GUI clients.
pub fn serve() -> Result<()> {
    let dir = private_directory()?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(dir.join("lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(());
    }
    if socket().exists() {
        let meta = fs::symlink_metadata(socket())?;
        ensure!(
            !meta.file_type().is_symlink() && meta.uid() == unsafe { libc::geteuid() },
            "Unsafe terminal socket"
        );
        fs::remove_file(socket())?;
    }
    let listener = UnixListener::bind(socket())?;
    fs::set_permissions(socket(), fs::Permissions::from_mode(0o600))?;
    let server = Arc::new(Server {
        agents: agents::Store::default(),
        runtimes: runtimes::Store::default(),
        sessions: Mutex::new(HashMap::new()),
        last_exit: Mutex::new(Instant::now()),
        stop: AtomicBool::new(false),
        next_viewer: AtomicU64::new(1),
    });
    let mut idle_since = Some(Instant::now());
    while !server.stop.load(Ordering::Acquire) {
        let ids: Vec<_> = server.sessions.lock().unwrap().keys().cloned().collect();
        for id in ids {
            server.end_unattached(&id, true);
        }
        server.stop_if_idle(&mut idle_since);
        if server.stop.load(Ordering::Acquire) {
            break;
        }
        let mut fd = libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut fd, 1, 1000) } <= 0 {
            continue;
        }
        let (stream, _) = listener.accept()?;
        let server = server.clone();
        std::thread::spawn(move || {
            if let Err(error) = handle(&server, stream.try_clone().unwrap()) {
                let mut stream = stream;
                let _ = send_json(&mut stream, &Response::Error(format!("{error:#}")));
            }
        });
    }
    fs::remove_file(socket())?;
    drop(lock);
    Ok(())
}

static RESIZED: AtomicBool = AtomicBool::new(true);
extern "C" fn resized(_: libc::c_int) {
    RESIZED.store(true, Ordering::Release);
}
/// Attach stdin/stdout to a session after policy verification. Framed mode multiplexes
/// input/resize/output; dropping a view obeys the session's persistence policy.
pub fn attach(id: &str, policy: &str, framed: bool) -> Result<()> {
    let launcher = Launcher {
        executable: current_executable()?,
        prefix: vec![],
    };
    let (dir, _) = locate(id, &launcher)?
        .context("Terminal session is no longer available; refresh the session list")?;
    let mut stream = connect_at(&dir)?;
    send_json(
        &mut stream,
        &Request::Attach {
            id: id.into(),
            policy: policy.into(),
        },
    )?;
    recv_json::<_, Response>(&mut stream)?.checked()?;
    stream.set_read_timeout(None)?;
    struct Raw(Option<libc::termios>);
    impl Drop for Raw {
        fn drop(&mut self) {
            if let Some(termios) = self.0 {
                unsafe {
                    libc::tcsetattr(0, libc::TCSANOW, &termios);
                }
            }
        }
    }
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    let termios = if unsafe { libc::tcgetattr(0, &mut old) } == 0 {
        let mut raw = old;
        unsafe {
            libc::cfmakeraw(&mut raw);
            libc::tcsetattr(0, libc::TCSANOW, &raw);
        }
        Some(old)
    } else {
        None
    };
    let _raw = Raw(termios);
    unsafe {
        libc::signal(libc::SIGWINCH, resized as *const () as libc::sighandler_t);
    }
    let input = Arc::new(Mutex::new(stream.try_clone()?));
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    std::thread::spawn(move || {
        let mut bytes = [0; 8192];
        while !stop.load(Ordering::Acquire) {
            if RESIZED.swap(false, Ordering::AcqRel) {
                let mut size: libc::winsize = unsafe { std::mem::zeroed() };
                if unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut size) } == 0 {
                    let mut data = size.ws_row.to_be_bytes().to_vec();
                    data.extend(size.ws_col.to_be_bytes());
                    if frame(&mut *input.lock().unwrap(), b'R', &data).is_err() {
                        break;
                    }
                }
            }
            let mut fd = libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut fd, 1, 100) } <= 0 {
                continue;
            }
            let n = match std::io::stdin().read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if frame(&mut *input.lock().unwrap(), b'I', &bytes[..n]).is_err() {
                break;
            }
        }
        let _ = input.lock().unwrap().shutdown(std::net::Shutdown::Both);
    });
    let result = (|| -> Result<()> {
        let mut output = std::io::stdout().lock();
        loop {
            let (kind, data) = read_frame(&mut stream)?;
            if framed {
                frame(&mut output, kind, &data)?;
            } else if kind == b'O' || kind == b'X' {
                output.write_all(&data)?;
                output.flush()?;
            }
            if kind == b'X' {
                break;
            }
        }
        Ok(())
    })();
    stopped.store(true, Ordering::Release);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    result
}

/// Entry point shared by the standalone headless helper and the GUI binary's
/// local fallback. No windowing code is linked into rode-sessiond.
pub fn cli(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("protocol") if args.len() == 1 => {
            println!("{PROTOCOL}");
            Ok(())
        }
        Some("serve") => serve(),
        Some("request") => {
            let mut bytes = vec![];
            std::io::stdin()
                .take(MAX_FRAME as u64 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() < MAX_FRAME, "Session request exceeds 4 MiB");
            let request = serde_json::from_slice(&bytes)?;
            let launcher = Launcher {
                executable: current_executable()?,
                prefix: vec![],
            };
            let response = request_result(request, &launcher);
            serde_json::to_writer(std::io::stdout().lock(), &response)?;
            Ok(())
        }
        Some("attach-runtime") if args.len() == 3 => runtimes::bridge(&args[1], &args[2]),
        Some("attach-agent") if args.len() == 3 => agents::bridge(&args[1], &args[2]),
        Some("attach") if args.len() == 3 => attach(&args[1], &args[2], false),
        Some("attach") if args.len() == 4 && args[1] == "--framed" => {
            attach(&args[2], &args[3], true)
        }
        _ => bail!("Usage: rode-sessiond serve | request | attach SESSION POLICY"),
    }
}
fn request_result(request: Request, launcher: &Launcher) -> Response {
    self::request(&request, launcher).unwrap_or_else(|error| Response::Error(format!("{error:#}")))
}

/// Read the framed native terminal stream (output, shared size, or exit).
pub fn read_stream(input: &mut impl Read) -> Result<(u8, Vec<u8>)> {
    read_frame(input)
}

/// Keep native GUI streams framed even if attaching fails before its first page.
pub fn report_error(args: &[String], error: &anyhow::Error) {
    if args.iter().any(|arg| arg == "--framed") {
        let _ = frame(
            &mut std::io::stdout().lock(),
            b'X',
            format!("\r\n[terminal attach failed: {error:#}]\r\n").as_bytes(),
        );
    } else {
        eprintln!("{error:#}");
    }
}

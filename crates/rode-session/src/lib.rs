//! A headless, per-user PTY and coding-agent service. GUI clients may disconnect
//! without ending work retained by its lifetime policy. Control uses same-user Unix sockets;
//! A host-owned capability protects control on Landlock ABI 4/5;
//! Linux abstract sockets additionally enforce ABI 6's IPC-domain boundary.
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
//! and last-view cleanup therefore differ. Clients use a single private runtime
//! directory and require the current protocol before dispatching any operation.
//!
//! Agent controllers share this host lifetime but use a single controlling view
//! rather than PTY-style shared input. Their complete protocol/history state
//! comes from `rode-agent-core`, keeping remote helpers independent of the GUI.
//! Pending questions wait without a view; persistence never broadens policy.
//!
//! `protocol` defines the shared wire schema; `framing` owns bounded socket
//! encoding, and `runtime_directory` owns protected host artifacts. This root
//! composes authentication, PTY ownership and dispatch. Agent/runtime controllers
//! and stdin attachment cleanup live in their corresponding private modules.
#![warn(missing_docs)]
use anyhow::{Context, Result, bail, ensure};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::MetadataExt,
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
mod attachment;
#[cfg(target_os = "linux")]
pub mod control_auth;
mod entry;
mod framing;
mod protocol;
mod runtime_directory;
mod runtimes;
mod terminal_output;
pub mod tunnels;

pub use entry::helper_main;
use framing::{MAX_FRAME, frame, read_frame, recv_json, send_json};
pub use protocol::{Info, PROTOCOL, Request, Response};
pub use runtime_directory::directory;
use runtime_directory::{private_directory, private_directory_at, private_listener, runtime_path};

const MAX_SESSIONS: usize = 32;
const MAX_CLIENTS: usize = 8;
// Authentication reads run on blocking workers. Bound these before spawning so
// a local peer cannot create unlimited threads without proving host access.
const MAX_PENDING_CONTROL: usize = 32;
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
/// Executable identity used to start this client's matching service generation.
#[derive(Clone)]
pub struct Launcher {
    /// Native helper executable path, which may refer to the running image after an upgrade.
    pub executable: PathBuf,
    /// Arguments inserted before the service subcommand, such as `--session-service`.
    pub prefix: Vec<String>,
}

#[cfg(target_os = "linux")]
fn control_address(meta: &fs::Metadata) -> std::io::Result<std::os::unix::net::SocketAddr> {
    use std::os::linux::net::SocketAddrExt;
    std::os::unix::net::SocketAddr::from_abstract_name(format!(
        "rode-session-auth-{}-{}-{}",
        meta.uid(),
        meta.dev(),
        meta.ino()
    ))
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct ControlAuthentication;

#[cfg(target_os = "linux")]
impl std::fmt::Display for ControlAuthentication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Cannot authenticate terminal control")
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct ControlDisconnected;

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
fn control_authentication_error(stream: &UnixStream, error: anyhow::Error) -> anyhow::Error {
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

#[cfg(all(test, target_os = "linux"))]
mod control_tests;

/// Perform a blocking control request, starting a matching service when needed. Service
/// creation is serialized; only the current runtime directory is used.
pub fn request(request: &Request, launcher: &Launcher) -> Result<Response> {
    let until = Instant::now()
        + if matches!(
            request,
            Request::List | Request::Agents | Request::Runtimes | Request::Tunnels
        ) {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(10)
        };
    loop {
        let result = request_current(request, launcher);
        let retry = matches!(
            request,
            Request::StartTunnel { .. }
                | Request::Create { .. }
                | Request::CreateAgent { .. }
                | Request::CreateRuntime { .. }
                | Request::List
                | Request::Agents
                | Request::Runtimes
                | Request::Tunnels
        ) && result.as_ref().err().is_some_and(|error| {
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
        // Retry idempotent Create and read-only List across that transition.
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn service_offline(error: &anyhow::Error) -> bool {
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
fn request_once(request: &Request, launcher: &Launcher, dir: &Path) -> Result<Response> {
    let mut stream = match connect_control(dir) {
        Ok(stream) => stream,
        Err(error) => {
            if matches!(
                request,
                Request::List | Request::Agents | Request::Runtimes | Request::Tunnels
            ) {
                return Err(error)
                    .context("Session service is unavailable; existing jobs were not touched");
            }
            if !matches!(
                request,
                Request::StartTunnel { .. }
                    | Request::Create { .. }
                    | Request::CreateAgent { .. }
                    | Request::CreateRuntime { .. }
            ) {
                return Err(error)
                    .context("Session service is offline; no session was created or ended.");
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
    #[cfg(target_os = "linux")]
    control_auth: control_auth::Token,
    runtimes: runtimes::Store,
    tunnels: tunnels::Store,
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
        if !sessions.is_empty() || self.agents.live() || self.runtimes.live() || self.tunnels.live()
        {
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
        if sessions.is_empty()
            && !self.agents.live()
            && !self.runtimes.live()
            && !self.tunnels.live()
        {
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
            mut env,
            persistent,
            allocation,
        } = request
        else {
            unreachable!()
        };
        rode_host_core::private_tmp::apply(&mut env)?;
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
                let arguments = rode_host_core::command::compact_pty(&mut command)?;
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
                Ok((session, reader, stop_read, child, arguments))
            })();
            match setup {
                Err(error) => {
                    let _ = ready_tx.send(Err(format!("{error:#}")));
                }
                Ok((session, reader, stop_read, mut child, arguments)) => {
                    let read_session = session.clone();
                    let (drained_tx, drained_rx) = mpsc::sync_channel(1);
                    std::thread::spawn(move || {
                        terminal_output::read(reader, stop_read, |data| {
                            let mut state = read_session.state.lock().unwrap();
                            state.parser.process(data);
                            Session::broadcast(&mut state, b'O', data);
                        });
                        let _ = drained_tx.send(());
                    });
                    let _ = ready_tx.send(Ok(session.clone()));
                    let status = child.wait();
                    drop(arguments);
                    // Let the reader drain queued bytes before publishing exit.
                    // It bounds idle/live descendants itself, so descheduling the
                    // reader cannot make a supervisor timer discard the tail.
                    let _ = session.stop.lock().unwrap().write_all(b"D");
                    let _ = drained_rx.recv();
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
/// Deny control if mount namespaces differ or either identity cannot be read.
/// Kernel ptrace/proc restrictions can deny the read before comparison, so both
/// rejection paths need to explain the same host-only control boundary.
#[cfg(target_os = "linux")]
fn verify_control_namespace(
    peer: std::io::Result<PathBuf>,
    host: std::io::Result<PathBuf>,
) -> Result<()> {
    let context =
        "Cannot verify terminal control mount namespace; use a client outside the project sandbox";
    ensure!(
        peer.context(context)? == host.context(context)?,
        "Terminal control must originate outside the project sandbox"
    );
    Ok(())
}

fn handle(
    server: &Arc<Server>,
    mut stream: UnixStream,
    admission: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
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
        verify_control_namespace(
            fs::read_link(format!("/proc/{}/ns/mnt", cred.pid)),
            fs::read_link("/proc/self/ns/mnt"),
        )?;
    }
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    #[cfg(target_os = "linux")]
    {
        server.control_auth.verify(&mut stream)?;
        send_json(&mut stream, &Response::Ok)?;
    }
    ensure!(
        matches!(recv_json::<_, Request>(&mut stream)?, Request::Protocol),
        "Session protocol handshake required"
    );
    send_json(&mut stream, &Response::Protocol(PROTOCOL))?;
    // Established attachments have their own controller/view limits. They must
    // not occupy the budget reserved for clients still proving their authority.
    drop(admission);
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
            Request::Protocol => Response::Protocol(PROTOCOL),
            Request::Tunnels => Response::Tunnels(server.tunnels.list()),
            Request::StartTunnel { id, spec } => {
                let _creation = server.sessions.lock().unwrap();
                ensure!(
                    !server.stop.load(Ordering::Acquire),
                    "Session service is stopping"
                );
                Response::Tunnel(Box::new(server.tunnels.start(id, spec)?))
            }
            Request::StopTunnel { id } => {
                server.tunnels.stop(&id);
                Response::Ok
            }
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
                if sessions.is_empty()
                    && !server.agents.live()
                    && !server.runtimes.live()
                    && !server.tunnels.live()
                {
                    server.stop.store(true, Ordering::Release);
                }
                Response::Ok
            }
            Request::Shutdown => {
                let sessions = server.sessions.lock().unwrap();
                ensure!(
                    !server.tunnels.live(),
                    "Stop SSH tunnels before stopping the service"
                );
                ensure!(
                    !server.runtimes.live(),
                    "End notebook/debugger runtimes before stopping the service"
                );
                ensure!(
                    !server.agents.live(),
                    "End running agents before stopping the service"
                );
                ensure!(
                    sessions.values().all(|session| !session.info().alive),
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
/// Call in a dedicated helper before spawning other threads: startup briefly
/// changes the file-creation mask, restoring it before accepting work.
pub fn serve() -> Result<()> {
    let (dir, directory) = private_directory()?;
    let runtime = runtime_path(&dir, &directory);
    let socket = runtime.join("socket");
    let lock = rode_host_core::locking::open_private(
        &runtime.join("lock"),
        rode_host_core::locking::PrivateFile::Lock,
    )
    .with_context(|| {
        format!(
            "Cannot open private session lock {}",
            dir.join("lock").display()
        )
    })?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(());
    }
    if socket.exists() {
        let meta = fs::symlink_metadata(&socket)?;
        ensure!(
            !meta.file_type().is_symlink() && meta.uid() == unsafe { libc::geteuid() },
            "Unsafe terminal socket"
        );
        fs::remove_file(&socket)?;
    }
    #[cfg(target_os = "linux")]
    let control_auth = control_auth::Token::publish(&runtime)?;
    // Publish discovery only after the authenticated endpoint is listening. A
    // filesystem socket alone would be reachable through Landlock's pathname
    // Unix-socket limitation, even without a grant for the runtime directory.
    #[cfg(target_os = "linux")]
    let listener = UnixListener::bind_addr(&control_address(&directory.metadata()?)?)?;
    let discovery = private_listener(&socket)?;
    #[cfg(not(target_os = "linux"))]
    let listener = discovery.try_clone()?;
    let server = Arc::new(Server {
        #[cfg(target_os = "linux")]
        control_auth,
        agents: agents::Store::default(),
        runtimes: runtimes::Store::default(),
        tunnels: tunnels::Store::default(),
        sessions: Mutex::new(HashMap::new()),
        last_exit: Mutex::new(Instant::now()),
        stop: AtomicBool::new(false),
        next_viewer: AtomicU64::new(1),
    });
    let mut idle_since = Some(Instant::now());
    let pending_control = Arc::new(tokio::sync::Semaphore::new(MAX_PENDING_CONTROL));
    while !server.stop.load(Ordering::Acquire) {
        let ids: Vec<_> = server.sessions.lock().unwrap().keys().cloned().collect();
        for id in ids {
            server.end_unattached(&id, true);
        }
        server.stop_if_idle(&mut idle_since);
        if server.stop.load(Ordering::Acquire) {
            break;
        }
        let mut fds = [
            libc::pollfd {
                fd: listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if cfg!(target_os = "linux") {
                    discovery.as_raw_fd()
                } else {
                    -1
                },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 1000) } <= 0 {
            continue;
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let (mut stream, _) = discovery.accept()?;
            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
            let _ = send_json(&mut stream, &Response::Error("Terminal control must originate outside the project sandbox using an updated RODE helper".into()));
        }
        if fds[0].revents & libc::POLLIN == 0 {
            continue;
        }
        let (stream, _) = listener.accept()?;
        let Ok(admission) = pending_control.clone().try_acquire_owned() else {
            continue;
        };
        let server = server.clone();
        // Failed worker creation drops the socket and permit. It must not panic
        // the daemon and terminate retained work during resource pressure.
        let _ = std::thread::Builder::new()
            .name("rode-control".into())
            .spawn(move || {
                let result = stream
                    .try_clone()
                    .map_err(anyhow::Error::from)
                    .and_then(|peer| handle(&server, peer, admission));
                if let Err(error) = result {
                    let mut stream = stream;
                    let _ = send_json(&mut stream, &Response::Error(format!("{error:#}")));
                }
            });
    }
    fs::remove_file(&socket)?;
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
    attach_stream(id, policy, framed, false)
}

fn attach_stream(id: &str, policy: &str, framed: bool, heartbeat: bool) -> Result<()> {
    let mut stream = connect_control(&directory()?)?;
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
    let termios = if !framed && unsafe { libc::tcgetattr(0, &mut old) } == 0 {
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
    let mut framed_input = framed
        .then(rode_host_core::connection::Input::stdin)
        .transpose()?;
    if !heartbeat && let Some(input) = &mut framed_input {
        input.unbounded();
    }
    if !framed {
        unsafe {
            libc::signal(libc::SIGWINCH, resized as *const () as libc::sighandler_t);
        }
    }
    let input = Arc::new(Mutex::new(stream.try_clone()?));
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    std::thread::spawn(move || {
        if let Some(mut stdin) = framed_input {
            // A remote attachment has pipes, not a TTY. Carry dimensions in
            // the same ordered stream as input instead of relying on SIGWINCH.
            while !stop.load(Ordering::Acquire) {
                let Ok((kind, bytes)) = read_frame(&mut stdin) else {
                    break;
                };
                if kind == b'H' && bytes.is_empty() {
                    continue;
                }
                if !matches!((kind, bytes.len()), (b'I', _) | (b'R', 4)) {
                    break;
                }
                if frame(&mut *input.lock().unwrap(), kind, &bytes).is_err() {
                    break;
                }
            }
            let _ = input.lock().unwrap().shutdown(std::net::Shutdown::Both);
            return;
        }
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
        let mut output: Box<dyn Write> = if framed {
            Box::new(rode_host_core::connection::Output::stdout()?)
        } else {
            Box::new(std::io::stdout().lock())
        };
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
    if args.first().is_some_and(|s| s == "--git-credential") {
        return rode_host_core::project::git_auth::helper(&args[1..]);
    }
    if args.first().is_some_and(|s| s == "--landlock-launch") {
        return rode_host_core::landlock::launch(&args[1..]);
    }
    if args.first().is_some_and(|s| s == "--mise-launch") {
        rode_host_core::supervisor::signals()?;
        return rode_host_core::mise::native::launch(&args[1..]);
    }
    match args.first().map(String::as_str) {
        Some("--slurm-step") => return rode_host_core::slurm::native::step(&args[1..]),
        Some("--compute-step") => return rode_host_core::slurm::native::compute(&args[1..], false),
        Some("--compute-runtime") => {
            return rode_host_core::slurm::native::compute(&args[1..], true);
        }
        _ => {}
    }
    if args.len() == 1 {
        match args[0].as_str() {
            "--agent-file-tool" => return rode_host_core::agent_files::serve(),
            "host-rpc" => return host::serve(),
            "--host-info" => return host::info(),
            "--runtime-cache" => return rode_host_core::runtime_cache::serve(),
            "--process" => return rode_host_core::supervisor::serve(),
            "--workspace-request" => return host::workspace_request(),
            "--host-worker" => return host::worker(),
            "--file-helper" => return host::file_request(),
            "--debug-helper" => return rode_host_core::debugger::bridge::coordinator(),
            "--vault-helper" => return rode_host_core::vault::coordinator(),
            "--latex-helper" => return rode_host_core::latex::coordinator(),
            "--marp-helper" => return rode_host_core::marp::coordinator(),
            "--extension-helper" => return rode_host_core::extensions::coordinator(),
            _ => {}
        }
    }
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
        Some("attach") if args.len() == 5 && args[1..3] == ["--framed", "--heartbeat"] => {
            attach_stream(&args[3], &args[4], true, true)
        }
        _ => bail!("Usage: rode-sessiond serve | request | attach SESSION POLICY"),
    }
}
fn request_result(request: Request, launcher: &Launcher) -> Response {
    self::request(&request, launcher).unwrap_or_else(|error| Response::Error(format!("{error:#}")))
}

/// Write one bounded native stream frame, including terminal input and resize.
pub fn write_stream(output: &mut impl Write, kind: u8, bytes: &[u8]) -> Result<()> {
    frame(output, kind, bytes)
}

/// Read a bounded native stream frame (input, output, size, or exit).
pub fn read_stream(input: &mut impl Read) -> Result<(u8, Vec<u8>)> {
    read_frame(input)
}

/// Keep native GUI streams framed even if attaching fails before its first page.
pub fn report_error(args: &[String], error: &anyhow::Error) {
    if args.iter().any(|arg| arg == "--framed") {
        if let Ok(output) = rode_host_core::connection::Output::stdout() {
            let _ = frame(
                &mut output.with_timeout(Duration::from_secs(1)),
                b'X',
                format!("\r\n[terminal attach failed: {error:#}]\r\n").as_bytes(),
            );
        }
    } else if let Ok(output) = rode_host_core::connection::Output::stderr() {
        let _ = writeln!(output.with_timeout(Duration::from_secs(1)), "{error:#}");
    }
}

/// Project policy to persistent-service request conversion.
pub mod workspace;

/// Native connection-owned host RPC composition.
pub mod host;

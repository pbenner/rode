//! Disposable desktop attachments to native host-owned notebook/debugger controllers.
use crate::{files, process, project, sandbox, sessions, ssh};
use anyhow::{Result, ensure};
use rode_runtime_core::service::{Command, Envelope, Info, Spec, State, Update};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

/// Execute a read-only discovery or explicit mutation outside the process sandbox.
pub fn request(
    plan: &sandbox::Plan,
    remote: Option<&ssh::Remote>,
    request: rode_session::Request,
) -> Result<sessions::Reply> {
    request_with_upload(plan, remote, request, true)
}
/// Reuse a previously verified helper without resending its executable payload.
pub fn request_with_upload(
    plan: &sandbox::Plan,
    remote: Option<&ssh::Remote>,
    request: rode_session::Request,
    upload: bool,
) -> Result<sessions::Reply> {
    let operation = sessions::Operation::Runtimes {
        request: Box::new(request),
    };
    if let Some(remote) = remote {
        let reply = remote.request(&files::Request::Tool {
            request: project::Request::Sessions {
                root: plan.root.clone(),
                operation,
                helper: Some(sessions::helper(upload)?),
                directory: std::env::var("RODE_SESSION_DIR").ok(),
            },
        })?;
        match reply.tool {
            Some(project::Response::Sessions(reply)) => Ok(reply),
            _ => anyhow::bail!("Missing runtime service response"),
        }
    } else {
        sessions::local(&operation)
    }
}
/// End a discovered runtime and wait for its owned processes to be reaped.
pub fn end(plan: &sandbox::Plan, remote: Option<&ssh::Remote>, info: &Info) -> Result<()> {
    end_on_host(plan, remote, &Identity::from(info))
}
fn end_on_host(
    plan: &sandbox::Plan,
    remote: Option<&ssh::Remote>,
    identity: &Identity,
) -> Result<()> {
    request_with_upload(
        plan,
        remote,
        rode_session::Request::EndRuntime {
            id: identity.id.clone(),
            policy: identity.policy.clone(),
        },
        false,
    )?;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let reply = request_with_upload(plan, remote, rode_session::Request::Runtimes, false)?;
        if let rode_session::Response::Runtimes(infos) = reply.response
            && infos.iter().all(|i| i.id != identity.id)
        {
            return Ok(());
        }
        ensure!(
            std::time::Instant::now() < until,
            "Runtime shutdown is still pending on the host; inspect Sessions before releasing its allocation"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
#[derive(Clone)]
struct Identity {
    id: String,
    policy: String,
}
impl From<&Info> for Identity {
    fn from(info: &Info) -> Self {
        Self {
            id: info.id.clone(),
            policy: info.policy.clone(),
        }
    }
}
type Prepared = Result<(sessions::Reply, Info), String>;
enum Lifecycle {
    Preparing {
        result: mpsc::Receiver<Prepared>,
        owns_creation: bool,
    },
    Connecting(process::Session),
    Attached(process::Session),
    Offline,
    Ending(mpsc::Receiver<Result<(), String>>),
    Ended,
}
impl Lifecycle {
    fn bridge(&self) -> Option<&process::Session> {
        match self {
            Self::Connecting(bridge) | Self::Attached(bridge) => Some(bridge),
            _ => None,
        }
    }
}
/// One exclusive, bounded attachment. Dropping it preserves only persistent work.
pub struct Client {
    cleanup: Arc<AtomicBool>,
    identity: Identity,
    lifecycle: Lifecycle,
    plan: sandbox::Plan,
    remote: Option<ssh::Remote>,
    wake: Arc<dyn Fn() + Send + Sync>,
    queued: VecDeque<Envelope>,
    queued_bytes: usize,
    inflight: VecDeque<String>,
    /// Latest acknowledged host identity, configuration and queue state.
    pub state: Option<State>,
    /// Latest host identity, even before initial replay completes.
    pub info: Option<Info>,
    /// Current attachment/preparation failure; does not imply the host job ended.
    pub error: Option<String>,
    initial_persistence: bool,
    persistence_change: Option<bool>,
}
impl Client {
    /// Create one host controller asynchronously after an explicit Run/Start.
    pub fn start(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        mut spec: Spec,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        let persistent = spec.persistent;
        spec.name = spec
            .name
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect();
        spec.policy = sessions::fingerprint(&plan);
        spec.root = plan.root.clone();
        spec.prefix = if plan.active {
            plan.prefix.clone()
        } else {
            vec![]
        };
        spec.mode = plan.label().into();
        let identity = Identity {
            id: spec.id.clone(),
            policy: spec.policy.clone(),
        };
        let (tx, rx) = mpsc::channel();
        let host = plan.clone();
        let peer = remote.clone();
        let repaint = wake.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<_> {
                let reply = request(
                    &host,
                    peer.as_ref(),
                    rode_session::Request::CreateRuntime {
                        spec: Box::new(spec),
                    },
                )?;
                let rode_session::Response::Runtime(info) = &reply.response else {
                    anyhow::bail!("Unexpected runtime creation reply")
                };
                let info = info.as_ref().clone();
                Ok((reply, info))
            })();
            if let Err(mpsc::SendError(Ok((_, info)))) =
                tx.send(result.map_err(|e| format!("{e:#}")))
                && !persistent
            {
                let _ = end(&host, peer.as_ref(), &info);
            }
            repaint();
        });
        Ok(Self::new(
            plan,
            remote,
            identity,
            persistent,
            Lifecycle::Preparing {
                result: rx,
                owns_creation: true,
            },
            None,
            wake,
        ))
    }
    /// Reattach without starting a process or rerunning cells/programs.
    pub fn attach(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        info: Info,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        ensure!(
            info.policy == sessions::fingerprint(&plan) && info.root == plan.root,
            "Open the runtime's original project and sandbox policy before attaching"
        );
        let persistent = info.persistent;
        let identity = Identity::from(&info);
        let cached = info.clone();
        let (tx, rx) = mpsc::channel();
        let host = plan.clone();
        let peer = remote.clone();
        let repaint = wake.clone();
        std::thread::spawn(move || {
            let result = request(&host, peer.as_ref(), rode_session::Request::Runtimes)
                .map(|reply| (reply, info))
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            repaint();
        });
        Ok(Self::new(
            plan,
            remote,
            identity,
            persistent,
            Lifecycle::Preparing {
                result: rx,
                owns_creation: false,
            },
            Some(cached),
            wake,
        ))
    }
    fn new(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        identity: Identity,
        persistent: bool,
        lifecycle: Lifecycle,
        info: Option<Info>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            cleanup: Arc::new(AtomicBool::new(false)),
            identity,
            lifecycle,
            plan,
            remote,
            wake,
            queued: VecDeque::new(),
            queued_bytes: 0,
            inflight: VecDeque::new(),
            state: None,
            info,
            error: None,
            initial_persistence: persistent,
            persistence_change: None,
        }
    }
    fn connect(&mut self, reply: sessions::Reply, info: Info) -> Result<()> {
        ensure!(
            info.policy == sessions::fingerprint(&self.plan) && info.root == self.plan.root,
            "Runtime policy changed; reconnect with its original policy"
        );
        let mut argv = vec![reply.executable];
        argv.extend(reply.prefix);
        argv.extend([
            "attach-runtime".into(),
            info.id.clone(),
            info.policy.clone(),
        ]);
        let mut plan = sandbox::Plan::default();
        plan.root = self.plan.root.clone();
        let bridge = process::Session::start(
            process::Launch {
                argv,
                plan,
                remote: self.remote.clone(),
                python: "off".into(),
                python_program: false,
                native_debug: false,
                line_limit: 16 * 1024 * 1024,
            },
            self.wake.clone(),
        )?;
        self.lifecycle = Lifecycle::Connecting(bridge);
        self.info = Some(info);
        Ok(())
    }
    /// Queue a high-level operation. Source payloads are bounded before any side effect.
    pub fn send(&mut self, command: Command) -> Result<()> {
        ensure!(
            !matches!(self.lifecycle, Lifecycle::Ending(_) | Lifecycle::Ended),
            "Runtime shutdown is pending"
        );
        ensure!(
            !self.offline() && self.error.is_none(),
            "Runtime attachment is offline; reattach before sending more work"
        );
        let command = Envelope {
            id: rode_session::new_id()?,
            command,
        };
        let bytes = serde_json::to_vec(&command)?.len();
        ensure!(
            self.queued.len() < 64 && self.queued_bytes + bytes < 12 * 1024 * 1024,
            "Runtime command queue is full; wait for acknowledgement"
        );
        self.queued_bytes += bytes;
        self.queued.push_back(command);
        Ok(())
    }
    /// Drain state and replay, then retry bounded queued writes.
    pub fn poll(&mut self) -> Vec<Update> {
        let mut updates = vec![];
        match &self.lifecycle {
            Lifecycle::Ending(result) => match result.try_recv() {
                Ok(result) => {
                    self.lifecycle = Lifecycle::Ended;
                    if let Err(error) = result {
                        self.error = Some(error);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.lifecycle = Lifecycle::Ended;
                    self.error = Some("Runtime shutdown worker disconnected; inspect Sessions before releasing its allocation".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            },
            Lifecycle::Preparing { result, .. } => match result.try_recv() {
                Ok(result) => {
                    if let Err(error) = result.and_then(|(reply, info)| {
                        self.connect(reply, info).map_err(|e| format!("{e:#}"))
                    }) {
                        self.lifecycle = Lifecycle::Offline;
                        self.error = Some(error);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.lifecycle = Lifecycle::Offline;
                    self.error = Some(
                        "Runtime startup worker disconnected; inspect Sessions before retrying"
                            .into(),
                    );
                }
                Err(mpsc::TryRecvError::Empty) => {}
            },
            _ => {}
        }
        let mut connected = self.connected();
        let mut failed = false;
        if let Some(bridge) = self.lifecycle.bridge() {
            for _ in 0..64 {
                let Some(event) = bridge.poll() else { break };
                match event {
                    process::Event::Message(value) => match serde_json::from_value::<Update>(value)
                    {
                        Ok(Update::State(state)) => {
                            connected = true;
                            self.info = Some(state.info.clone());
                            self.inflight
                                .retain(|id| !state.acknowledgements.contains(id));
                            if self.persistence_change == Some(state.info.persistent) {
                                self.persistence_change = None;
                            }
                            if !state.error.is_empty() {
                                self.error = Some(state.error.clone());
                            }
                            self.state = Some(*state.clone());
                            updates.push(Update::State(state));
                        }
                        Ok(Update::Error(error)) => {
                            failed = true;
                            self.error = Some(error);
                        }
                        Ok(update) => updates.push(update),
                        Err(error) => {
                            failed = true;
                            self.error = Some(error.to_string());
                        }
                    },
                    process::Event::Error(error) => {
                        failed = true;
                        self.error = Some(error);
                    }
                    process::Event::Exited(code) => {
                        failed = true;
                        connected = false;
                        if !matches!(self.lifecycle, Lifecycle::Ending(_) | Lifecycle::Ended) {
                            self.error = Some(format!(
                                "Runtime view disconnected ({code:?}); persistent work may still be running. Reattach from Sessions."
                            ));
                        }
                    }
                    process::Event::Stderr(error) => {
                        if !connected {
                            self.error = Some(error);
                        }
                    }
                    process::Event::Started => {}
                }
            }
            if connected && !failed {
                for _ in 0..32 {
                    let Some(command) = self.queued.front() else {
                        break;
                    };
                    match bridge
                        .writer()
                        .try_send(&serde_json::to_value(command).unwrap())
                    {
                        Ok(true) => {
                            self.queued_bytes -= serde_json::to_vec(command).unwrap().len();
                            let sent = self.queued.pop_front().unwrap();
                            self.inflight.push_back(sent.id);
                        }
                        Ok(false) => break,
                        Err(error) => {
                            failed = true;
                            self.error = Some(error.to_string());
                            break;
                        }
                    }
                }
            }
        }
        if failed {
            self.lifecycle = Lifecycle::Offline;
        } else if connected && matches!(self.lifecycle, Lifecycle::Connecting(_)) {
            let Lifecycle::Connecting(bridge) =
                std::mem::replace(&mut self.lifecycle, Lifecycle::Offline)
            else {
                unreachable!()
            };
            self.lifecycle = Lifecycle::Attached(bridge);
        }
        updates
    }
    /// Acknowledged persistence; enabling it must complete before detachment.
    pub fn persistent(&self) -> bool {
        self.state
            .as_ref()
            .map_or(self.initial_persistence, |s| s.info.persistent)
    }
    /// Whether a requested persistence change is waiting for host acknowledgement.
    pub fn persistence_pending(&self) -> bool {
        self.persistence_change.is_some()
    }
    /// Whether this attachment failed and needs an explicit reattachment.
    pub fn offline(&self) -> bool {
        matches!(self.lifecycle, Lifecycle::Offline)
    }
    /// Whether startup or queued state mutations require acknowledgement.
    pub fn pending(&self) -> bool {
        match self.lifecycle {
            Lifecycle::Preparing { .. } | Lifecycle::Connecting(_) | Lifecycle::Ending(_) => true,
            Lifecycle::Attached(_) => {
                self.persistence_change.is_some()
                    || !self.queued.is_empty()
                    || !self.inflight.is_empty()
            }
            Lifecycle::Offline | Lifecycle::Ended => false,
        }
    }
    /// Whether authoritative state has arrived from the controlling lease.
    pub fn connected(&self) -> bool {
        matches!(self.lifecycle, Lifecycle::Attached(_))
    }
    /// Change last-view lifetime without restarting computation.
    pub fn set_persistent(&mut self, persistent: bool) -> Result<()> {
        ensure!(
            self.connected() && self.persistence_change.is_none(),
            "Wait for the runtime connection/persistence acknowledgement"
        );
        self.send(Command::Persistence { persistent })?;
        self.persistence_change = Some(persistent);
        Ok(())
    }
    /// Explicit end always stops the host computation; ordinary Drop only detaches.
    pub fn end(&mut self) {
        if matches!(self.lifecycle, Lifecycle::Ending(_) | Lifecycle::Ended) {
            return;
        }
        let previous = std::mem::replace(&mut self.lifecycle, Lifecycle::Ended);
        let preparation = match previous {
            Lifecycle::Preparing { result, .. } => Some(result),
            _ => None, // Disposing a bridge releases its lease before shutdown.
        };
        let plan = self.plan.clone();
        let remote = self.remote.clone();
        let identity = self.identity.clone();
        let wake = self.wake.clone();
        let cleanup = self.cleanup.clone();
        cleanup.store(true, Ordering::Release);
        let (tx, rx) = mpsc::channel();
        self.lifecycle = Lifecycle::Ending(rx);
        self.queued.clear();
        self.queued_bytes = 0;
        self.inflight.clear();
        std::thread::spawn(move || {
            // Own the creation reply even if it has already been published but
            // never polled. End must follow creation, not race ahead of it.
            if let Some(preparation) = preparation {
                let _ = preparation.recv();
            }
            let result = end_on_host(&plan, remote.as_ref(), &identity);
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
            cleanup.store(false, Ordering::Release);
            wake();
        });
    }
    /// Shared acknowledged host-cleanup flag.
    pub fn cleanup(&self) -> Arc<AtomicBool> {
        self.cleanup.clone()
    }
    /// Whether explicit termination is pending on the host.
    pub fn is_stopping(&self) -> bool {
        matches!(self.lifecycle, Lifecycle::Ending(_))
    }
    /// Clone an input handle for bounded debugger terminal operations.
    pub fn writer(&self) -> Option<process::Writer> {
        self.lifecycle.bridge().map(process::Session::writer)
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        // A creation reply can already be queued after its worker exited. Keep
        // an owner for cleanup rather than relying on a cancellation flag.
        if !self.persistent()
            && matches!(
                self.lifecycle,
                Lifecycle::Preparing {
                    owns_creation: true,
                    ..
                }
            )
        {
            self.end();
        }
    }
}

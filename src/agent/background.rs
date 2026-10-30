//! Disposable local/SSH views of host-owned agent controllers. Preparation,
//! helper deployment and control operations run off the GUI thread. Dropping a
//! view closes only its bridge; the host decides last-view lifetime.
use super::{Config, Launch};
use crate::{files, project, sessions as service};
use anyhow::{Context, Result, ensure};
use rode_agent_core::service::{Command, Info, Spec, State};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

/// Local/SSH service operation using the same verified helper as native terminals.
pub fn request(
    launch: &Launch,
    request: rode_session::Request,
    upload: bool,
) -> Result<service::Reply> {
    let operation = service::Operation::Agents {
        request: Box::new(request),
    };
    if let Some(remote) = &launch.remote {
        let response = remote.request(&files::Request::Tool {
            request: project::Request::Sessions {
                root: launch.plan.root.clone(),
                operation,
                helper: Some(service::helper(upload)?),
                directory: std::env::var("RODE_SESSION_DIR").ok(),
            },
        })?;
        match response.tool {
            Some(project::Response::Sessions(reply)) => Ok(reply),
            _ => anyhow::bail!("Missing background-agent service reply"),
        }
    } else {
        service::local(&operation)
    }
}
struct Preparing {
    pending: AtomicBool,
    cancelled: AtomicBool,
}
/// One controlling attachment. Only an explicit message or History creates a
/// controller; setting persistence before Send creates no process.
pub struct Client {
    launch: Launch,
    ready: mpsc::Receiver<Result<(service::Reply, Info), String>>,
    preparing: Arc<Preparing>,
    bridge: Option<crate::process::Session>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Latest acknowledged host state.
    pub state: Option<State>,
    /// Last connection/preparation failure, consumed by its owning UI.
    pub error: Option<String>,
    id: String,
    desired: bool,
    persistence_pending: Option<bool>,
    /// A requested explicit End is distinct from detaching a persistent view.
    pub ending: bool,
    closed: bool,
}
impl Client {
    /// Prepare and create a host-owned conversation, or explicit history browser.
    pub fn start(
        launch: Launch,
        text: Option<String>,
        persistent: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        launch.config.arguments(text.is_some())?;
        if let Some(text) = &text {
            ensure!(
                !text.trim().is_empty() && text.len() <= super::PROMPT_LIMIT,
                "Agent messages must contain 1–32768 bytes"
            );
        }
        let id = rode_session::new_id()?;
        let mut client = Self::pending(launch.clone(), id.clone(), persistent, wake.clone());
        let (tx, rx) = mpsc::channel();
        client.ready = rx;
        let preparing = client.preparing.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<_> {
                let argv = launch.config.arguments(text.is_some())?;
                let mut plan = launch.plan.clone();
                let executable = if argv[0] == crate::bundled::AUTO_CODEX {
                    let (program, prefix) = crate::bundled::prepare(
                        &plan,
                        launch.remote.as_ref(),
                        &preparing.cancelled,
                    )?;
                    plan.prefix = prefix;
                    program
                } else {
                    argv[0].clone()
                };
                ensure!(
                    !preparing.cancelled.load(Ordering::Acquire),
                    "Agent start cancelled"
                );
                let title = text
                    .as_deref()
                    .and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
                    .unwrap_or("Saved conversations");
                let name: String = title.chars().filter(|c| !c.is_control()).take(60).collect();
                let spec = Spec {
                    id: id.clone(),
                    name,
                    policy: service::fingerprint(&launch.plan),
                    mode: plan.label().into(),
                    config: launch.config.clone(),
                    plan: rode_agent_core::Plan {
                        root: plan.root.clone(),
                        active: plan.active,
                        project_read_only: plan.project_read_only,
                        prefix: plan.prefix,
                    },
                    executable,
                    env: BTreeMap::new(),
                    python: launch.python.clone(),
                    persistent,
                    text,
                };
                let reply = request(
                    &launch,
                    rode_session::Request::CreateAgent {
                        spec: Box::new(spec),
                    },
                    true,
                )?;
                let rode_session::Response::Agent(info) = &reply.response else {
                    anyhow::bail!("Unexpected background-agent creation reply");
                };
                let info = info.clone();
                if preparing.cancelled.load(Ordering::Acquire) {
                    request(
                        &launch,
                        rode_session::Request::EndAgent {
                            id: id.clone(),
                            policy: info.policy.clone(),
                        },
                        false,
                    )?;
                    anyhow::bail!("Agent start cancelled");
                }
                Ok((reply, info))
            })()
            .map_err(|e| format!("{e:#}"));
            if let Err(mpsc::SendError(Ok((_, info)))) = tx.send(result)
                && (!persistent || preparing.cancelled.load(Ordering::Acquire))
            {
                let _ = request(
                    &launch,
                    rode_session::Request::EndAgent {
                        id,
                        policy: info.policy,
                    },
                    false,
                );
            }
            preparing.pending.store(false, Ordering::Release);
            wake();
        });
        Ok(client)
    }
    fn pending(
        launch: Launch,
        id: String,
        desired: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (_, ready) = mpsc::channel();
        Self {
            launch,
            ready,
            preparing: Arc::new(Preparing {
                pending: AtomicBool::new(true),
                cancelled: AtomicBool::new(false),
            }),
            bridge: None,
            wake,
            state: None,
            error: None,
            id,
            desired,
            persistence_pending: None,
            ending: false,
            closed: false,
        }
    }
    /// Attach an existing controller without submitting a model prompt.
    pub fn attach(launch: Launch, info: Info, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let mut client = Self::pending(
            launch.clone(),
            info.id.clone(),
            info.persistent,
            wake.clone(),
        );
        let (tx, rx) = mpsc::channel();
        client.ready = rx;
        let preparing = client.preparing.clone();
        std::thread::spawn(move || {
            let result = request(&launch, rode_session::Request::Agents, true)
                .map(|reply| (reply, info))
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            preparing.pending.store(false, Ordering::Release);
            wake();
        });
        client
    }
    fn connect(&mut self, reply: service::Reply, info: Info) -> Result<()> {
        ensure!(
            info.policy == service::fingerprint(&self.launch.plan),
            "Open the agent's original project and sandbox policy before attaching"
        );
        let mut argv = vec![reply.executable];
        argv.extend(reply.prefix);
        argv.extend(["attach-agent".into(), info.id, info.policy]);
        let control = crate::sandbox::Plan {
            root: self.launch.plan.root.clone(),
            ..Default::default()
        };
        // The broker connection remains outside the project namespace, like
        // native terminal attachment. The agent itself retains its fixed policy.
        self.bridge = Some(crate::process::Session::start(
            crate::process::Launch {
                argv,
                plan: control,
                remote: self.launch.remote.clone(),
                python: "off".into(),
                python_program: false,
                line_limit: 4 * 1024 * 1024,
                native_debug: false,
            },
            self.wake.clone(),
        )?);
        if self.ending {
            self.send(Command::End)?;
        }
        Ok(())
    }
    /// Drain bounded service updates; the host owns all protocol deadlines.
    pub fn poll(&mut self) -> Option<State> {
        if self.bridge.is_none() && !self.closed {
            match self.ready.try_recv() {
                Ok(Ok((reply, info))) => {
                    if let Err(error) = self.connect(reply, info) {
                        self.error = Some(format!("{error:#}"));
                        self.closed = true;
                    }
                }
                Ok(Err(error)) => {
                    self.error = Some(error);
                    self.closed = true;
                }
                Err(_) => {}
            }
        }
        let mut update = None;
        for _ in 0..16 {
            let Some(event) = self.bridge.as_ref().and_then(crate::process::Session::poll) else {
                break;
            };
            match event {
                crate::process::Event::Message(value) => {
                    match serde_json::from_value::<rode_agent_core::service::Update>(value)
                        .map_err(anyhow::Error::from)
                        .and_then(|update| update.apply(self.state.as_ref()))
                    {
                        Ok(state) => {
                            self.launch.config = state.chat.config.clone();
                            if self.persistence_pending == Some(state.info.persistent) {
                                self.persistence_pending = None;
                            }
                            self.desired = state.info.persistent;
                            if !state.info.alive {
                                self.closed = true;
                            }
                            self.state = Some(state.clone());
                            update = Some(state);
                        }
                        Err(error) => {
                            self.error = Some(format!("Invalid agent service reply: {error}"))
                        }
                    }
                }
                crate::process::Event::Error(error) | crate::process::Event::Stderr(error) => {
                    self.error = Some(error)
                }
                crate::process::Event::Exited(_) => {
                    self.closed = true;
                    if self.state.as_ref().is_some_and(|s| s.info.alive) {
                        self.error.get_or_insert_with(|| "Agent view disconnected; persistent work may still be running. Use Running to reattach.".into());
                    }
                }
                crate::process::Event::Started => {}
            }
        }
        if self.closed {
            self.bridge = None;
            self.persistence_pending = None;
        }
        update
    }
    /// Queue an explicit controller action without blocking the UI.
    pub fn send(&self, action: Command) -> Result<()> {
        ensure!(
            !self.closed,
            "Agent view is disconnected; reattach from Running"
        );
        self.bridge
            .as_ref()
            .context("Agent service is still preparing")?
            .send(&serde_json::to_value(action)?)
    }
    /// Whether this exact process/host/interpreter can accept a follow-up.
    pub fn matches(&self, launch: &Launch) -> bool {
        !self.closed
            && self.launch.config.same_process(&launch.config)
            && self.launch.python == launch.python
            && service::fingerprint(&self.launch.plan) == service::fingerprint(&launch.plan)
    }
    /// Last acknowledged persistence flag; pending requests do not paint as enabled.
    pub fn persistent(&self) -> bool {
        self.state
            .as_ref()
            .map(|s| s.info.persistent)
            .unwrap_or(self.desired)
    }
    /// Change lifetime in place, waiting for an authoritative acknowledgement.
    pub fn set_persistent(&mut self, persistent: bool) -> Result<()> {
        self.send(Command::Persistence { persistent })?;
        self.persistence_pending = Some(persistent);
        Ok(())
    }
    /// Preparation/lifetime changes must finish before a clean editor exit.
    pub fn work_pending(&self) -> bool {
        self.preparing.pending.load(Ordering::Acquire)
            || (!self.closed && self.state.is_none())
            || self.persistence_pending.is_some()
            || (self.ending && !self.closed)
    }
    /// Request explicit termination, including an in-flight initial preparation.
    pub fn end(&mut self) {
        self.preparing.cancelled.store(true, Ordering::Release);
        self.ending = true;
        if self.bridge.is_some()
            && let Err(error) = self.send(Command::End)
        {
            self.error = Some(error.to_string());
        }
    }
    /// Cancel optional startup work on a nonpersistent clean exit.
    pub fn cancel_startup_for_exit(&self) {
        if !self.persistent() {
            self.preparing.cancelled.store(true, Ordering::Release);
        }
    }
    /// Whether the controlling connection has ended.
    pub fn closed(&self) -> bool {
        self.closed
    }
    /// Stable native identity for background management and test assertions.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Current backend settings restored into the UI on reattachment.
    pub fn config(&self) -> Option<&Config> {
        self.state.as_ref().map(|s| &s.chat.config)
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        if !self.persistent() || self.ending {
            self.preparing.cancelled.store(true, Ordering::Release);
        }
    }
}

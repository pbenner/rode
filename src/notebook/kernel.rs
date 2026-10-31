//! Desktop kernel launches and disposable views of host-owned execution.
use crate::{process, runtime, sandbox, ssh};
use anyhow::{Result, ensure};
pub use rode_runtime_core::notebook::{Event, Run};
use rode_runtime_core::service::{self, Command, Update};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
enum Backend {
    Direct(Box<rode_runtime_core::notebook::Kernel>),
    Host(Box<runtime::Client>),
}
/// A kernel view. Persistent execution, queue and outputs belong to the host.
pub struct Kernel {
    backend: Backend,
    bindings: Vec<Run>,
    source_hashes: BTreeMap<String, String>,
    document: String,
    revisions: BTreeMap<String, u64>,
    ready: bool,
    busy: bool,
    submitted: bool,
    retain_sent: bool,
    /// Current lifecycle status.
    pub status: String,
    /// Actual runtime interpreter.
    pub python: String,
    /// Installed kernelspec.
    pub name: String,
    /// Actual host/allocation.
    pub execution: String,
}
impl Kernel {
    fn direct(inner: rode_runtime_core::notebook::Kernel) -> Self {
        Self {
            status: inner.status.clone(),
            python: inner.python.clone(),
            name: inner.name.clone(),
            execution: inner.execution.clone(),
            backend: Backend::Direct(Box::new(inner)),
            bindings: vec![],
            source_hashes: BTreeMap::new(),
            document: String::new(),
            revisions: BTreeMap::new(),
            ready: false,
            busy: true,
            submitted: false,
            retain_sent: false,
        }
    }
    /// Start an ordinary project-host kernel.
    pub fn start(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        python: String,
        name: String,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        Self::start_on(plan, remote, python, name, None, wake)
    }
    /// Start a foreground kernel on the selected execution target.
    pub fn start_on(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        python: String,
        name: String,
        target: Option<crate::slurm::Target>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        validate_name(&name)?;
        let argv = argv(&plan, &python, target.as_ref())?;
        let session = process::Session::start(
            process::Launch {
                native_debug: false,
                argv,
                plan,
                remote,
                python: if target.is_some() || python == "off" {
                    "python3".into()
                } else {
                    python.clone()
                },
                python_program: target.is_none(),
                line_limit: 2 * 1024 * 1024,
            },
            wake,
        )?;
        Ok(Self::direct(
            rode_runtime_core::notebook::Kernel::from_session(
                session.into_runtime(),
                python,
                name,
                target
                    .as_ref()
                    .map(|t| format!("Slurm job {} · starting", t.id))
                    .unwrap_or_else(|| "Project host".into()),
                Duration::from_secs(if target.is_some() { 60 } else { 30 }),
            )?,
        ))
    }
    /// Create host-owned execution with explicit persistence.
    pub fn start_persistent(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        python: String,
        name: String,
        target: Option<crate::slurm::Target>,
        document: String,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        Self::start_managed(plan, remote, python, name, target, document, true, wake)
    }
    /// Start a host controller with persistence that can be changed without a restart.
    #[allow(clippy::too_many_arguments)]
    pub fn start_managed(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        python: String,
        name: String,
        target: Option<crate::slurm::Target>,
        document: String,
        persistent: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        validate_name(&name)?;
        ensure!(
            !persistent || !document.is_empty(),
            "Save this notebook before starting a persistent kernel"
        );
        let spec = service::Spec {
            sources: BTreeMap::new(),
            id: rode_session::new_id()?,
            name: format!(
                "Notebook · {}",
                std::path::Path::new(&document)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ),
            root: plan.root.clone(),
            policy: String::new(),
            mode: String::new(),
            document,
            argv: argv(&plan, &python, target.as_ref())?,
            prefix: vec![],
            env: BTreeMap::new(),
            python: if python == "off" {
                "python3".into()
            } else {
                python.clone()
            },
            python_program: target.is_none(),
            persistent,
            target: target.map(serde_json::to_value).transpose()?,
            start: service::Start::Notebook { name: name.clone() },
        };
        let host = runtime::Client::start(plan, remote, spec, wake)?;
        Ok(Self::host(host, python, name))
    }
    fn host(host: runtime::Client, python: String, name: String) -> Self {
        Self {
            backend: Backend::Host(Box::new(host)),
            bindings: vec![],
            source_hashes: BTreeMap::new(),
            document: String::new(),
            revisions: BTreeMap::new(),
            ready: false,
            busy: true,
            submitted: false,
            retain_sent: false,
            status: "Connecting to host kernel…".into(),
            python,
            name,
            execution: "Connecting…".into(),
        }
    }
    /// Reattach without executing a cell or resetting Python state.
    pub fn attach(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        info: service::Info,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        let python = info.python.clone();
        let name = info.configuration.clone();
        Ok(Self::host(
            runtime::Client::attach(plan, remote, info, wake)?,
            python,
            name,
        ))
    }
    /// Current cell identities and source snapshots, used to reject obsolete replay.
    pub fn bind(&mut self, runs: Vec<Run>) {
        self.retain_sent = false;
        self.source_hashes = runs
            .iter()
            .map(|run| (run.cell.clone(), crate::files::digest(run.code.as_bytes())))
            .collect();
        self.bindings = runs;
    }
    /// Whether source versions changed since the last replay binding.
    pub fn needs_bind(&self, versions: &[(u64, u64)]) -> bool {
        self.bindings
            .iter()
            .map(|run| (run.uid, run.version))
            .ne(versions.iter().copied())
    }
    /// Keep host discovery aligned with an explicit notebook save/rename.
    pub fn bind_document(&mut self, path: &str) {
        if !path.is_empty()
            && self.document != path
            && let Backend::Host(host) = &mut self.backend
            && (host.info.as_ref().is_some_and(|i| i.document == path)
                || host.send(Command::Document { path: path.into() }).is_ok())
        {
            self.document = path.into();
        }
    }
    /// Submit one bounded queue. Host-owned queues keep executing after detachment.
    pub fn run(&mut self, runs: Vec<Run>) -> Result<()> {
        match &mut self.backend {
            Backend::Direct(k) => k.run(runs),
            Backend::Host(host) => {
                ensure!(
                    !self.busy || !self.submitted,
                    "A notebook run is already in progress"
                );
                ensure!(
                    host.info.as_ref().is_none_or(|i| i.alive),
                    "Kernel ended; shut it down before starting a new kernel"
                );
                host.send(Command::Run { runs })?;
                self.busy = true;
                self.submitted = true;
                self.retain_sent = false;
                Ok(())
            }
        }
    }
    /// Drop obsolete queued snapshots without interrupting the active execution.
    pub fn retain_queued(&mut self, mut keep: impl FnMut(&Run) -> bool) {
        let bindings = &self.bindings;
        let source_hashes = &self.source_hashes;
        match &mut self.backend {
            Backend::Direct(k) => k.retain_queued(keep),
            Backend::Host(host) => {
                if !self.retain_sent
                    && let Some(state) = &host.state
                    && state.runs.iter().any(|run| {
                        bindings
                            .iter()
                            .find(|r| {
                                r.cell == run.cell
                                    && source_hashes.get(&run.cell) == Some(&run.source)
                            })
                            .is_none_or(|r| !keep(r))
                    })
                {
                    let cells = self
                        .bindings
                        .iter()
                        .map(|r| (r.cell.clone(), r.code.clone()))
                        .collect();
                    self.retain_sent = host.send(Command::Retain { cells }).is_ok();
                }
            }
        }
    }
    /// Whether initialization or execution is pending.
    pub fn busy(&self) -> bool {
        self.busy
    }
    /// Current executing cell in this document, if still source-compatible.
    pub fn running_uid(&self) -> Option<u64> {
        match &self.backend {
            Backend::Direct(k) => k.running_uid(),
            Backend::Host(host) => host
                .state
                .as_ref()?
                .runs
                .first()
                .and_then(|run| {
                    self.bindings.iter().find(|binding| {
                        binding.cell == run.cell
                            && self.source_hashes.get(&run.cell) == Some(&run.source)
                    })
                })
                .map(|run| run.uid),
        }
    }
    fn bound(&self, run: &Run) -> Option<Run> {
        self.bindings
            .iter()
            .find(|current| current.cell == run.cell && current.code == run.code)
            .cloned()
    }
    /// Interrupt only this kernel.
    pub fn interrupt(&mut self) -> Result<()> {
        match &mut self.backend {
            Backend::Direct(k) => k.interrupt(),
            Backend::Host(host) => host.send(Command::Interrupt),
        }
    }
    /// Clear retained host output without changing Python state.
    pub fn clear_outputs(&mut self, cell: Option<String>) -> Result<()> {
        if let Backend::Host(host) = &mut self.backend {
            host.send(Command::ClearOutputs { cell })?;
        }
        Ok(())
    }
    /// Explicitly shut down the kernel; dropping a persistent view only detaches.
    pub fn stop(&mut self) {
        match &mut self.backend {
            Backend::Direct(k) => k.stop(),
            Backend::Host(host) => host.end(),
        }
        self.busy = false;
        self.status = "Stopped".into();
    }
    /// Drop the view according to the host's acknowledged persistence setting.
    pub fn persistent(&self) -> bool {
        matches!(&self.backend,Backend::Host(host) if host.persistent())
    }
    /// Whether this view controls host-owned execution.
    pub fn host_owned(&self) -> bool {
        matches!(&self.backend, Backend::Host(_))
    }
    /// Whether startup/submission/persistence acknowledgement still prevents safe exit.
    pub fn pending(&self) -> bool {
        matches!(&self.backend,Backend::Host(host) if host.pending())
    }
    /// Change host-owned persistence. Foreground kernels must be restarted to opt in.
    pub fn set_persistent(&mut self, on: bool) -> Result<()> {
        match &mut self.backend {
            Backend::Host(host) => host.set_persistent(on),
            Backend::Direct(_) => {
                anyhow::bail!("Shut down this foreground kernel before enabling persistence")
            }
        }
    }
    /// Native identity for discovery.
    pub fn native_id(&self) -> Option<&str> {
        match &self.backend {
            Backend::Host(host) => host.info.as_ref().map(|i| i.id.as_str()),
            _ => None,
        }
    }
    /// Drain bounded controller updates and remap replays to unchanged stable cells.
    pub fn poll(&mut self) -> Vec<Event> {
        if let Backend::Direct(kernel) = &mut self.backend {
            let events = kernel.poll();
            self.status = kernel.status.clone();
            self.python = kernel.python.clone();
            self.execution = kernel.execution.clone();
            self.busy = kernel.busy();
            return events;
        }
        let Backend::Host(host) = &mut self.backend else {
            unreachable!()
        };
        let updates = host.poll();
        let failure = host.error.take();
        let pending = host.pending();
        let mut events = vec![];
        for update in updates {
            match update {
                Update::State(state) => {
                    self.busy = state.info.busy || pending;
                    self.status = state.info.status.clone();
                    self.python = state.python.clone();
                    self.execution = state.execution.clone();
                    if !self.ready && !state.language.is_null() {
                        self.ready = true;
                        events.push(Event::Ready(state.language.clone()));
                    }
                    if !state.info.alive && !state.error.is_empty() {
                        events.push(Event::Error(state.error));
                    }
                }
                Update::Cell(cell) => {
                    if self.revisions.get(&cell.run.cell) == Some(&cell.revision) {
                        continue;
                    }
                    self.revisions.insert(cell.run.cell.clone(), cell.revision);
                    let Some(run) = self.bound(&cell.run) else {
                        continue;
                    };
                    events.push(Event::Clear(run.clone()));
                    for output in cell.outputs {
                        events.push(Event::Output {
                            run: run.clone(),
                            output: output.value,
                            update: false,
                            display_id: output.display,
                        });
                    }
                    if let Some(count) = cell.count {
                        events.push(Event::Count(run.clone(), count));
                    }
                    if let Some(status) = cell.done {
                        events.push(Event::Done(run, status));
                    }
                }
                Update::Error(error) => events.push(Event::Error(error)),
                _ => {}
            }
        }
        if let Some(error) = failure {
            events.push(Event::Error(error));
        }
        events
    }
}
fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name
                .chars()
                .all(|s| s.is_ascii_alphanumeric() || "._-".contains(s)),
        "Invalid kernelspec name"
    );
    Ok(())
}
fn argv(
    plan: &sandbox::Plan,
    python: &str,
    target: Option<&crate::slurm::Target>,
) -> Result<Vec<String>> {
    if let Some(target) = target {
        target.notebook_argv(plan, python)
    } else {
        Ok(vec![
            "python3".into(),
            "-u".into(),
            "-c".into(),
            include_str!("../notebook_kernel.py").into(),
        ])
    }
}

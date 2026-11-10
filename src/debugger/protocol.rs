//! Desktop views of shared foreground or host-owned debugger controllers.
use crate::{process, runtime, sandbox, ssh};
use anyhow::{Result, ensure};
pub use rode_runtime_core::debugger::protocol::{Phase, Verified};
use rode_runtime_core::{
    debugger::{Settings, Start},
    service::{self, Command, DebugAction, Update},
};
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    sync::{Arc, atomic::AtomicBool},
};
/// Debugger presentation and its disposable controlling attachment.
#[derive(Default)]
pub struct Client {
    inner: rode_runtime_core::debugger::protocol::Client,
    host: Option<runtime::Client>,
    last_navigation: Option<Navigation>,
}

#[derive(PartialEq, Eq)]
struct Navigation {
    epoch: u64,
    frame: Option<i64>,
    location: (String, usize, usize),
}
impl Deref for Client {
    type Target = rode_runtime_core::debugger::protocol::Client;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl DerefMut for Client {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
impl Client {
    /// Start a foreground debugger on the project host.
    pub fn start(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        file: String,
        python: String,
        settings: Settings,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        Self::start_on(plan, remote, file, python, settings, None, false, wake)
    }
    /// Start the whole coordinator, adapter and debuggee on the selected target.
    #[allow(clippy::too_many_arguments)]
    pub fn start_on(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        file: String,
        python: String,
        settings: Settings,
        target: Option<crate::slurm::Target>,
        persistent: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        Self::start_inner(
            plan,
            remote,
            file,
            python,
            settings,
            target,
            persistent,
            persistent,
            BTreeMap::new(),
            wake,
        )
    }
    /// Start a host controller whose lifetime can be changed without restarting the target.
    #[allow(clippy::too_many_arguments)]
    pub fn start_managed(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        file: String,
        python: String,
        settings: Settings,
        target: Option<crate::slurm::Target>,
        persistent: bool,
        sources: BTreeMap<String, String>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        Self::start_inner(
            plan, remote, file, python, settings, target, persistent, true, sources, wake,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn start_inner(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        file: String,
        python: String,
        settings: Settings,
        target: Option<crate::slurm::Target>,
        persistent: bool,
        managed: bool,
        sources: BTreeMap<String, String>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        settings.validate()?;
        ensure!(
            !plan.active || !settings.config.attach,
            "Attach is available in normal mode. An existing process cannot be moved into a project sandbox."
        );
        let start = Start {
            root: plan.root.clone(),
            file: file.clone(),
            python: python.clone(),
            config: settings.config.clone(),
        };
        let argv = if let Some(target) = &target {
            target.debugger_argv(&plan, &python)?
        } else if let Some(helper) = &plan.native_helper {
            vec![helper.clone(), "--debug-helper".into()]
        } else {
            vec![
                rode_host_core::command::current_executable()?
                    .to_string_lossy()
                    .into(),
                "--debug-helper".into(),
            ]
        };
        if managed {
            let spec = service::Spec {
                sources,
                id: rode_session::new_id()?,
                name: format!(
                    "Debugger · {}",
                    std::path::Path::new(&file)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ),
                root: plan.root.clone(),
                policy: String::new(),
                mode: String::new(),
                document: file,
                argv,
                prefix: vec![],
                env: BTreeMap::new(),
                python,
                python_program: false,
                persistent,
                target: target.map(serde_json::to_value).transpose()?,
                start: service::Start::Debugger {
                    launch: Box::new(start),
                    settings: Box::new(settings.clone()),
                },
            };
            return Ok(Self {
                inner: rode_runtime_core::debugger::protocol::Client::mirror(settings),
                host: Some(runtime::Client::start(plan, remote, spec, wake)?),
                last_navigation: None,
            });
        }
        let session = process::Session::start(
            process::Launch {
                argv,
                plan,
                remote,
                python,
                python_program: false,
                line_limit: 2 * 1024 * 1024,
                native_debug: target.is_none(),
            },
            wake,
        )?;
        Self::from_session(session, start, settings)
    }
    /// Adopt a prepared foreground coordinator transport.
    pub fn from_session(
        session: process::Session,
        start: Start,
        settings: Settings,
    ) -> Result<Self> {
        Ok(Self {
            inner: rode_runtime_core::debugger::protocol::Client::from_session(
                session.into_runtime(),
                start,
                settings,
            )?,
            host: None,
            last_navigation: None,
        })
    }
    /// Reattach with the original immutable project and policy.
    pub fn attach(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        info: service::Info,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        Ok(Self {
            inner: rode_runtime_core::debugger::protocol::Client::mirror(Settings::default()),
            host: Some(runtime::Client::attach(plan, remote, info, wake)?),
            last_navigation: None,
        })
    }
    /// Current stopped epoch.
    pub fn inspection_epoch(&self) -> u64 {
        self.inner.inspection_epoch()
    }
    /// Shared foreground cleanup flag.
    pub fn cleanup(&self) -> Arc<AtomicBool> {
        self.host
            .as_ref()
            .map_or_else(|| self.inner.cleanup(), runtime::Client::cleanup)
    }
    /// Whether scoped foreground shutdown is pending.
    pub fn is_stopping(&self) -> bool {
        self.host
            .as_ref()
            .map_or_else(|| self.inner.is_stopping(), runtime::Client::is_stopping)
    }
    /// Whether protocol or host acknowledgement work needs polling.
    pub fn pending(&self) -> bool {
        self.host
            .as_ref()
            .map_or_else(|| self.inner.pending(), runtime::Client::pending)
    }
    /// Acknowledged host lifetime.
    pub fn persistent(&self) -> bool {
        self.host.as_ref().is_some_and(runtime::Client::persistent)
    }
    /// Native identity for running-list selection.
    pub fn native_id(&self) -> Option<&str> {
        self.host.as_ref()?.info.as_ref().map(|i| i.id.as_str())
    }
    /// Whether authoritative debugger replay has arrived.
    pub fn attached_ready(&self) -> bool {
        self.host
            .as_ref()
            .and_then(|h| h.state.as_ref())
            .is_some_and(|s| s.debugger.is_some())
    }
    /// Whether startup or a lifetime acknowledgement must finish before leaving.
    pub fn lifetime_pending(&self) -> bool {
        self.host.as_ref().is_some_and(runtime::Client::pending)
    }
    /// Change persistence without restarting the debugger.
    pub fn set_persistent(&mut self, on: bool) -> Result<()> {
        self.host
            .as_mut()
            .ok_or_else(|| {
                anyhow::anyhow!("Stop this foreground debugger before enabling persistence")
            })?
            .set_persistent(on)
    }
    fn action(&mut self, operation: DebugAction) -> Result<()> {
        let epoch = self.inner.inspection_epoch();
        self.host.as_mut().unwrap().send(Command::Debug {
            epoch,
            operation: Box::new(operation),
        })
    }
    /// Continue, pause or step in the current stopped epoch.
    pub fn control(&mut self, command: &str) -> Result<()> {
        if self.host.is_some() {
            self.action(DebugAction::Control {
                command: command.into(),
            })
        } else {
            self.inner.control(command)
        }
    }
    /// Select a paused thread.
    pub fn select_thread(&mut self, id: i64) -> Result<()> {
        if self.host.is_some() {
            self.action(DebugAction::Thread { id })
        } else {
            self.inner.select_thread(id)
        }
    }
    /// Select a paused frame.
    pub fn select_frame(&mut self, id: i64) -> Result<()> {
        if self.host.is_some() {
            self.action(DebugAction::Frame { id })
        } else {
            self.inner.select_frame(id)
        }
    }
    /// Expand a current-epoch variable reference.
    pub fn load_variables(&mut self, reference: i64) -> Result<()> {
        if self.host.is_some() {
            self.action(DebugAction::Variables { reference })
        } else {
            self.inner.load_variables(reference)
        }
    }
    /// Evaluate in the selected frame.
    pub fn evaluate(&mut self, expression: &str) -> Result<()> {
        if self.host.is_some() {
            self.action(DebugAction::Evaluate {
                expression: expression.into(),
            })
        } else {
            self.inner.evaluate(expression)
        }
    }
    /// Synchronize the saved watch configuration.
    pub fn refresh_watches(&mut self) -> Result<()> {
        if self.host.is_some() {
            self.action(DebugAction::Configure {
                settings: self.inner.settings.clone(),
                paths: vec![],
                watches: true,
            })
        } else {
            self.inner.refresh_watches()
        }
    }
    /// Synchronize a source's breakpoint configuration.
    pub fn sync_breakpoints(&mut self, path: &str) -> Result<()> {
        if self.host.is_some() {
            self.action(DebugAction::Configure {
                settings: self.inner.settings.clone(),
                paths: vec![path.into()],
                watches: false,
            })
        } else {
            self.inner.sync_breakpoints(path)
        }
    }
    /// Poll authoritative host state without replaying initialization or launch.
    pub fn poll(&mut self) {
        let Some(host) = &mut self.host else {
            self.inner.poll();
            return;
        };
        let updates = host.poll();
        let error = host.error.take();
        let offline = host.offline();
        for update in updates {
            match update {
                Update::State(state) => {
                    if let Some(view) = state.debugger {
                        self.restore_host_view(view);
                    }
                }
                Update::Terminal(event) => self.inner.terminal_events.push_back(event),
                Update::Error(error) => self.inner.error = error,
                _ => {}
            }
        }
        if let Some(error) = error {
            self.inner.error = error;
        }
        if offline {
            self.inner.phase = Phase::Ended;
            self.inner.status = "Debugger view disconnected · Reattach from Sessions".into();
        }
    }
    fn restore_host_view(&mut self, view: rode_runtime_core::debugger::protocol::View) {
        let same = self.inner.inspection_epoch() == view.epoch && self.inner.frame == view.frame;
        let pending = if same && view.phase == Phase::Paused {
            self.inner.navigation.take()
        } else {
            None
        };
        self.inner.restore_view(view);
        let navigation = self
            .inner
            .navigation
            .take()
            .filter(|_| self.inner.phase == Phase::Paused);
        if let Some(location) = navigation {
            let key = Navigation {
                epoch: self.inner.inspection_epoch(),
                frame: self.inner.frame,
                location,
            };
            if self.last_navigation.as_ref() != Some(&key) {
                self.inner.navigation = Some(key.location.clone());
                self.last_navigation = Some(key);
                return;
            }
        }
        // Repeated snapshots must not erase a jump before poll_debugger consumes it.
        // Once consumed, the same location must not steal focus back from other tools.
        self.inner.navigation = pending;
    }
    /// End only this host debugger and its owned debuggee; attached targets detach.
    pub fn stop(&mut self) {
        if let Some(host) = &mut self.host {
            host.end();
        }
        self.inner.stop();
    }
    /// Leave a persistent debugger running while disposing its desktop controls.
    pub fn detach(&mut self) {
        if self.persistent() {
            self.host = None;
            self.inner.stop();
        } else {
            self.stop();
        }
    }
    /// Input handle for a foreground or reattached debugger terminal.
    pub fn writer(&self) -> Option<rode_runtime_core::process::Writer> {
        if let Some(host) = &self.host {
            Some(rode_runtime_core::process::Writer::new(TerminalSink(
                host.writer()?,
            )))
        } else {
            self.inner.writer()
        }
    }
}
struct TerminalSink(process::Writer);
impl rode_runtime_core::process::Sink for TerminalSink {
    fn try_send_buffer(&self, bytes: &mut Vec<u8>) -> Result<bool> {
        let command = serde_json::from_slice(bytes)?;
        let message = service::Envelope {
            id: rode_session::new_id()?,
            command: Command::Debug {
                epoch: 0,
                operation: Box::new(DebugAction::Terminal { command }),
            },
        };
        if self.0.try_send(&serde_json::to_value(message)?)? {
            bytes.clear();
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rode_runtime_core::debugger::protocol::View;

    fn paused(epoch: u64, frame: i64, row: Option<usize>) -> View {
        let mut view = rode_runtime_core::debugger::protocol::Client::default().view();
        view.phase = Phase::Paused;
        view.epoch = epoch;
        view.frame = Some(frame);
        view.navigation = row.map(|row| ("/project/main.py".into(), row, 1));
        view
    }

    #[test]
    fn batched_host_snapshots_keep_navigation_until_the_ui_consumes_it() {
        let mut client = Client::default();
        let view = paused(1, 41, Some(3));
        client.restore_host_view(view.clone());
        client.restore_host_view(view.clone());
        client.restore_host_view(paused(1, 41, None));
        assert_eq!(client.navigation.take(), view.navigation);
        client.restore_host_view(view);
        assert!(
            client.navigation.is_none(),
            "Repeated snapshots stole focus back"
        );
    }

    #[test]
    fn a_location_arriving_after_its_frame_is_still_delivered() {
        let mut client = Client::default();
        client.restore_host_view(paused(1, 41, Some(3)));
        client.navigation.take();
        // The adapter reuses its frame ID for a later stop; state and source arrive separately.
        client.restore_host_view(paused(2, 41, None));
        let view = paused(2, 41, Some(4));
        client.restore_host_view(view.clone());
        assert_eq!(client.navigation.take(), view.navigation);
        client.restore_host_view(paused(2, 42, Some(4)));
        assert!(
            client.navigation.take().is_some(),
            "Selecting another frame must navigate"
        );
        client.restore_host_view(paused(2, 42, Some(5)));
        assert_eq!(client.navigation.take().unwrap().1, 5);
    }

    #[test]
    fn resume_and_frame_changes_discard_unconsumed_source_jumps() {
        let mut client = Client::default();
        client.restore_host_view(paused(1, 41, Some(3)));
        let mut running = paused(2, 41, Some(3));
        running.phase = Phase::Running;
        client.restore_host_view(running);
        assert!(client.navigation.is_none());
        client.restore_host_view(paused(3, 41, Some(3)));
        client.restore_host_view(paused(3, 42, None));
        assert!(client.navigation.is_none());
        client.restore_host_view(paused(3, 42, Some(4)));
        client.restore_host_view(paused(4, 42, None));
        assert!(client.navigation.is_none());
    }
}

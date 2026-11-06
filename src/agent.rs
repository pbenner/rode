//! Project-host preparation and transport for the shared headless agent controller.
pub mod background;
pub mod controls;
pub mod history;
pub mod personality;
pub mod protocol;
use crate::{sandbox, ssh};
use anyhow::Result;
pub use personality::Personality;
pub use rode_agent_core::network::Network;
pub use rode_agent_core::prompt_history;
pub use rode_agent_core::{Access, Approval, Config, LINE_LIMIT, PROMPT_LIMIT, Provider};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};
/// Agent launch context captured from the active workspace.
#[derive(Clone)]
pub struct Launch {
    /// Agent protocol/provider settings.
    pub config: Config,
    /// Actual project-host sandbox plan.
    pub plan: sandbox::Plan,
    /// Existing SSH connection, or local execution when absent.
    pub remote: Option<ssh::Remote>,
    /// Selected project-host Python interpreter for transport helpers.
    pub python: String,
}

impl Launch {
    /// Read older prompts without resuming or changing the live agent's session.
    pub fn prompt_history(
        &self,
        thread: &str,
        known: Vec<String>,
        wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<prompt_history::Reader> {
        prompt_history::Reader::open(self.core(), thread, known, wake)
    }
    /// Adapt the captured desktop host context to the headless controller through an injected transport factory.
    fn core(&self) -> rode_agent_core::Launch {
        rode_agent_core::Launch {
            config: self.config.clone(),
            plan: rode_agent_core::Plan {
                root: self.plan.root.clone(),
                active: self.plan.active,
                project_read_only: self.plan.project_read_only,
                prefix: self.plan.prefix.clone(),
            },
            remote: self.remote.is_some(),
            python: self.python.clone(),
            factory: Arc::new(HostFactory {
                launch: self.clone(),
                executables: Mutex::new(vec![]),
            }),
        }
    }
}
/// Desktop local/SSH transport factory that preserves the captured project-host policy.
struct HostFactory {
    launch: Launch,
    // Weak references protect active/retiring transports without retaining
    // their controllers or accumulating completed history/recovery sessions.
    executables: Mutex<Vec<Weak<crate::process::ExecutablePaths>>>,
}
impl rode_agent_core::Factory for HostFactory {
    fn command_tool(
        &self,
        arguments: &Value,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Value> {
        anyhow::ensure!(
            self.launch.remote.is_none(),
            "Offline remote commands require the native RODE host service"
        );
        rode_host_core::agent_commands::run(
            &self.launch.plan,
            arguments,
            &std::env::vars().collect(),
            cancel,
        )
    }
    fn file_tool(
        &self,
        tool: &str,
        arguments: &Value,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Value> {
        anyhow::ensure!(
            self.launch.remote.is_none(),
            "File-only remote editing requires the native RODE host service"
        );
        let mut executables = self
            .executables
            .lock()
            .map_err(|_| anyhow::anyhow!("Agent executable identity lock failed"))?;
        executables.retain(|program| program.strong_count() != 0);
        let mut protected_paths = Vec::new();
        for program in executables.iter().filter_map(Weak::upgrade) {
            let paths = program
                .get()
                .ok_or_else(|| anyhow::anyhow!("Agent executable identity is not ready"))?;
            for path in paths {
                anyhow::ensure!(
                    path.is_absolute(),
                    "Agent executable identity is unresolved; configure an absolute agent path"
                );
                protected_paths.push(path.to_string_lossy().into_owned());
            }
        }
        anyhow::ensure!(
            !protected_paths.is_empty(),
            "No active agent executable identity is available"
        );
        drop(executables);
        let mut plan = self.launch.plan.clone();
        plan.environment.mise = false;
        let (mut command, _lease) = plan.coordinator_command("--agent-file-tool")?;
        command.current_dir(&plan.root);
        let request = rode_host_core::agent_files::Request {
            root: self.launch.plan.root.clone(),
            tool: tool.into(),
            arguments: arguments.clone(),
            read_only: self.launch.plan.project_read_only,
            protected_paths,
            protected_configuration: rode_host_core::agent_files::configuration_paths(
                std::path::Path::new(&plan.root),
                std::env::var("CODEX_HOME").ok().as_deref(),
                std::env::var("HOME").ok().as_deref(),
                &plan.prefix,
            )?,
        };
        let output = crate::project::run_controlled(
            command,
            serde_json::to_vec(&request)?,
            std::time::Duration::from_secs(10),
            cancel,
            false,
        )?
        .checked()?;
        rode_host_core::agent_files::response(&output)
    }
    fn start(
        &self,
        launch: &rode_agent_core::Launch,
        history: bool,
        wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<rode_agent_core::Session> {
        let mut host = self.launch.clone();
        host.config = launch.config.clone();
        if host.config.native_tools() {
            host.plan.environment.mise = false;
        }
        let session = if history {
            Session::history(host, wake)?
        } else {
            Session::start(host, wake)?
        };
        let mut executables = self
            .executables
            .lock()
            .map_err(|_| anyhow::anyhow!("Agent executable identity lock failed"))?;
        executables.retain(|program| program.strong_count() != 0);
        executables.push(Arc::downgrade(&session.0.executable()));
        Ok(rode_agent_core::Session::new(session))
    }
}

pub use crate::process::Event;

/// Owned agent process using the core supervised stdio transport. Drop stops the owned process
/// tree.
pub struct Session(crate::process::Session);
impl rode_agent_core::Transport for Session {
    fn send(&self, message: &Value) -> Result<()> {
        self.send(message)
    }
    fn poll(&self) -> Option<Event> {
        self.poll()
    }
    fn stop(&self) {
        self.stop();
    }
}
impl Session {
    /// Validate agent settings and launch supervision; observe process events for asynchronous
    /// startup errors.
    pub fn start(launch: Launch, wake: std::sync::Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let argv = launch.config.argv()?;
        Self::with_arguments(launch, argv, LINE_LIMIT, wake)
    }
    /// Start a separately supervised metadata/history connection with its smaller line budget;
    /// this path omits conversation-start arguments and never submits a prompt.
    pub(crate) fn history(
        launch: Launch,
        wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        let argv = launch.config.arguments(false)?;
        Self::with_arguments(launch, argv, 2 * 1024 * 1024, wake)
    }
    /// Start the shared supervised process transport using captured arguments and protocol line bound.
    fn with_arguments(
        mut launch: Launch,
        argv: Vec<String>,
        limit: usize,
        wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        if launch.config.native_tools() {
            launch.plan.environment.mise = false;
        }
        launch.plan.prefix = rode_agent_core::Plan {
            root: launch.plan.root.clone(),
            active: launch.plan.active,
            project_read_only: launch.plan.project_read_only,
            prefix: launch.plan.prefix.clone(),
        }
        .controller_prefix(launch.config.network)?;
        crate::process::Session::start(
            crate::process::Launch {
                native_debug: false,
                argv,
                plan: launch.plan,
                remote: launch.remote,
                python: launch.python,
                python_program: false,
                line_limit: limit,
            },
            wake,
        )
        .map(Self)
    }
    /// Queue one bounded JSON-RPC message without blocking the UI.
    pub fn send(&self, message: &Value) -> Result<()> {
        self.0.send(message)
    }
    /// Take an available transport event without waiting.
    pub fn poll(&self) -> Option<Event> {
        self.0.poll()
    }
    /// Request shutdown of this agent's owned process tree.
    pub fn stop(&self) {
        self.0.stop();
    }
}

/// Agent configuration keyed by workspace identity; excludes transcripts and credentials.
pub type Preferences = BTreeMap<String, Config>;

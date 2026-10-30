//! Project-host preparation and transport for the shared headless agent controller.
pub mod background;
pub mod controls;
pub mod history;
pub mod personality;
pub mod protocol;
use crate::{sandbox, ssh};
use anyhow::Result;
pub use personality::Personality;
pub use rode_agent_core::{Access, Approval, Config, LINE_LIMIT, PROMPT_LIMIT, Provider};
use serde_json::Value;
use std::collections::BTreeMap;
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
            factory: std::sync::Arc::new(HostFactory(self.clone())),
        }
    }
}
struct HostFactory(Launch);
impl rode_agent_core::Factory for HostFactory {
    fn start(
        &self,
        launch: &rode_agent_core::Launch,
        history: bool,
        wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<rode_agent_core::Session> {
        let mut host = self.0.clone();
        host.config = launch.config.clone();
        let session = if history {
            Session::history(host, wake)?
        } else {
            Session::start(host, wake)?
        };
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
    pub(crate) fn history(
        launch: Launch,
        wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        let argv = launch.config.arguments(false)?;
        Self::with_arguments(launch, argv, 2 * 1024 * 1024, wake)
    }
    fn with_arguments(
        launch: Launch,
        argv: Vec<String>,
        limit: usize,
        wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
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

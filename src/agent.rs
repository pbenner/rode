//! Project-host agent processes. Both protocols use bounded JSON-RPC over stdio;
//! the full process tree is launched inside the already-probed project sandbox.
pub mod controls;
pub mod history;
pub mod personality;
pub mod protocol;
pub use personality::Personality;

use crate::{sandbox, ssh};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Default inbound chat JSON line limit; explicit history connections allow 2 MiB.
pub const LINE_LIMIT: usize = 256 * 1024;
/// Maximum submitted chat prompt size in bytes.
pub const PROMPT_LIMIT: usize = 32 * 1024;

/// Supported installed agent protocols/providers; selection never starts a process.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// Codex app-server using its configured provider and credentials.
    #[default]
    Codex,
    /// Codex app-server using an explicitly configured local Responses endpoint.
    Local,
    /// An installed Agent Client Protocol v1 executable.
    Acp,
}

/// How the client handles requests to exceed an agent's existing tool permissions.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    /// Present the agent's request for an explicit user decision.
    #[default]
    Ask,
    /// Deny permission requests automatically; already permitted tools can still run.
    Never,
}
impl Approval {
    /// Human-readable selector label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask when needed",
            Self::Never => "Never ask",
        }
    }
    pub(crate) fn codex(self) -> &'static str {
        match self {
            Self::Ask => "on-request",
            Self::Never => "never",
        }
    }
}

/// Codex tool access inside the separately enforced project-host sandbox.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Permit inspection without tool writes or approval-based escalation.
    ReadOnly,
    /// Permit workspace writes and request approval for additional access.
    #[default]
    Project,
    /// Use the host permissions remaining inside the active outer sandbox.
    Full,
}
impl Access {
    /// Human-readable selector label.
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read only",
            Self::Project => "Project access",
            Self::Full => "Full access",
        }
    }
    pub(crate) fn codex(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Project => "workspace-write",
            Self::Full => "danger-full-access",
        }
    }
}
impl Provider {
    /// Human-readable provider name for settings and chat headers.
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Local => "Codex · local model",
            Self::Acp => "Other agent · ACP",
        }
    }
}

/// Persisted host/project agent settings; transcripts and credentials are excluded.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Protocol/provider to launch on the next explicit message.
    pub provider: Provider,
    /// `auto` discovers the optional Codex runtime or an installed project-host executable.
    pub codex: String,
    /// Optional model override for the normal Codex provider.
    pub model: String,
    /// Explicit model name served by the local provider.
    pub local_model: String,
    /// Responses API base URL; localhost refers to the project host, including over SSH.
    pub local_url: String,
    /// Installed ACP agent executable on the project host.
    pub acp_program: String,
    /// Literal ACP executable arguments.
    pub acp_arguments: Vec<String>,
    /// Request read-only agent operation without widening the outer sandbox.
    pub read_only: bool,
    /// Agent tool scope; the outer project sandbox always remains authoritative.
    pub access: Access,
    /// Whether additional permission requests require a user decision or are denied.
    pub approval: Approval,
    /// Optional Codex reasoning effort advertised by the selected model.
    pub reasoning_effort: String,
    /// Explicit ACP selector overrides, keyed by advertised option ID.
    pub acp_options: BTreeMap<String, Value>,
    /// Chat response tone only; does not change tools, execution or permission settings.
    pub personality: Personality,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            provider: Provider::Codex,
            codex: "auto".into(),
            model: String::new(),
            local_model: String::new(),
            local_url: "http://127.0.0.1:11434/v1".into(),
            acp_program: String::new(),
            acp_arguments: vec![],
            read_only: false,
            access: Access::default(),
            approval: Approval::default(),
            reasoning_effort: String::new(),
            acp_options: BTreeMap::new(),
            personality: Personality::Default,
        }
    }
}
impl Config {
    /// Preserve old read-only preferences while applying a new access selector.
    pub fn access(&self) -> Access {
        if self.read_only {
            Access::ReadOnly
        } else {
            self.access
        }
    }

    /// Settings which require another executable/provider process, rather than a turn override.
    pub fn same_process(&self, next: &Self) -> bool {
        self.provider == next.provider
            && self.codex == next.codex
            && self.local_url == next.local_url
            && self.acp_program == next.acp_program
            && self.acp_arguments == next.acp_arguments
    }
    /// Return the model setting appropriate to the selected provider.
    pub fn model(&self) -> &str {
        if self.provider == Provider::Local {
            &self.local_model
        } else {
            &self.model
        }
    }
    /// Validate settings and construct literal process arguments. Does not install an agent,
    /// start inference or perform authentication.
    pub fn argv(&self) -> Result<Vec<String>> {
        self.arguments(true)
    }
    fn arguments(&self, require_model: bool) -> Result<Vec<String>> {
        let mut args = if self.provider == Provider::Acp {
            let mut args = vec![self.acp_program.trim().to_owned()];
            args.extend(self.acp_arguments.clone());
            args
        } else {
            vec![
                match self.codex.trim() {
                    "" | "auto" | "codex" => crate::bundled::AUTO_CODEX.into(),
                    program => program.into(),
                },
                "app-server".into(),
                "--listen".into(),
                "stdio://".into(),
            ]
        };
        ensure!(
            !args[0].is_empty(),
            "Choose an installed agent executable in Agent settings."
        );
        ensure!(
            args.len() <= 64,
            "At most 64 agent arguments are supported."
        );
        ensure!(
            args.iter().all(|v| v.len() <= 4096 && !v.contains('\0')),
            "Invalid agent executable or argument."
        );
        ensure!(
            args.iter().map(String::len).sum::<usize>() <= 16 * 1024,
            "Agent arguments exceed 16 KiB."
        );
        ensure!(
            self.model().len() <= 256 && !self.model().chars().any(char::is_control),
            "Invalid model name."
        );
        ensure!(
            controls::identifier(&self.reasoning_effort, true),
            "Invalid reasoning effort."
        );
        ensure!(
            self.acp_options.len() <= 32
                && self
                    .acp_options
                    .iter()
                    .all(|(key, value)| controls::identifier(key, false)
                        && (value.is_boolean()
                            || value
                                .as_str()
                                .is_some_and(|s| controls::identifier(s, false)))),
            "Invalid ACP option settings."
        );
        if self.provider == Provider::Local {
            ensure!(
                !require_model || !self.local_model.trim().is_empty(),
                "Enter an installed local model name in Agent settings."
            );
            let url = self.local_url.trim().trim_end_matches('/');
            ensure!(
                (url.starts_with("http://") || url.starts_with("https://"))
                    && url.len() <= 2048
                    && !url.chars().any(|c| c.is_control() || c.is_whitespace()),
                "Enter an HTTP model API base URL, such as http://127.0.0.1:11434/v1."
            );
            for (key, value) in [
                ("model_provider", Value::String("rode_local".into())),
                (
                    "model_providers.rode_local.name",
                    Value::String("Local model".into()),
                ),
                (
                    "model_providers.rode_local.base_url",
                    Value::String(url.into()),
                ),
                (
                    "model_providers.rode_local.wire_api",
                    Value::String("responses".into()),
                ),
                (
                    "model_providers.rode_local.requires_openai_auth",
                    Value::Bool(false),
                ),
                (
                    "model_providers.rode_local.supports_websockets",
                    Value::Bool(false),
                ),
                ("web_search", Value::String("disabled".into())),
            ] {
                args.extend(["-c".into(), format!("{key}={value}")]);
            }
        }
        Ok(args)
    }
}

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

pub use crate::process::Event;

/// Owned agent process using the core supervised stdio transport. Drop stops the owned process
/// tree.
pub struct Session(crate::process::Session);
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

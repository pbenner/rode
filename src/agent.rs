//! Project-host agent processes. Both protocols use bounded JSON-RPC over stdio;
//! the full process tree is launched inside the already-probed project sandbox.
pub mod protocol;

use crate::{sandbox, ssh};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Maximum inbound agent JSON line size in bytes.
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
        }
    }
}
impl Config {
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
        if self.provider == Provider::Local {
            ensure!(
                !self.local_model.trim().is_empty(),
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
        crate::process::Session::start(
            crate::process::Launch {
                native_debug: false,
                argv,
                plan: launch.plan,
                remote: launch.remote,
                python: launch.python,
                python_program: false,
                line_limit: LINE_LIMIT,
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

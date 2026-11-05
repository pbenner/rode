//! Headless Codex/Claude/ACP conversation state shared by the GUI and native host service.
//!
//! Protocol state belongs beside the agent process so it can keep handling output,
//! deadlines and existing permission policies without a desktop connection. Host
//! preparation and transport are injected; this crate has no GUI, SSH, filesystem
//! helper, credential manager or model-server dependency.
#![warn(missing_docs)]
pub mod attachment;
pub mod controls;
pub mod file_only;
pub mod history;
mod input_history;
pub mod io;
pub mod network;
pub mod personality;
pub mod prompt_history;
pub mod protocol;
pub mod queue;
pub mod service;
#[cfg(test)]
mod tests;
use anyhow::{Result, ensure};
pub use personality::Personality;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
/// Automatic executable marker resolved by the host adapter before agent execution.
pub const AUTO_CODEX: &str = "@rode/codex";
/// Installed Claude Agent SDK adapter, using ACP v1 over stdio.
pub const CLAUDE_ACP: &str = "claude-agent-acp";
/// Bounded agent JSON line limit, including submitted image content.
pub const LINE_LIMIT: usize = 4 * 1024 * 1024;
/// Maximum diagnostic stderr line size, including its trailing newline.
pub const STDERR_LINE_LIMIT: usize = 1024 * 1024;
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
    /// Anthropic Claude Code through its installed Claude Agent SDK ACP adapter.
    Claude,
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
    /// Translate approval policy to Codex launch terminology without changing outer host grants.
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
    /// Edit through bounded project file tools, with Codex execution tools disabled.
    FilesOnly,
    /// Use the host permissions remaining inside the active outer sandbox.
    Full,
}
impl Access {
    /// Human-readable selector label.
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read only",
            Self::Project => "Project access",
            Self::FilesOnly => "Edit project files only",
            Self::Full => "Full access",
        }
    }
    /// Translate tool access to Codex's inner sandbox mode; the captured host policy still applies.
    pub(crate) fn codex(self) -> &'static str {
        match self {
            Self::ReadOnly | Self::FilesOnly => "read-only",
            Self::Project => "workspace-write",
            Self::Full => "danger-full-access",
        }
    }
}
impl Provider {
    /// Providers offered by the chat selector, in presentation order.
    pub const ALL: [Self; 4] = [Self::Codex, Self::Claude, Self::Local, Self::Acp];

    /// Whether this provider speaks ACP, independently of its executable preset.
    pub fn is_acp(self) -> bool {
        matches!(self, Self::Claude | Self::Acp)
    }

    /// Human-readable provider name for settings and chat headers.
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Local => "Codex · local model",
            Self::Claude => "Claude Code · Anthropic",
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
    /// Installed Claude ACP adapter or wrapper on the project host.
    pub claude_program: String,
    /// Literal arguments for the Claude ACP adapter; never shell-expanded.
    pub claude_arguments: Vec<String>,
    /// Request read-only agent operation without widening the outer sandbox.
    pub read_only: bool,
    /// Agent tool scope; the outer project sandbox always remains authoritative.
    pub access: Access,
    /// Whether additional permission requests require a user decision or are denied.
    pub approval: Approval,
    /// Network scope for agent tools; model-only mode requires an offline project sandbox.
    pub network: network::Network,
    /// Optional Codex reasoning effort advertised by the selected model.
    pub reasoning_effort: String,
    /// Explicit ACP selector overrides, keyed by advertised option ID.
    pub acp_options: BTreeMap<String, Value>,
    /// Claude's advertised model/effort/mode choices, separate from other ACP agents.
    pub claude_options: BTreeMap<String, Value>,
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
            claude_program: CLAUDE_ACP.into(),
            claude_arguments: vec![],
            read_only: false,
            access: Access::default(),
            approval: Approval::default(),
            network: network::Network::default(),
            reasoning_effort: String::new(),
            acp_options: BTreeMap::new(),
            claude_options: BTreeMap::new(),
            personality: Personality::Default,
        }
    }
}
impl Config {
    /// Executable for the selected ACP provider. Blank Claude settings use the preset.
    pub fn acp_program(&self) -> &str {
        if self.provider == Provider::Claude {
            match self.claude_program.trim() {
                "" => CLAUDE_ACP,
                program => program,
            }
        } else {
            self.acp_program.trim()
        }
    }

    /// Literal arguments for the selected ACP provider.
    pub fn acp_arguments(&self) -> &[String] {
        if self.provider == Provider::Claude {
            &self.claude_arguments
        } else {
            &self.acp_arguments
        }
    }

    /// Edit the selected provider's argument list without overwriting another provider.
    pub fn acp_arguments_mut(&mut self) -> &mut Vec<String> {
        if self.provider == Provider::Claude {
            &mut self.claude_arguments
        } else {
            &mut self.acp_arguments
        }
    }

    /// Saved options for the selected ACP provider.
    pub fn acp_options(&self) -> &BTreeMap<String, Value> {
        if self.provider == Provider::Claude {
            &self.claude_options
        } else {
            &self.acp_options
        }
    }

    /// Edit only the selected provider's advertised options.
    pub fn acp_options_mut(&mut self) -> &mut BTreeMap<String, Value> {
        if self.provider == Provider::Claude {
            &mut self.claude_options
        } else {
            &mut self.acp_options
        }
    }

    /// Whether only the host's confined file tools may change the project.
    pub fn files_only(&self) -> bool {
        self.access() == Access::FilesOnly
    }
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
            && self.claude_program == next.claude_program
            && self.claude_arguments == next.claude_arguments
            && self.files_only() == next.files_only()
            && self.network == next.network
            && self.offline_commands() == next.offline_commands()
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
    /// Construct arguments with optional model validation for history browsing.
    pub fn arguments(&self, require_model: bool) -> Result<Vec<String>> {
        ensure!(
            self.network != network::Network::ModelOnly || !self.provider.is_acp(),
            "Model connection only is supported by Codex and Codex local models. This ACP provider cannot enforce offline tools."
        );
        ensure!(
            !self.files_only() || !self.provider.is_acp(),
            "Edit project files only requires Codex. Select another access mode before using Claude Code or another ACP agent."
        );
        let mut args = if self.provider.is_acp() {
            let mut args = vec![self.acp_program().to_owned()];
            args.extend_from_slice(self.acp_arguments());
            args
        } else {
            vec![
                match self.codex.trim() {
                    "" | "auto" | "codex" => crate::AUTO_CODEX.into(),
                    program => program.into(),
                },
                "app-server".into(),
                "--listen".into(),
                "stdio://".into(),
            ]
        };
        if self.native_tools() {
            for setting in file_only::LAUNCH_OVERRIDES {
                args.extend(["-c".into(), (*setting).into()]);
            }
        }
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
            self.acp_options().len() <= 32
                && self
                    .acp_options()
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

/// Permission-relevant parts of the prepared project launch policy.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Plan {
    /// Canonical working folder on the agent host.
    pub root: String,
    /// Whether the outer sandbox actually enforces isolation.
    pub active: bool,
    /// Whether the project is mounted read only.
    pub project_read_only: bool,
    /// Captured sandbox prefix; also identifies shared network access.
    pub prefix: Vec<String>,
}
/// Captured conversation context with an injected host-side process factory.
#[derive(Clone)]
pub struct Launch {
    /// Backend, tools and reply preferences.
    pub config: Config,
    /// Actual project policy.
    pub plan: Plan,
    /// Whether preparation includes a remote runtime deployment.
    pub remote: bool,
    /// Selected interpreter identity, also used to invalidate history work.
    pub python: String,
    /// Host adapter; opening the UI never calls this factory.
    pub factory: Arc<dyn Factory>,
}
/// Bounded transport events consumed by the conversation controller.
#[derive(Debug)]
pub enum Event {
    /// The process is ready for protocol initialization.
    Started,
    /// One complete JSON-RPC message.
    Message(Value),
    /// Bounded stderr text.
    Stderr(String),
    /// Process or transport failure.
    Error(String),
    /// The process exited, with no numeric status for signal termination.
    Exited(Option<i32>),
}
/// Nonblocking process connection. Implementations must keep their spawning
/// supervisor alive while a sandbox child runs and stop only the owned tree.
pub trait Transport: Send {
    /// Queue a bounded JSON-RPC message.
    fn send(&self, message: &Value) -> Result<()>;
    /// Take an event without waiting.
    fn poll(&self) -> Option<Event>;
    /// Request scoped shutdown.
    fn stop(&self);
}
/// Launch adapter for local, SSH and service-owned conversation processes.
pub trait Factory: Send + Sync {
    /// Run a bounded client-owned command under the original offline project
    /// policy. Called only by the model-only tool surface, never by file-only
    /// mode. Unsupported hosts must reject the operation.
    fn command_tool(
        &self,
        _arguments: &Value,
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Value> {
        anyhow::bail!(
            "This host does not support offline agent commands. Start a new chat with an updated RODE helper."
        )
    }
    /// Run one bounded native file operation on the captured project host, inside
    /// its existing policy. Implementations must never accept an executable here.
    /// Observe cancellation before starting work and terminate an interrupted
    /// helper; controllers retain the worker until cleanup has finished.
    fn file_tool(
        &self,
        _tool: &str,
        _arguments: &Value,
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Value> {
        anyhow::bail!(
            "This host does not support confined agent file tools. Start a new chat with an updated RODE helper."
        )
    }
    /// Start only after Send or explicit History. History does not require a model.
    fn start(
        &self,
        launch: &Launch,
        history: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Session>;
}
/// Owned process connection shared by chat and history restoration.
pub struct Session(Box<dyn Transport>);
impl Session {
    /// Adopt an already prepared host transport.
    pub fn new(transport: impl Transport + 'static) -> Self {
        Self(Box::new(transport))
    }
    /// Start a conversation process through its captured host factory.
    pub fn start(launch: Launch, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        launch.plan.controller_prefix(launch.config.network)?;
        launch.factory.start(&launch, false, wake)
    }
    /// Start a history process without model validation or inference.
    pub fn history(launch: Launch, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        launch.plan.controller_prefix(launch.config.network)?;
        launch.factory.start(&launch, true, wake)
    }
    /// Queue one bounded JSON-RPC message.
    pub fn send(&self, message: &Value) -> Result<()> {
        self.0.send(message)
    }
    /// Take one available event.
    pub fn poll(&self) -> Option<Event> {
        self.0.poll()
    }
    /// Request scoped process shutdown.
    pub fn stop(&self) {
        self.0.stop();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

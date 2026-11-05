//! Separate the trusted model controller from client-owned offline tools.
//!
//! Model-only mode disables Codex's built-in execution/remote tools and uses the
//! same verified dynamic file surface as file-only mode, plus a bounded native
//! command tool. Only the controller's launch prefix gains network access; tool
//! hosts retain the original, active offline project policy.
use crate::{Access, Config, Plan};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Agent networking, independent of the project's existing execution policy.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Network {
    /// The complete agent process inherits the project's network policy.
    #[default]
    Project,
    /// Only model/authentication traffic is available; native tools stay offline.
    ModelOnly,
}

impl Network {
    /// Label used in the agent access menu.
    pub fn label(self) -> &'static str {
        match self {
            Self::Project => "Use project network policy",
            Self::ModelOnly => "Model connection only",
        }
    }
}

impl Config {
    /// Native tool surfaces disable built-in execution and all external tools.
    pub fn native_tools(&self) -> bool {
        self.files_only() || self.network == Network::ModelOnly
    }

    /// Read-only and file-only agents never receive a native command tool.
    pub fn offline_commands(&self) -> bool {
        self.network == Network::ModelOnly
            && matches!(self.access(), Access::Project | Access::Full)
    }
}

impl Plan {
    /// Construct a controller-only network exception, never modifying the
    /// captured tool policy. Missing/fallback/network-enabled plans are errors;
    /// this mode must not start a controller on an unenforced offline promise.
    pub fn controller_prefix(&self, network: Network) -> Result<Vec<String>> {
        let mut prefix = self.prefix.clone();
        if network == Network::ModelOnly {
            ensure!(
                self.active && !prefix.is_empty() && !prefix.iter().any(|arg| arg == "--share-net"),
                "Model connection only requires an active project sandbox with Allow network access turned off. Apply those project settings first."
            );
            ensure!(
                prefix.iter().position(|arg| arg == "--") == Some(prefix.len() - 1),
                "Model-only controller requires a literal sandbox policy without a project environment launcher; start a new chat"
            );
            prefix.insert(prefix.len() - 1, "--share-net".into());
        }
        Ok(prefix)
    }
}

/// Add the command surface only when the current access level permits it.
pub(crate) fn tools(config: &Config, read_only: bool) -> Value {
    let mut tools = crate::file_only::tools();
    if config.offline_commands() && !read_only {
        tools.as_array_mut().expect("native tool array").push(json!({
            "type":"function","name":"rode_run_command",
            "description":"Run a shell command in the project on its host under RODE's enforced offline sandbox. No network or permission escalation is available. Output is bounded; long-running interactive sessions are unsupported.",
            "inputSchema":{
                "type":"object", "additionalProperties":false,
                "properties":{
                    "command":{"type":"string", "minLength":1,"maxLength":32768},
                    "timeout_seconds":{"type":"integer","minimum":1,"maximum":120}
                },"required":["command"]
            }
        }));
    }
    tools
}

pub(crate) const INSTRUCTIONS: &str = "RODE model-connection-only mode: use the supplied rode_ file tools for reading and editing project files, and rode_run_command when available for builds, tests, Git and other shell commands. Commands and all their children run on the project host inside its enforced offline sandbox. Network requests, installations needing downloads and permission escalation are unavailable. Built-in shell, apply_patch, web, browser, MCP and plugin tools are disabled. Do not attempt to bypass these restrictions. Ask the user to prepare missing dependencies outside this agent. Read-only access and Edit project files only do not permit commands. File-tool paths are project-relative; read files before edits and use their revisions.";

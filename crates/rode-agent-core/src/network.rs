//! Separate the trusted model controller from client-owned offline tools.
//!
//! Model-only mode disables Codex's built-in execution/remote tools and uses the
//! same verified dynamic file surface as file-only mode, plus a bounded native
//! command tool. Only the controller's launch prefix gains network access; tool
//! hosts retain the original, active offline project policy.
use crate::{Access, Config, Plan, Provider};
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
    /// Whether the captured policy blocks the controller's network access.
    /// Normal-mode fallbacks and the explicit model-only exception are distinct
    /// from an enforced offline controller; diagnostics must not conflate them.
    pub fn controller_network_disabled(&self, network: Network) -> bool {
        self.active
            && !self.prefix.is_empty()
            && !self.prefix.iter().any(|arg| arg == "--share-net")
            && network == Network::Project
    }

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

/// Add the captured offline policy to failures from any provider, without
/// claiming it caused the original error. Provider-specific advice must only
/// offer network modes that RODE can enforce for that provider. Reserve space
/// for that guidance before clipping a long backend error to the display budget.
pub(crate) fn explain_error(
    provider: Provider,
    offline: bool,
    error: &str,
    limit: usize,
) -> std::borrow::Cow<'_, str> {
    let hint = if offline {
        if provider.is_acp() {
            "The project sandbox blocks network access for this agent, including connections to localhost. If this provider requires a model or authentication connection, enable Allow network access in the project sandbox settings, apply them and start a new chat. Model connection only is not supported for this provider. The offline policy is additional context; the original error above may have another cause."
        } else {
            "The project sandbox blocks network access for this agent, including connections to localhost. If it requires a model or authentication connection, choose Access → Agent networking → Model connection only to keep commands offline, then start a new chat. Alternatively, enable Allow network access in the project sandbox settings and apply them. The offline policy is additional context; the original error above may have another cause."
        }
    } else {
        let message = error
            .strip_prefix("Agent: ")
            .or_else(|| error.strip_prefix("Agent history: "))
            .unwrap_or(error)
            .trim();
        if provider != Provider::Codex
            || !matches!(
                message,
                "workspace routing discovery failed" | "workspace routing discovery timed out"
            )
        {
            return error.into();
        }
        "Codex could not complete its ChatGPT account lookup. Check DNS, HTTPS/proxy connectivity and access to required CA certificate files on the project host. This error alone does not identify a sandbox denial."
    };
    if hint.len() + 2 >= limit {
        return crate::protocol::clip(hint, limit).into();
    }
    format!(
        "{}\n\n{hint}",
        crate::protocol::clip(error, limit - hint.len() - 2)
    )
    .into()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_diagnostics_distinguish_controller_access_from_offline_tools() {
        let mut plan = Plan {
            active: true,
            prefix: vec!["landlock".into(), "--".into()],
            project_read_only: true,
            ..Default::default()
        };
        let original = plan.prefix.clone();
        assert!(plan.controller_network_disabled(Network::Project));
        assert!(!plan.controller_network_disabled(Network::ModelOnly));
        assert_eq!(plan.prefix, original);
        let controller = plan.controller_prefix(Network::ModelOnly).unwrap();
        assert!(controller.iter().any(|arg| arg == "--share-net"));
        assert!(!plan.prefix.iter().any(|arg| arg == "--share-net"));

        plan.active = false;
        assert!(!plan.controller_network_disabled(Network::Project));
        plan.active = true;
        plan.prefix = controller;
        assert!(!plan.controller_network_disabled(Network::Project));
        plan.prefix.clear();
        assert!(!plan.controller_network_disabled(Network::Project));

        let error = "workspace routing discovery failed";
        let offline = explain_error(Provider::Codex, true, error, 2048);
        assert!(offline.starts_with(error));
        assert!(offline.contains("Model connection only"));
        let online = explain_error(Provider::Codex, false, error, 2048);
        assert!(online.contains("CA certificate"));
        assert!(!online.contains("sandbox blocks network"));
        for error in [
            "unauthorized",
            "unrelated failure",
            "workspace routing discovery unauthorized (401)",
        ] {
            assert_eq!(explain_error(Provider::Codex, false, error, 2048), error);
        }
        assert_eq!(
            explain_error(
                Provider::Claude,
                false,
                "workspace routing discovery failed",
                2048,
            ),
            "workspace routing discovery failed"
        );
    }

    #[test]
    fn every_offline_provider_gets_supported_actions_without_rewriting_its_error() {
        for provider in [
            Provider::Codex,
            Provider::Local,
            Provider::Claude,
            Provider::Acp,
        ] {
            // Network vocabulary differs between providers. Even a vague exit or
            // timeout must expose the known policy without guessing the cause.
            for error in [
                "API connection error",
                "fetch failed",
                "Agent exited (1)",
                "operation timed out",
                "invalid configuration",
            ] {
                let text = explain_error(provider, true, error, 2048);
                assert!(text.starts_with(&format!("{error}\n\n")));
                assert!(text.contains("sandbox blocks network"));
                assert!(text.contains("localhost"));
                assert!(text.contains("may have another cause"));
                assert_eq!(text.contains("choose Access →"), !provider.is_acp());
                assert_eq!(
                    text.contains("not supported for this provider"),
                    provider.is_acp()
                );
                assert_eq!(explain_error(provider, false, error, 2048), error);
            }
        }
    }

    #[test]
    fn long_unicode_errors_cannot_displace_network_guidance() {
        let error = "A long backend error: ".to_owned() + &"🦀é界".repeat(4096);
        for provider in [
            Provider::Codex,
            Provider::Local,
            Provider::Claude,
            Provider::Acp,
        ] {
            for limit in [2048, 16384] {
                let text = explain_error(provider, true, &error, limit);
                assert!(text.len() <= limit);
                assert!(text.starts_with("A long backend error: "));
                assert!(text.contains("[… truncated]"));
                assert!(text.contains("sandbox blocks network"));
                assert!(text.contains("Allow network access"));
            }
        }
    }
}

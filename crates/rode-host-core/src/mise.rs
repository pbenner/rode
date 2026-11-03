//! Optional installed mise integration. The bundled extension contributes native UI,
//! while this small host launcher resolves an environment inside the captured sandbox.
//! Resolved values live only in the child process and never enter desktop preferences.

use crate::{environment, sandbox};
use anyhow::Result;
pub mod native;
use serde::{Deserialize, Serialize};

/// Explicit project-host operation. Inspection never installs tools or trusts configuration.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// List installed/configured tools and project tasks.
    Inspect,
    /// Explicitly trust the current project's configuration with mise.
    Trust,
    /// Explicitly install declared project tools.
    Install,
}

/// A task contributed by the project's mise configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    /// Literal task name, passed as a single quoted command argument.
    pub name: String,
    /// Bounded description shown beside the action.
    pub description: String,
}

/// Nonsecret metadata returned by a host-side inspection/setup operation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// Installed mise version.
    pub version: String,
    /// Names of the tools selected by the project configuration.
    pub tools: Vec<String>,
    /// Project tasks contributed to the Tasks panel.
    pub tasks: Vec<Task>,
    /// Setup command output, shown only after an explicit action.
    pub output: String,
}

/// Run mise metadata/setup through the same bounded implementation used by the SSH peer.
/// The caller supplies the sandbox boundary and keeps this off the UI thread.
pub fn execute(root: &str, settings: &environment::Settings, action: Action) -> Result<Snapshot> {
    native::execute(root, settings, action)
}

/// Quote a configured mise executable, including the standard user-local fallback for shells.
pub fn shell_program(settings: &environment::Settings) -> String {
    let program = settings.program("mise");
    if program.contains('/') {
        crate::command::quote(&program)
    } else {
        format!(
            "\"$(command -v {} || printf '%s' \"$HOME/.local/bin/\"{})\"",
            crate::command::quote(&program),
            crate::command::quote(&program)
        )
    }
}

/// Reapply the new shell's provider after login startup; the caller activates explicit Python next.
pub fn shell_environment(settings: &environment::Settings, root: &str) -> String {
    if !settings.mise {
        return String::new();
    }
    format!(
        "_rode_mise_env=\"$(MISE_AUTO_INSTALL=0 MISE_EXEC_AUTO_INSTALL=0 MISE_PARANOID=1 {} -C {} env --shell bash)\" && eval \"$_rode_mise_env\"; unset _rode_mise_env;\n",
        shell_program(settings),
        crate::command::quote(root)
    )
}

impl sandbox::Plan {
    /// Environment provider launcher selected with the captured host backend.
    pub fn environment_prefix(&self, python: &str) -> Result<Vec<String>> {
        if !self.environment.mise {
            return Ok(vec![]);
        }
        self.environment.validate()?;
        let helper = self
            .native_helper
            .clone()
            .or_else(|| {
                self.helper
                    .as_ref()
                    .map(|h| h.path.to_string_lossy().into_owned())
            })
            .unwrap_or(
                crate::command::current_executable()?
                    .to_string_lossy()
                    .into_owned(),
            );
        Ok(vec![
            helper,
            "--mise-launch".into(),
            serde_json::to_string(
                &serde_json::json!({"root":self.root,"settings":self.environment,"python":python}),
            )?,
            "--".into(),
        ])
    }

    /// Return the launch boundary plus the optional project environment provider.
    /// The `active` flag still describes actual isolation, independently of this wrapper.
    pub fn execution_prefix(&self, python: &str) -> Result<Vec<String>> {
        anyhow::ensure!(
            !self.active || !self.prefix.is_empty(),
            "Missing active sandbox launch policy"
        );
        let mut arguments = if self.active {
            self.prefix.clone()
        } else {
            vec![]
        };
        arguments.extend(self.environment_prefix(python)?);
        Ok(arguments)
    }
}

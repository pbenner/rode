//! Optional installed mise integration. The bundled extension contributes native UI,
//! while this small host launcher resolves an environment inside the captured sandbox.
//! Resolved values live only in the child process and never enter desktop preferences.

use crate::{environment, sandbox};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Shared Python launcher: bounded commands, environment resolution, then exec of the target.
pub const LAUNCHER: &str = concat!(
    include_str!("remote_environment.py"),
    "\n",
    include_str!("remote_mise.py"),
    "\n",
    include_str!("mise_launch.py")
);

const INSPECTOR: &str = concat!(
    include_str!("remote_environment.py"),
    "\n",
    include_str!("remote_mise.py"),
    "\nimport sys\njson.dump(mise_handle(json.load(sys.stdin)), sys.stdout)\n"
);

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
    settings.validate()?;
    let mut command = std::process::Command::new("python3");
    command.args(["-c", INSPECTOR]).current_dir(root);
    let input =
        serde_json::to_vec(&serde_json::json!({"root":root,"settings":settings,"action":action}))?;
    let output = crate::project::run(command, input, std::time::Duration::from_secs(130))?;
    let text = output.checked()?;
    serde_json::from_str(&text).context("Invalid mise metadata response")
}

/// Build an environment launcher without changing the raw sandbox prefix or its grants.
/// Callers append this after bubblewrap (and before the target), including in normal mode.
pub fn prefix(settings: &environment::Settings, root: &str, python: &str) -> Result<Vec<String>> {
    if !settings.mise {
        return Ok(vec![]);
    }
    settings.validate()?;
    Ok(vec![
        "python3".into(),
        "-c".into(),
        LAUNCHER.into(),
        serde_json::to_string(
            &serde_json::json!({"root":root,"settings":settings,"python":python}),
        )?,
        "--".into(),
    ])
}

/// Quote a configured mise executable, including the standard user-local fallback for shells.
pub fn shell_program(settings: &environment::Settings) -> String {
    let program = settings.program("mise");
    if program.contains('/') {
        crate::ssh::quote(&program)
    } else {
        format!(
            "\"$(command -v {} || printf '%s' \"$HOME/.local/bin/\"{})\"",
            crate::ssh::quote(&program),
            crate::ssh::quote(&program)
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
        crate::ssh::quote(root)
    )
}

impl sandbox::Plan {
    /// Control/inspection requests run before providers; actual project tools inherit the provider.
    pub(crate) fn environment_applies(&self, request: &crate::files::Request) -> bool {
        self.environment.mise
            && matches!(request,
            crate::files::Request::Tool { request } if !matches!(request,
                crate::project::Request::Mise { .. }
                | crate::project::Request::Monitor { .. }
                | crate::project::Request::Sessions { .. }))
    }

    /// Return the launch boundary plus the optional project environment provider.
    /// The `active` flag still describes actual isolation, independently of this wrapper.
    pub fn execution_prefix(&self, python: &str) -> Result<Vec<String>> {
        let mut arguments = if self.active {
            self.prefix.clone()
        } else {
            vec![]
        };
        arguments.extend(prefix(&self.environment, &self.root, python)?);
        Ok(arguments)
    }
}

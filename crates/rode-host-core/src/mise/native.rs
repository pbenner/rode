//! Installed mise execution with bounded output and no implicit installation or trust.
use super::{Action, Snapshot, Task};
use crate::{environment::Settings, project};
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeMap, path::Path, process::Command, time::Duration};

fn run(root: &str, settings: &Settings, arguments: &[&str], timeout: u64) -> Result<String> {
    settings.validate()?;
    let executable = crate::command::resolve_program(Path::new(root), &settings.program("mise"));
    let mut command = Command::new(executable);
    command.args(arguments).current_dir(root).envs([
        ("MISE_AUTO_INSTALL", "0"),
        ("MISE_EXEC_AUTO_INSTALL", "0"),
        ("MISE_PARANOID", "1"),
        ("MISE_YES", "0"),
    ]);
    let output = project::run(command, vec![], Duration::from_secs(timeout))?;
    if arguments == ["env", "--json"] && !output.success() {
        // Failed providers can leave secret values on stdout. Do not include those in errors.
        anyhow::bail!(
            "{}",
            if output.stderr.is_empty() {
                "Cannot resolve the mise environment".into()
            } else {
                output.stderr.chars().take(8192).collect::<String>()
            }
        );
    }
    output.checked()
}

pub(super) fn execute(root: &str, settings: &Settings, action: Action) -> Result<Snapshot> {
    match action {
        Action::Trust | Action::Install => Ok(Snapshot {
            output: run(
                root,
                settings,
                &[if matches!(action, Action::Trust) {
                    "trust"
                } else {
                    "install"
                }],
                if matches!(action, Action::Trust) {
                    30
                } else {
                    120
                },
            )?
            .chars()
            .take(65536)
            .collect(),
            ..Default::default()
        }),
        Action::Inspect => {
            let version = run(root, settings, &["--version"], 15)?;
            let tools: serde_json::Value =
                serde_json::from_str(&run(root, settings, &["ls", "--current", "--json"], 15)?)?;
            let tasks: serde_json::Value = serde_json::from_str(&run(
                root,
                settings,
                &["tasks", "ls", "--local", "--json"],
                15,
            )?)?;
            let tools = tools.as_object().context("Invalid mise tool metadata")?;
            let tasks = tasks.as_array().context("Invalid mise task metadata")?;
            ensure!(tasks.len() <= 256, "Invalid mise tool/task metadata");
            Ok(Snapshot {
                version: version.trim().chars().take(256).collect(),
                tools: tools
                    .keys()
                    .take(128)
                    .map(|s| s.chars().take(128).collect())
                    .collect(),
                tasks: tasks
                    .iter()
                    .filter_map(|task| {
                        let name = task.get("name")?.as_str()?;
                        if name.is_empty()
                            || name.chars().count() > 256
                            || name.starts_with('-')
                            || name.chars().any(|c| c < ' ')
                        {
                            return None;
                        }
                        Some(Task {
                            name: name.into(),
                            description: task
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .chars()
                                .take(1024)
                                .collect(),
                        })
                    })
                    .collect(),
                output: String::new(),
            })
        }
    }
}

/// Resolve values on the project host inside its sandbox; never persist the result.
pub fn environment(
    root: &str,
    settings: &Settings,
    python: &str,
) -> Result<BTreeMap<String, String>> {
    let tools: serde_json::Value =
        serde_json::from_str(&run(root, settings, &["ls", "--current", "--json"], 15)?)?;
    let tools = tools
        .as_object()
        .context("mise returned invalid tool metadata")?;
    let missing = tools
        .iter()
        .filter(|(_, versions)| {
            versions.as_array().is_some_and(|versions| {
                versions
                    .iter()
                    .any(|v| v.get("installed") == Some(&serde_json::Value::Bool(false)))
            })
        })
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    ensure!(
        missing.is_empty(),
        "Missing mise tools: {}. Use Install declared tools.",
        missing.join(", ")
    );
    let text = run(root, settings, &["env", "--json"], 15)?;
    ensure!(
        text.len() <= 1024 * 1024,
        "mise returned an invalid or oversized environment"
    );
    let values: BTreeMap<String, String> =
        serde_json::from_str(&text).context("mise returned an invalid environment")?;
    ensure!(
        values.len() <= 2048
            && values.iter().all(|(name, value)| !name.is_empty()
                && !name.contains(['=', '\0'])
                && !value.contains('\0')),
        "mise returned an invalid or oversized environment"
    );
    let mut env: BTreeMap<_, _> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    env.extend(values);
    settings.apply(&mut env)?;
    let python = project::python_path(Path::new(root), python);
    if python.is_absolute()
        && let Some(bin) = python.parent()
    {
        env.insert(
            "PATH".into(),
            format!(
                "{}:{}",
                bin.display(),
                env.get("PATH").cloned().unwrap_or_default()
            ),
        );
        if let Some(prefix) = bin.parent()
            && prefix.join("pyvenv.cfg").is_file()
        {
            env.insert("VIRTUAL_ENV".into(), prefix.to_string_lossy().into());
            env.remove("PYTHONHOME");
        }
    }
    Ok(env)
}

/// Resolve once, then replace this single-purpose process without consuming target stdin.
pub fn launch(arguments: &[String]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    ensure!(
        arguments.len() >= 3 && arguments[1] == "--" && arguments[0].len() <= 128 * 1024,
        "Invalid mise launch command"
    );
    #[derive(serde::Deserialize)]
    struct Spec {
        root: String,
        settings: Settings,
        python: String,
    }
    let spec: Spec = serde_json::from_str(&arguments[0])?;
    let root = crate::files::expand_home(&spec.root).canonicalize()?;
    let env = environment(
        root.to_str().context("Non-UTF-8 project root")?,
        &spec.settings,
        &spec.python,
    )?;
    ensure!(
        !crate::command::CANCELLED.load(std::sync::atomic::Ordering::Acquire),
        "mise launch cancelled"
    );
    Err(Command::new(&arguments[2])
        .args(&arguments[3..])
        .current_dir(root)
        .env_clear()
        .envs(env)
        .exec())
    .context("Cannot execute the project command")
}

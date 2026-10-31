//! Workspace policy and deployment adapter for the native session service.
use crate::{project, sandbox};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// Workspace-level native-terminal operations, shared with the remote deployment peer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Operation {
    /// Host-owned notebook/debugger control, outside the captured process sandbox.
    Runtimes {
        /// Only runtime discovery, creation and termination are accepted.
        request: Box<rode_session::Request>,
    },
    /// Discover running terminal sessions without attaching a view.
    List,
    /// Native agent operations use the same verified host helper and service.
    Agents {
        /// Agent-only native service request.
        request: Box<rode_session::Request>,
    },
    /// Create a shell under the captured project plan and explicit lifetime policy.
    Create {
        /// New opaque session identifier.
        id: String,
        /// Initial display name.
        name: String,
        /// Prepared project-host sandbox plan captured at creation.
        plan: Box<sandbox::Plan>,
        /// Fingerprint matching that actual plan.
        policy: String,
        /// Selected interpreter used for new-shell environment activation.
        python: String,
        /// Runtime paths and literal variables captured for this new shell.
        #[serde(default)]
        environment: Box<crate::environment::Settings>,
        /// Explicit opt-in lifetime after the final view closes.
        persistent: bool,
        /// Optional managed scheduler launch; otherwise create the normal login shell.
        #[serde(default)]
        allocation: Option<crate::slurm::Launch>,
    },
    /// Change one session's display name.
    Rename {
        /// Session to rename.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
        /// Replacement display name.
        name: String,
    },
    /// Change one session's last-view lifetime without restarting it.
    SetPersistence {
        /// Session to update.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
        /// Desired persistence flag.
        persistent: bool,
    },
    /// Explicitly terminate a session and its owned process tree.
    End {
        /// Session to terminate.
        id: String,
        /// Expected workspace policy fingerprint.
        policy: String,
    },
}
/// Checksum-verified native helper deployment metadata for an SSH host.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Helper {
    /// Lowercase SHA-256 of helper bytes.
    pub digest: String,
    /// Target helper architecture label.
    pub architecture: String,
    /// Optional standard base64 helper bytes; omitted when only checking a cached deployment.
    pub data: Option<String>,
}
/// Control result and matching executable identity needed for later attachment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    /// Native service's typed control reply.
    pub response: rode_session::Response,
    /// Project-host helper executable used for attachment.
    pub executable: String,
    /// Literal arguments inserted before the attachment subcommand.
    pub prefix: Vec<String>,
}
/// Hash the root and actual sandbox launch policy to prevent attachment under a mismatched
/// workspace policy.
pub fn fingerprint(plan: &sandbox::Plan) -> String {
    crate::files::digest(
        &serde_json::to_vec(&(
            &plan.root,
            plan.active,
            plan.project_read_only,
            &plan.prefix,
        ))
        .expect("serializable terminal policy"),
    )
}
/// Perform a blocking local control operation through the matching native service; never
/// restart existing jobs merely to upgrade the helper.
pub fn local(operation: &Operation) -> Result<Reply> {
    let exe = rode_session::current_executable()?;
    // The editor's embedded service always matches its client. A sibling
    // binary can be stale after cargo build or an unpackaged update.
    let standalone = std::env::var_os("RODE_SESSION_HELPER").map(PathBuf::from);
    // Library-test executables have Rust's test harness as their main, so they
    // cannot implement the editor's --session-service entry point themselves.
    #[cfg(test)]
    let standalone = standalone.or_else(|| {
        exe.parent()
            .and_then(std::path::Path::parent)
            .map(|p| p.join("rode-sessiond"))
            .filter(|p| p.is_file())
    });
    if let Some(path) = &standalone {
        let mut command = std::process::Command::new(path);
        command.arg("protocol");
        let output = project::run_controlled_limited(
            command,
            vec![],
            std::time::Duration::from_secs(5),
            &std::sync::atomic::AtomicBool::new(false),
            false,
            4096,
            std::time::Duration::ZERO,
        )
        .context("Cannot check RODE_SESSION_HELPER; rebuild it with scripts/build-sessiond.sh")?;
        ensure!(
            output.success() && output.stdout.trim() == rode_session::PROTOCOL.to_string(),
            "RODE_SESSION_HELPER uses an older terminal protocol. Rebuild it with scripts/build-sessiond.sh or unset RODE_SESSION_HELPER to use the editor's matching service. Existing jobs were not touched."
        );
    }
    let launcher = match standalone {
        Some(executable) => rode_session::Launcher {
            executable,
            prefix: vec![],
        },
        None => rode_session::Launcher {
            executable: exe,
            prefix: vec!["--session-service".into()],
        },
    };
    let request = match operation {
        Operation::Runtimes { request } => {
            let mut request = *request.clone();
            ensure!(
                matches!(
                    request,
                    rode_session::Request::Runtimes
                        | rode_session::Request::CreateRuntime { .. }
                        | rode_session::Request::EndRuntime { .. }
                ),
                "Only runtime control requests are accepted"
            );
            if let rode_session::Request::CreateRuntime { spec } = &mut request {
                let root = std::path::Path::new(&spec.root);
                let python = project::python_path(root, &spec.python);
                spec.argv[0] = if spec.python_program {
                    python.clone()
                } else {
                    crate::process::resolve_program(root, &spec.argv[0])
                }
                .to_string_lossy()
                .into();
                spec.env = std::env::vars_os()
                    .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                    .collect();
                spec.env
                    .extend(project::environment_variables(&python.to_string_lossy()));
            }
            request
        }
        Operation::List => rode_session::Request::List,
        Operation::Agents { request } => {
            let mut request = *request.clone();
            ensure!(
                matches!(
                    request,
                    rode_session::Request::Agents
                        | rode_session::Request::CreateAgent { .. }
                        | rode_session::Request::EndAgent { .. }
                ),
                "Only agent control requests are accepted here"
            );
            if let rode_session::Request::CreateAgent { spec } = &mut request {
                let root = std::path::Path::new(&spec.plan.root);
                spec.executable = crate::process::resolve_program(root, &spec.executable)
                    .to_string_lossy()
                    .into();
                spec.env = std::env::vars_os()
                    .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                    .collect();
                let python = project::python_path(root, &spec.python);
                spec.env
                    .extend(project::environment_variables(&python.to_string_lossy()));
            }
            request
        }
        Operation::Create {
            id,
            name,
            plan,
            policy,
            python,
            environment,
            persistent,
            allocation,
        } => {
            ensure!(
                *policy == fingerprint(plan),
                "Terminal policy does not match the prepared workspace"
            );
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
            let mut argv = if plan.active {
                plan.prefix.clone()
            } else {
                vec![]
            };
            if let Some(launch) = allocation {
                argv.extend(launch.argv.clone());
            } else {
                argv.extend([shell, "-l".into()]);
            }
            let mut env: BTreeMap<String, String> = std::env::vars_os()
                .filter_map(|(key, value)| {
                    Some((key.into_string().ok()?, value.into_string().ok()?))
                })
                .collect();
            let python = project::python_path(std::path::Path::new(&plan.root), python);
            env.extend(project::environment_variables(&python.to_string_lossy()));
            let mut configured = env.clone();
            environment.apply(&mut configured)?;
            if allocation.is_some() {
                env.retain(|key, _| !key.starts_with("SALLOC_") && !key.starts_with("SRUN_"));
                configured
                    .retain(|key, _| !key.starts_with("SALLOC_") && !key.starts_with("SRUN_"));
            }
            if plan.active {
                // Apply project values inside bwrap, not to its host-side loader.
                let mut options = vec![];
                for (name, value) in &configured {
                    if environment.variables.contains_key(name) || env.get(name) != Some(value) {
                        options.extend(["--setenv".into(), name.clone(), value.clone()]);
                    }
                }
                if !options.is_empty() {
                    ensure!(
                        plan.prefix.last().is_some_and(|arg| arg == "--"),
                        "Invalid sandbox command boundary"
                    );
                    argv.splice(plan.prefix.len() - 1..plan.prefix.len() - 1, options);
                }
            } else {
                env = configured;
            }
            rode_session::Request::Create {
                id: id.clone(),
                name: name.clone(),
                root: plan.root.clone(),
                policy: policy.clone(),
                mode: plan.label().into(),
                argv,
                // Do not send shell input while the allocator is waiting for resources.
                initial_input: if allocation.is_some() {
                    String::new()
                } else {
                    let activation = initial_input(&python);
                    let exports = environment.exports()?;
                    if exports.is_empty() {
                        activation
                    } else {
                        format!("{activation}{exports}\n")
                    }
                },
                env,
                persistent: *persistent,
                allocation: allocation.as_ref().map(|launch| launch.job_name.clone()),
            }
        }
        Operation::Rename { id, policy, name } => rode_session::Request::Rename {
            id: id.clone(),
            policy: policy.clone(),
            name: name.clone(),
        },
        Operation::SetPersistence {
            id,
            policy,
            persistent,
        } => rode_session::Request::SetPersistence {
            id: id.clone(),
            policy: policy.clone(),
            persistent: *persistent,
        },
        Operation::End { id, policy } => rode_session::Request::End {
            id: id.clone(),
            policy: policy.clone(),
        },
    };
    let response = rode_session::request(&request, &launcher)?;
    Ok(Reply {
        response,
        executable: launcher.executable.to_string_lossy().into(),
        prefix: launcher.prefix,
    })
}
/// One-time shell initialization for a newly created session only. Login
/// startup files may replace PATH after the inherited environment was set.
pub fn initial_input(python: &std::path::Path) -> String {
    let Some(bin) = python.parent().filter(|_| python.is_absolute()) else {
        return String::new();
    };
    let Some(prefix) = bin.parent() else {
        return String::new();
    };
    let bin = crate::ssh::quote(&bin.to_string_lossy());
    let deactivate = "if command -v deactivate >/dev/null 2>&1; then deactivate; fi; ";
    if prefix.join("pyvenv.cfg").is_file() {
        let activate = crate::ssh::quote(&prefix.join("bin/activate").to_string_lossy());
        let prefix = crate::ssh::quote(&prefix.to_string_lossy());
        format!(
            "{deactivate}if [ -r {activate} ]; then . {activate}; else export VIRTUAL_ENV={prefix}; export PATH={bin}:\"$PATH\"; unset PYTHONHOME; fi\n"
        )
    } else {
        format!("{deactivate}unset VIRTUAL_ENV PYTHONHOME; export PATH={bin}:\"$PATH\"\n")
    }
}
/// Prefer the static helper for remote hosts; it needs no desktop libraries.
pub fn helper(with_data: bool) -> Result<Helper> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let exe = std::env::current_exe()?;
    let bin = exe.parent().context("Missing binary directory")?;
    let mut candidates = vec![];
    if let Some(path) = std::env::var_os("RODE_SESSION_HELPER") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(bin.join("rode-sessiond-static"));
    if let Some(target) = bin.parent() {
        candidates.push(
            target
                .join(format!("{}-unknown-linux-musl", std::env::consts::ARCH))
                .join(bin.file_name().unwrap())
                .join("rode-sessiond-standalone"),
        );
        // Preserve discovery of static helpers built before the Cargo install target.
        candidates.push(
            target
                .join(format!("{}-unknown-linux-musl", std::env::consts::ARCH))
                .join(bin.file_name().unwrap())
                .join("rode-sessiond"),
        );
    }
    candidates.push(bin.join("rode-sessiond"));
    let path=candidates.into_iter().find(|p|p.is_file()).context("Remote persistent terminals need rode-sessiond beside the editor. Reinstall with cargo install --locked --path . from the RODE source checkout, use scripts/build-sessiond.sh for a static helper, or set RODE_SESSION_HELPER to a compatible helper binary.")?;
    let bytes = std::fs::read(&path)?;
    ensure!(
        bytes.len() <= 24 * 1024 * 1024,
        "Session helper exceeds 24 MiB; use a release build"
    );
    ensure!(
        bytes.starts_with(b"\x7fELF") && bytes.len() > 20,
        "Remote session helper must be a Linux ELF binary"
    );
    let architecture = match u16::from_le_bytes([bytes[18], bytes[19]]) {
        62 => "x86_64",
        183 => "aarch64",
        _ => anyhow::bail!("Unsupported remote helper architecture"),
    };
    Ok(Helper {
        digest: crate::files::digest(&bytes),
        architecture: architecture.into(),
        data: with_data.then(|| STANDARD.encode(bytes)),
    })
}

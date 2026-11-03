//! Policy-aware workspace adapter for the persistent service.
use anyhow::{Result, ensure};
use rode_host_core::{project, sandbox};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
/// Workspace-level native-terminal operations, shared with the remote deployment peer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Operation {
    /// Host-owned notebook/debugger control, outside the captured process sandbox.
    Runtimes {
        /// Preserve bare commands for resolution by the captured mise launch wrapper.
        #[serde(default)]
        mise: bool,
        /// Only runtime discovery, creation and termination are accepted.
        request: Box<crate::Request>,
    },
    /// Discover running terminal sessions without attaching a view.
    List,
    /// Native agent operations use the same verified host helper and service.
    Agents {
        /// Preserve bare commands for resolution by the captured mise launch wrapper.
        #[serde(default)]
        mise: bool,
        /// Agent-only native service request.
        request: Box<crate::Request>,
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
        environment: Box<rode_host_core::environment::Settings>,
        /// Explicit opt-in lifetime after the final view closes.
        persistent: bool,
        /// Optional managed scheduler launch; otherwise create the normal login shell.
        #[serde(default)]
        allocation: Option<rode_host_core::slurm::Launch>,
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
    pub response: crate::Response,
    /// Project-host helper executable used for attachment.
    pub executable: String,
    /// Literal arguments inserted before the attachment subcommand.
    pub prefix: Vec<String>,
}
/// Hash the root and actual sandbox launch policy to prevent attachment under a mismatched
/// workspace policy.
pub fn fingerprint(plan: &sandbox::Plan) -> String {
    rode_host_core::files::digest(
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
pub fn local(operation: &Operation, launcher: crate::Launcher) -> Result<Reply> {
    let request = match operation {
        Operation::Runtimes { request, mise } => {
            let mut request = *request.clone();
            ensure!(
                matches!(
                    request,
                    crate::Request::Runtimes
                        | crate::Request::CreateRuntime { .. }
                        | crate::Request::EndRuntime { .. }
                ),
                "Only runtime control requests are accepted"
            );
            if let crate::Request::CreateRuntime { spec } = &mut request {
                let root = std::path::Path::new(&spec.root);
                let python = project::python_path(root, &spec.python);
                spec.argv[0] = if spec.python_program {
                    python.clone()
                } else if *mise && !spec.argv[0].contains('/') && !spec.argv[0].starts_with('~') {
                    PathBuf::from(&spec.argv[0])
                } else {
                    rode_host_core::command::resolve_program(root, &spec.argv[0])
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
        Operation::List => crate::Request::List,
        Operation::Agents { request, mise } => {
            let mut request = *request.clone();
            ensure!(
                matches!(
                    request,
                    crate::Request::Agents
                        | crate::Request::CreateAgent { .. }
                        | crate::Request::EndAgent { .. }
                ),
                "Only agent control requests are accepted here"
            );
            if let crate::Request::CreateAgent { spec } = &mut request {
                let root = std::path::Path::new(&spec.plan.root);
                spec.executable =
                    if *mise && !spec.executable.contains('/') && !spec.executable.starts_with('~')
                    {
                        PathBuf::from(&spec.executable)
                    } else {
                        rode_host_core::command::resolve_program(root, &spec.executable)
                    }
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
        Operation::Create { .. } => terminal_request(operation)?,
        Operation::Rename { id, policy, name } => crate::Request::Rename {
            id: id.clone(),
            policy: policy.clone(),
            name: name.clone(),
        },
        Operation::SetPersistence {
            id,
            policy,
            persistent,
        } => crate::Request::SetPersistence {
            id: id.clone(),
            policy: policy.clone(),
            persistent: *persistent,
        },
        Operation::End { id, policy } => crate::Request::End {
            id: id.clone(),
            policy: policy.clone(),
        },
    };
    let response = crate::request(&request, &launcher)?;
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
    let bin = rode_host_core::command::quote(&bin.to_string_lossy());
    let deactivate = "if command -v deactivate >/dev/null 2>&1; then deactivate; fi; ";
    if prefix.join("pyvenv.cfg").is_file() {
        let activate =
            rode_host_core::command::quote(&prefix.join("bin/activate").to_string_lossy());
        let prefix = rode_host_core::command::quote(&prefix.to_string_lossy());
        format!(
            "{deactivate}if [ -r {activate} ]; then . {activate}; else export VIRTUAL_ENV={prefix}; export PATH={bin}:\"$PATH\"; unset PYTHONHOME; fi\n"
        )
    } else {
        format!("{deactivate}unset VIRTUAL_ENV PYTHONHOME; export PATH={bin}:\"$PATH\"\n")
    }
}

fn terminal_request(operation: &Operation) -> Result<crate::Request> {
    let Operation::Create {
        id,
        name,
        plan,
        policy,
        python,
        environment,
        persistent,
        allocation,
    } = operation
    else {
        anyhow::bail!("Expected terminal creation")
    };
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
    argv.extend(plan.environment_prefix(python)?);
    if let Some(launch) = allocation {
        argv.extend(launch.argv.clone());
    } else {
        argv.extend([shell, "-l".into()]);
    }
    let mut env: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let python = project::python_path(std::path::Path::new(&plan.root), python);
    env.extend(project::environment_variables(&python.to_string_lossy()));
    let mut configured = env.clone();
    environment.apply(&mut configured)?;
    if allocation.is_some() {
        env.retain(|key, _| !key.starts_with("SALLOC_") && !key.starts_with("SRUN_"));
        configured.retain(|key, _| !key.starts_with("SALLOC_") && !key.starts_with("SRUN_"));
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
    Ok(crate::Request::Create {
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
            let provider = rode_host_core::mise::shell_environment(&plan.environment, &plan.root);
            let exports = environment.exports()?;
            if exports.is_empty() {
                provider + &activation
            } else {
                format!("{provider}{activation}{exports}\n")
            }
        },
        env,
        persistent: *persistent,
        allocation: allocation.as_ref().map(|launch| launch.job_name.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminals_use_native_mise_only_when_the_captured_provider_is_enabled() {
        let saved = rode_host_core::environment::Settings {
            mise: true,
            variables: BTreeMap::from([("RODE_LITERAL".into(), "retained".into())]),
            ..Default::default()
        };
        for enabled in [false, true] {
            let plan = sandbox::Plan {
                root: "/project".into(),
                native_helper: Some("/verified/rode-sessiond".into()),
                environment: if enabled {
                    saved.clone()
                } else {
                    Default::default()
                },
                ..Default::default()
            };
            let request = terminal_request(&Operation::Create {
                id: "test".into(),
                name: "test".into(),
                policy: fingerprint(&plan),
                plan: Box::new(plan),
                python: "off".into(),
                environment: Box::new(saved.clone()),
                persistent: false,
                allocation: None,
            })
            .unwrap();
            let crate::Request::Create {
                argv,
                initial_input,
                env,
                ..
            } = request
            else {
                panic!("Expected terminal request")
            };
            assert_eq!(initial_input.contains("_rode_mise_env"), enabled);
            assert_eq!(argv.iter().any(|s| s == "--mise-launch"), enabled);
            if enabled {
                assert_eq!(argv[0], "/verified/rode-sessiond");
            }
            assert!(!argv.iter().any(|s| s == "python3"));
            assert!(initial_input.contains("export RODE_LITERAL='retained';"));
            assert_eq!(env["RODE_LITERAL"], "retained");
        }
    }
}

//! Project-host Git, environment, code-tool and telemetry protocols with bounded native/Python
//! behavior.

use rode::{
    files,
    project::{self, Request, Response},
    sandbox,
};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

fn remote(request: &Request) -> anyhow::Result<Response> {
    let mut child = Command::new("python3")
        .args(["-c", files::REMOTE_HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&files::Request::Tool {
            request: request.clone(),
        })?)?;
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice::<files::Response>(&output.stdout)?
        .tool
        .unwrap())
}

#[test]
fn python_command_runner_tolerates_a_child_exiting_before_stdin_is_flushed() {
    let script = format!(
        "{}\n{}",
        include_str!("../src/remote_project.py"),
        r#"
import sys

start_process = subprocess.Popen

def already_exited(*args, **kwargs):
    process = start_process(*args, **kwargs)
    # Force the valid scheduling order where the child exits before its writer runs.
    process.wait(timeout=5)
    return process

subprocess.Popen = already_exited
errors = []
threading.excepthook = lambda args: errors.append(str(args.exc_value))
for text in ("buffered input", "x" * (1024 * 1024)):
    output = project_run([sys.executable, "-c", "raise SystemExit(0)"], text=text)
    for thread in threading.enumerate():
        if thread is not threading.current_thread():
            thread.join(timeout=2)
    assert output == dict(code=0, stdout="", stderr=""), output
    assert not errors, errors
"#
    );
    let output = Command::new("python3")
        .args(["-c", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runtime_overrides_and_literal_variables_match_on_native_and_python_hosts() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("runtime tool 'quoted'");
    fs::write(
        &program,
        "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$APP_MODE\" \"$1\" \"$RUSTUP_AUTO_INSTALL\"\n",
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let mut settings = rode::environment::Settings::default();
    settings
        .variables
        .insert("APP_MODE".into(), "literal $(touch unwanted)".into());
    for (name, _) in rode::environment::TOOLS {
        settings
            .runtimes
            .insert((*name).into(), program.to_string_lossy().into_owned());
    }
    let request = Request::Runtimes {
        root: root.path().to_string_lossy().into_owned(),
        settings,
    };
    let Response::Runtimes(native) = project::local(&request).unwrap() else {
        panic!()
    };
    let Response::Runtimes(peer) = remote(&request).unwrap() else {
        panic!()
    };
    assert_eq!(native.len(), rode::environment::TOOLS.len());
    for (native, peer) in native.iter().zip(peer.iter()) {
        assert_eq!(native.name, peer.name);
        assert_eq!(native.executable, peer.executable);
        assert_eq!(native.version, peer.version);
        assert!(native.version.starts_with("literal $(touch unwanted)|"));
        assert!(native.version.ends_with("|0"));
        assert!(native.error.is_empty() && peer.error.is_empty());
    }
    assert!(!root.path().join("unwanted").exists());
}
fn git(root: &Path, args: &[&str]) -> project::Output {
    let request = Request::Git {
        root: root.to_str().unwrap().into(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        input: None,
    };
    let Response::Command(output) = project::local(&request).unwrap() else {
        panic!("wrong result")
    };
    output
}
fn checked(root: &Path, args: &[&str]) -> String {
    git(root, args).checked().unwrap()
}

#[test]
fn git_workflow_includes_quoted_paths_branches_worktrees_and_remote_protocol() {
    let repository = tempfile::tempdir().unwrap();
    let root = repository.path();
    checked(root, &["init", "-b", "main"]);
    checked(root, &["config", "user.name", "RODE Test"]);
    checked(root, &["config", "user.email", "rode@example.invalid"]);
    let filename = "file 'quoted' 🦀.py";
    fs::write(root.join(filename), "print('first')\n").unwrap();
    checked(root, &["add", "--", filename]);
    checked(root, &["commit", "-m", "Initial\n\nquoted 'message'"]);
    checked(root, &["switch", "-c", "feature/editor"]);
    fs::write(root.join(filename), "print('second')\n").unwrap();
    let request = Request::Git {
        root: root.to_str().unwrap().into(),
        args: vec![
            "status".into(),
            "--porcelain=v1".into(),
            "-z".into(),
            "--branch".into(),
        ],
        input: None,
    };
    let Response::Command(local) = project::local(&request).unwrap() else {
        panic!()
    };
    let Response::Command(peer) = remote(&request).unwrap() else {
        panic!()
    };
    assert_eq!(local.stdout, peer.stdout);
    let (branch, files) = project::parse_status(&local.stdout);
    assert!(branch.starts_with("feature/editor"));
    assert_eq!(files[0].path, filename);
    assert!(!files[0].staged());
    let renamed = "renamed\nfile.py";
    checked(root, &["restore", "--", filename]);
    checked(root, &["mv", "--", filename, renamed]);
    let status = checked(root, &["status", "--porcelain=v1", "-z"]);
    let (_, files) = project::parse_status(&status);
    assert_eq!(files[0].path, renamed);
    assert_eq!(files[0].original.as_deref(), Some(filename));
    checked(root, &["commit", "-m", "Rename"]);
    let worktree = root.join("linked worktree 🦀");
    checked(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "feature/worktree",
            "--",
            worktree.to_str().unwrap(),
        ],
    );
    assert!(worktree.join(renamed).exists());
    let listing = checked(root, &["worktree", "list", "--porcelain"]);
    assert!(listing.contains("feature/worktree"));
    checked(
        root,
        &["worktree", "remove", "--", worktree.to_str().unwrap()],
    );
    assert!(!worktree.exists());
    let failure = git(root, &["switch", "nonexistent"]);
    assert!(!failure.success());
    assert!(!failure.stderr.is_empty());
}

#[test]
fn individual_diff_hunks_can_be_staged_and_unstaged_without_staging_other_edits() {
    let repository = tempfile::tempdir().unwrap();
    let root = repository.path();
    checked(root, &["init"]);
    checked(root, &["config", "user.name", "RODE Test"]);
    checked(root, &["config", "user.email", "rode@example.invalid"]);
    let original = (0..40).map(|i| format!("line {i}\n")).collect::<String>();
    fs::write(root.join("file.txt"), &original).unwrap();
    checked(root, &["add", "file.txt"]);
    checked(root, &["commit", "-m", "Initial"]);
    fs::write(
        root.join("file.txt"),
        original
            .replace("line 1\n", "changed 1\n")
            .replace("line 35\n", "changed 35\n"),
    )
    .unwrap();
    let diff = checked(
        root,
        &["diff", "--no-ext-diff", "--no-textconv", "--", "file.txt"],
    );
    let hunks = project::hunk_patches(&diff);
    assert_eq!(hunks.len(), 2);
    for reverse in [false, true] {
        let mut args = vec!["apply".into(), "--cached".into()];
        if reverse {
            args.push("--reverse".into());
        }
        args.push("-".into());
        let Response::Command(output) = project::local(&Request::Git {
            root: root.to_str().unwrap().into(),
            args,
            input: Some(hunks[0].1.clone()),
        })
        .unwrap() else {
            panic!()
        };
        assert!(output.success(), "{}", output.stderr);
        let staged = checked(root, &["diff", "--cached"]);
        assert_eq!(staged.contains("changed 1"), !reverse);
        assert!(!staged.contains("changed 35"));
    }
}

#[test]
fn environments_are_created_detected_and_selected_on_both_backends() {
    for peer in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_str().unwrap().to_owned();
        let create = Request::CreateEnvironment {
            root: root.clone(),
            path: ".venv".into(),
            python: "python3".into(),
            with_pip: false,
        };
        let Response::Command(result) = (if peer {
            remote(&create).unwrap()
        } else {
            project::local(&create).unwrap()
        }) else {
            panic!()
        };
        assert!(result.success(), "{}", result.stderr);
        let request = Request::Environments {
            root,
            selected: "auto".into(),
        };
        let Response::Environments(environment) = (if peer {
            remote(&request).unwrap()
        } else {
            project::local(&request).unwrap()
        }) else {
            panic!()
        };
        assert!(environment.executable.contains(".venv/bin/python"));
        assert_ne!(environment.prefix, environment.base_prefix);
        assert!(environment.candidates.iter().any(|c| c.name == ".venv"));
        assert!(environment.kernels.is_empty());
        assert!(environment.kernel_error.contains("jupyter_client"));
        assert!(
            if peer {
                remote(&create)
            } else {
                project::local(&create)
            }
            .is_err(),
            "Existing environments must never be overwritten"
        );
    }
}

#[test]
fn ruff_formats_and_checks_buffer_without_writing_disk_on_both_backends() {
    let tool = std::env::var("RODE_TEST_RUFF")
        .ok()
        .map(Into::into)
        .or_else(|| sandbox::find_program("ruff"));
    let Some(tool): Option<std::path::PathBuf> = tool else {
        eprintln!("Set RODE_TEST_RUFF to exercise a real Ruff binary.");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("quoted 'name' 🦀.py");
    let source = "import os\nx=  [1,2,3]\nprint(undefined_name)\n";
    fs::write(&path, source).unwrap();
    // Even a project with `fix = true` must not mutate diagnostic input.
    fs::write(
        directory.path().join("ruff.toml"),
        "fix = true\nunsafe-fixes = true\n",
    )
    .unwrap();
    let request = Request::Analyze {
        root: directory.path().to_str().unwrap().into(),
        path: path.to_str().unwrap().into(),
        text: source.into(),
        python: String::new(),
        ruff: tool.to_str().unwrap().into(),
        format: true,
        check: true,
        fix: false,
    };
    let mut previous = None;
    for peer in [false, true] {
        let Response::Analysis(result) = (if peer {
            remote(&request).unwrap()
        } else {
            project::local(&request).unwrap()
        }) else {
            panic!()
        };
        assert!(result.text.contains("x = [1, 2, 3]"));
        assert!(result.text.contains("import os"));
        assert!(result.diagnostics.iter().any(|d| d.code == "F401"));
        assert!(result.diagnostics.iter().any(|d| d.code == "F821"));
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
        if let Some(previous) = &previous {
            assert_eq!(previous, &serde_json::to_value(&result).unwrap());
        }
        previous = Some(serde_json::to_value(result).unwrap());
    }
    for extension in ["pyi", "pyw", "PY"] {
        let mut variant = request.clone();
        if let Request::Analyze { path, .. } = &mut variant {
            *path = directory
                .path()
                .join(format!("variant.{extension}"))
                .to_string_lossy()
                .into();
        }
        for peer in [false, true] {
            let Response::Analysis(result) = (if peer {
                remote(&variant)
            } else {
                project::local(&variant)
            })
            .unwrap() else {
                panic!()
            };
            assert!(result.text.contains("x = [1, 2, 3]"));
            assert!(result.diagnostics.iter().any(|d| d.code == "F821"));
        }
    }
    let mut invalid = request.clone();
    if let Request::Analyze { text, .. } = &mut invalid {
        *text = "def broken(:\n".into();
    }
    for peer in [false, true] {
        let Response::Analysis(result) = (if peer {
            remote(&invalid).unwrap()
        } else {
            project::local(&invalid).unwrap()
        }) else {
            panic!()
        };
        assert_eq!(result.text, "def broken(:\n");
        assert!(!result.warnings.is_empty());
        assert!(result.diagnostics.iter().any(|d| d.severity == "error"));
    }
}

#[test]
fn gpu_parser_preserves_unknown_readings_and_monitor_has_real_host_counters() {
    let gpus = project::parse_nvidia(
        "0, NVIDIA A100, 73, 100, 40000, 53, 180.25\n1, NVIDIA MIG, N/A, 0, 10000, N/A, [Not Supported]\n",
    );
    assert_eq!(gpus.len(), 2);
    assert_eq!(gpus[0].usage, Some(73.));
    assert_eq!(gpus[0].memory_used, Some(100 * 1024 * 1024));
    assert_eq!(gpus[1].usage, None);
    assert_eq!(gpus[1].power, None);
    let request = Request::Monitor {
        root: "/tmp".into(),
    };
    for peer in [false, true] {
        let Response::Monitor(result) = (if peer {
            remote(&request).unwrap()
        } else {
            project::local(&request).unwrap()
        }) else {
            panic!()
        };
        assert!(result.cpus[0].total > 0);
        assert!(result.memory_total > result.memory_available);
        assert!(!result.hostname.is_empty());
        assert!(result.uptime > 0.);
    }
}

#[test]
fn command_timeout_and_output_bounds_stop_children() {
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 30 & wait"]);
    assert!(project::run(command, vec![], Duration::from_millis(80)).is_err());
    let mut command = Command::new("python3");
    command.args(["-c", "import os;os.write(1,b'x'*(9*1024*1024))"]);
    assert!(project::run(command, vec![], Duration::from_secs(5)).is_err());
}

fn analyze_request(root: &Path, path: &Path, text: &str, format: bool) -> Request {
    Request::Analyze {
        root: root.to_string_lossy().into(),
        path: path.to_string_lossy().into(),
        text: text.into(),
        python: String::new(),
        ruff: "auto".into(),
        format,
        check: true,
        fix: false,
    }
}

#[test]
fn gofmt_formats_unsaved_buffers_and_reports_syntax_on_both_backends() {
    if sandbox::find_program("gofmt").is_none() {
        eprintln!("Install Go / add gofmt to PATH to exercise Go tools.");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("quoted 'code' 🦀.go");
    fs::write(&path, "on-disk source must not be read or changed").unwrap();
    for (source, format) in [
        ("package main\nfunc main(){println(\"hello\")}\n", true),
        ("package main\nfunc main( {\n", true),
        ("package main\nfunc main( {\n", false),
    ] {
        let request = analyze_request(directory.path(), &path, source, format);
        let mut previous = None;
        for peer in [false, true] {
            let Response::Analysis(result) = (if peer {
                remote(&request)
            } else {
                project::local(&request)
            })
            .unwrap() else {
                panic!()
            };
            if source.contains("println") {
                assert!(result.text.contains("func main() {"));
                assert!(result.diagnostics.is_empty());
            } else {
                assert_eq!(result.text, source);
                assert!(
                    result
                        .diagnostics
                        .iter()
                        .any(|d| d.row == 2 && d.column > 1 && d.severity == "error")
                );
            }
            if let Some(previous) = previous {
                assert_eq!(serde_json::to_value(&result).unwrap(), previous);
            }
            previous = Some(serde_json::to_value(result).unwrap());
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "on-disk source must not be read or changed"
            );
        }
    }
}

#[test]
fn installed_web_and_c_formatters_use_stdin_and_project_configuration() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
    if let Some(prettier) = std::env::var_os("RODE_TEST_PRETTIER") {
        symlink(prettier, root.join("node_modules/.bin/prettier")).unwrap();
        fs::write(
            root.join(".prettierrc.json"),
            "{\"singleQuote\":true,\"semi\":false}",
        )
        .unwrap();
        let path = root.join("quoted 'file'.ts");
        fs::write(&path, "disk").unwrap();
        for source in ["const value:string=\"hello\";\n", "const value = ;\n"] {
            let mut previous = None;
            for peer in [false, true] {
                let Response::Analysis(result) = (if peer {
                    remote(&analyze_request(root, &path, source, true))
                } else {
                    project::local(&analyze_request(root, &path, source, true))
                })
                .unwrap() else {
                    panic!()
                };
                if source.contains("hello") {
                    assert_eq!(result.text, "const value: string = 'hello'\n");
                } else {
                    assert_eq!(result.text, source);
                    assert_eq!(result.diagnostics[0].row, 1);
                    assert!(result.diagnostics[0].column > 1);
                }
                if let Some(previous) = previous {
                    assert_eq!(serde_json::to_value(&result).unwrap(), previous);
                }
                previous = Some(serde_json::to_value(result).unwrap());
                assert_eq!(fs::read_to_string(&path).unwrap(), "disk");
            }
        }
    } else {
        eprintln!("Set RODE_TEST_PRETTIER for real web formatter checks.");
    }
    if sandbox::find_program("clang-format").is_some() {
        fs::write(
            root.join(".clang-format"),
            "BasedOnStyle: LLVM\nIndentWidth: 4\n",
        )
        .unwrap();
        let source = "int main(){return 0;}\n";
        for peer in [false, true] {
            let mut request = analyze_request(root, &root.join("main.cpp"), source, true);
            if let Request::Analyze { check, .. } = &mut request {
                *check = false;
            }
            let Response::Analysis(result) = (if peer {
                remote(&request)
            } else {
                project::local(&request)
            })
            .unwrap() else {
                panic!()
            };
            assert_eq!(result.text, "int main() { return 0; }\n");
            assert!(!root.join("main.cpp").exists());
        }
    }
}

#[test]
fn project_local_formatter_obeys_sandbox_grants() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
    let outside = directory.path().join("private");
    fs::write(&outside, "private").unwrap();
    let tool = root.join("node_modules/.bin/prettier");
    // A deterministic installed formatter that tries to read outside the project.
    fs::write(&tool, format!("#!/usr/bin/python3\nimport sys,os\nassert not os.path.exists({:?})\nsys.stdout.write(sys.stdin.read())\n",outside.to_str().unwrap())).unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let plan = sandbox::prepare(
        root.to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    if !plan.active {
        eprintln!("bwrap unavailable: {}", plan.reason);
        return;
    }
    let request = files::Request::Tool {
        request: analyze_request(&root, &root.join("main.ts"), "const x = 1;\n", true),
    };
    let response = plan
        .local_request_with_helper(&request, Path::new(env!("CARGO_BIN_EXE_rode")))
        .unwrap();
    assert!(
        matches!(response.tool,Some(Response::Analysis(result)) if result.text == "const x = 1;\n")
    );
}

#[test]
fn shell_formatter_preserves_invalid_buffers_and_reports_positions() {
    if sandbox::find_program("shfmt").is_none() {
        eprintln!("Install shfmt to exercise shell tools.");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("quoted 'shell'.sh");
    for source in ["if true;then echo hello;fi\n", "echo hello\nif then\n"] {
        let mut previous = None;
        for peer in [false, true] {
            let request = analyze_request(root.path(), &path, source, true);
            let Response::Analysis(result) = (if peer {
                remote(&request)
            } else {
                project::local(&request)
            })
            .unwrap() else {
                panic!()
            };
            if source.contains("if then") {
                assert_eq!(result.text, source);
                assert_eq!(result.diagnostics[0].row, 2);
            } else {
                assert!(result.text.contains("if true; then"));
                assert!(result.diagnostics.is_empty());
            }
            if let Some(previous) = previous {
                assert_eq!(serde_json::to_value(&result).unwrap(), previous);
            }
            previous = Some(serde_json::to_value(result).unwrap());
            assert!(!path.exists());
        }
    }
}

#[test]
fn installed_kernel_discovery_matches_on_native_and_python_hosts_without_starting_kernels() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("jupyter_client");
    fs::create_dir(&package).unwrap();
    fs::write(package.join("__init__.py"), "").unwrap();
    fs::write(package.join("kernelspec.py"), r#"
class KernelSpecManager:
    def find_kernel_specs(self):
        return dict(python3='fixture', julia='fixture', cloud='fixture', broken='fixture')
    def get_kernel_spec(self, name):
        if name == 'broken': raise ValueError('broken spec')
        class Spec:
            display_name = 'Fixture ' + name
            language = 'python' if name == 'python3' else 'julia'
            metadata = {'kernel_provisioner': {'provisioner_name': 'cloud' if name == 'cloud' else 'local-provisioner'}}
        return Spec()
"#).unwrap();
    let request = Request::Environments {
        root: directory.path().to_string_lossy().into(),
        selected: String::new(),
    };
    for response in [project::local(&request).unwrap(), remote(&request).unwrap()] {
        let Response::Environments(info) = response else {
            panic!()
        };
        assert!(info.kernel_error.is_empty());
        assert_eq!(
            info.kernels
                .iter()
                .map(|k| k.name.as_str())
                .collect::<Vec<_>>(),
            vec!["julia", "python3"]
        );
        assert_eq!(info.kernels[0].display_name, "Fixture julia");
    }
}

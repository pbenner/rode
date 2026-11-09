//! Optional mise providers preserve host/policy ownership, trust, explicit overrides and stdio.
use rode::sandbox::PlanExt as _;
#[path = "support/native.rs"]
mod native_peer;

use rode::{environment, files, mise, project};
use std::{
    collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path, process::Command,
    time::Duration,
};

struct Fixture {
    root: tempfile::TempDir,
    settings: environment::Settings,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("mise");
        fs::write(&program, include_str!("fixtures/mise.py")).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir(root.path().join("mise-bin")).unwrap();
        let settings = environment::Settings {
            mise: true,
            runtimes: BTreeMap::from([("mise".into(), program.to_string_lossy().into_owned())]),
            ..Default::default()
        };
        Self { root, settings }
    }
    fn root(&self) -> String {
        self.root.path().to_string_lossy().into_owned()
    }
    fn request(&self, action: mise::Action) -> files::Request {
        files::Request::Tool {
            request: project::Request::Mise {
                root: self.root(),
                settings: self.settings.clone(),
                action,
            },
        }
    }
    fn trusted(&self) {
        fs::write(self.root.path().join("mise-trusted"), "").unwrap();
    }
    fn program(&self, name: &str, source: &str) {
        let path = self.root.path().join("mise-bin").join(name);
        fs::write(&path, source).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn execute(&self, request: &files::Request, peer: bool) -> anyhow::Result<files::Response> {
        let mut command = if peer {
            native_peer::command()
        } else {
            let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
            command.arg("--file-helper");
            command
        };
        command.current_dir(self.root.path());
        command.env("RODE_SESSION_DIR", self.root.path().join("sessions"));
        let output = project::run(
            command,
            serde_json::to_vec(request)?,
            Duration::from_secs(30),
        )?;
        Ok(serde_json::from_str(&output.checked()?)?)
    }
}

#[test]
fn inspection_and_setup_have_desktop_headless_parity_and_never_export_secret_values() {
    for peer in [false, true] {
        let fixture = Fixture::new();
        let error = fixture
            .execute(&fixture.request(mise::Action::Inspect), peer)
            .unwrap_err();
        assert!(format!("{error:#}").contains("not trusted"));
        assert!(!fixture.root.path().join("mise-installed").exists());
        fixture
            .execute(&fixture.request(mise::Action::Trust), peer)
            .unwrap();
        let reply = fixture
            .execute(&fixture.request(mise::Action::Inspect), peer)
            .unwrap();
        let Some(project::Response::Mise(snapshot)) = reply.tool else {
            panic!("Missing mise snapshot")
        };
        assert_eq!(snapshot.tasks[0].name, "smoke");
        assert_eq!(snapshot.tools, ["node"]);
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("do-not-store-or-print")
        );
        assert!(!fixture.root.path().join("mise-installed").exists());
        fixture
            .execute(&fixture.request(mise::Action::Install), peer)
            .unwrap();
        assert!(fixture.root.path().join("mise-installed").exists());
    }
}

#[test]
fn provider_is_applied_before_native_or_python_tools_and_rode_overrides_win() {
    for peer in [false, true] {
        let mut fixture = Fixture::new();
        fixture.trusted();
        fixture
            .settings
            .variables
            .insert("RODE_MISE_VALUE".into(), "rode-override".into());
        fixture.program(
            "gofmt",
            "#!/bin/sh\nprintf 'formatted-%s\\n' \"$RODE_MISE_VALUE\"\n",
        );
        let request = files::Request::Tool {
            request: project::Request::Analyze {
                root: fixture.root(),
                path: fixture
                    .root
                    .path()
                    .join("main.go")
                    .to_string_lossy()
                    .into_owned(),
                text: "package main".into(),
                python: "auto".into(),
                ruff: String::new(),
                format: true,
                check: false,
                fix: false,
            },
        };
        let mut arguments =
            native_peer::environment_prefix(&fixture.settings, &fixture.root(), "auto").unwrap();
        if peer {
            arguments.extend([native_peer::binary().into(), "--file-helper".into()]);
        } else {
            arguments.extend([env!("CARGO_BIN_EXE_rode").into(), "--file-helper".into()]);
        }
        let mut command = Command::new(&arguments[0]);
        command.args(&arguments[1..]);
        let output = project::run(
            command,
            serde_json::to_vec(&request).unwrap(),
            Duration::from_secs(30),
        )
        .unwrap()
        .checked()
        .unwrap();
        let reply: files::Response = serde_json::from_str(&output).unwrap();
        let Some(project::Response::Analysis(analysis)) = reply.tool else {
            panic!("Missing analysis")
        };
        assert_eq!(analysis.text, "formatted-rode-override\n");
    }
}

#[test]
fn launch_preserves_protocol_stdin_and_refuses_invalid_provider_output_without_fallback() {
    let fixture = Fixture::new();
    fixture.trusted();
    let mut arguments =
        native_peer::environment_prefix(&fixture.settings, &fixture.root(), "auto").unwrap();
    arguments.extend([
        "/usr/bin/python3".into(),
        "-c".into(),
        "import os,sys;print(os.environ['RODE_MISE_VALUE']);print(sys.stdin.read())".into(),
    ]);
    let run = || {
        let mut command = Command::new(&arguments[0]);
        command.args(&arguments[1..]);
        project::run(command, b"protocol input".to_vec(), Duration::from_secs(20)).unwrap()
    };
    let output = run().checked().unwrap();
    assert_eq!(output, "from-mise\nprotocol input\n");
    fs::write(fixture.root.path().join("mise-invalid"), "").unwrap();
    let output = run();
    assert!(!output.success());
    assert!(output.stderr.contains("invalid") && output.stderr.contains("environment"));
    assert!(!output.stdout.contains("protocol input"));
    fs::remove_file(fixture.root.path().join("mise-invalid")).unwrap();
    fs::write(fixture.root.path().join("mise-missing"), "").unwrap();
    let output = run();
    assert!(!output.success());
    assert!(output.stderr.contains("Missing mise tools: node"));
    assert!(!fixture.root.path().join("mise-installed").exists());
}

#[test]
fn native_agent_controllers_honor_environment_prefixes_in_normal_mode() {
    let fixture = Fixture::new();
    fixture.trusted();
    let agent = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agent.py");
    fixture.program("mise-agent", &format!("#!/bin/sh\nprintf '%s' \"$RODE_MISE_VALUE\" > agent-environment.tmp\nmv agent-environment.tmp agent-environment\nexec /usr/bin/python3 {} \"$@\"\n",rode::ssh::quote(&agent.to_string_lossy())));
    let id = rode_session::new_id().unwrap();
    let policy = "b".repeat(64);
    let config = rode::agent::Config {
        codex: "mise-agent".into(),
        ..Default::default()
    };
    let spec = rode_agent_core::service::Spec {
        id: id.clone(),
        name: "mise agent".into(),
        policy: policy.clone(),
        mode: "Normal mode".into(),
        config,
        plan: rode_agent_core::Plan {
            root: fixture.root(),
            active: false,
            project_read_only: false,
            prefix: native_peer::environment_prefix(&fixture.settings, &fixture.root(), "auto")
                .unwrap(),
        },
        executable: "mise-agent".into(),
        file_tool_plan: None,
        env: Default::default(),
        python: "auto".into(),
        persistent: false.into(),
        text: Some("mise smoke".into()),
        attachments: vec![],
    };
    let request = |request| files::Request::Tool {
        request: project::Request::Sessions {
            root: fixture.root(),
            operation: rode::sessions::Operation::Agents {
                mise: true,
                request: Box::new(request),
            },
            directory: None,
        },
    };
    fixture
        .execute(
            &request(rode_session::Request::CreateAgent {
                spec: Box::new(spec),
            }),
            false,
        )
        .unwrap();
    let marker = fixture.root.path().join("agent-environment");
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !marker.exists() && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    let value = fs::read_to_string(marker);
    fixture
        .execute(
            &request(rode_session::Request::EndAgent { id, policy }),
            false,
        )
        .unwrap();
    assert_eq!(value.unwrap(), "from-mise");
}

#[test]
fn native_kernel_controllers_resolve_the_provider_before_starting_the_bridge() {
    let fixture = Fixture::new();
    fixture.trusted();
    let bridge = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/notebook_bridge.py");
    fixture.program("mise-kernel", &format!("#!/bin/sh\nprintf '%s' \"$RODE_MISE_VALUE\" > kernel-environment.tmp\nmv kernel-environment.tmp kernel-environment\nexec /usr/bin/python3 {}\n", rode::ssh::quote(&bridge.to_string_lossy())));
    let id = rode_session::new_id().unwrap();
    let policy = "c".repeat(64);
    let spec = rode_runtime_core::service::Spec {
        sources: Default::default(),
        id: id.clone(),
        name: "mise kernel".into(),
        root: fixture.root(),
        policy: policy.clone(),
        mode: "Normal mode".into(),
        document: fixture
            .root
            .path()
            .join("test.ipynb")
            .to_string_lossy()
            .into_owned(),
        argv: vec!["mise-kernel".into()],
        prefix: native_peer::environment_prefix(&fixture.settings, &fixture.root(), "auto")
            .unwrap(),
        env: Default::default(),
        python: "auto".into(),
        python_program: false,
        persistent: false,
        target: None,
        start: rode_runtime_core::service::Start::Notebook {
            name: "fixture".into(),
        },
    };
    let request = |request| files::Request::Tool {
        request: project::Request::Sessions {
            root: fixture.root(),
            operation: rode::sessions::Operation::Runtimes {
                mise: true,
                request: Box::new(request),
            },
            directory: None,
        },
    };
    fixture
        .execute(
            &request(rode_session::Request::CreateRuntime {
                spec: Box::new(spec),
            }),
            false,
        )
        .unwrap();
    let marker = fixture.root.path().join("kernel-environment");
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !marker.exists() && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    let value = fs::read_to_string(marker);
    fixture
        .execute(
            &request(rode_session::Request::EndRuntime { id, policy }),
            false,
        )
        .unwrap();
    assert_eq!(value.unwrap(), "from-mise");
}

#[test]
fn normal_mode_plan_wraps_project_tools_but_leaves_file_io_and_setup_available() {
    let fixture = Fixture::new();
    let mut plan = native_peer::prepare(&fixture.root(), &Default::default()).unwrap();
    plan.environment = fixture.settings.clone();
    // Setup bypasses the provider so an untrusted/missing environment can be repaired.
    plan.local_request_with_helper(
        &fixture.request(mise::Action::Trust),
        Path::new(env!("CARGO_BIN_EXE_rode")),
    )
    .unwrap();
    fixture.program("node", "#!/bin/sh\nprintf '%s\\n' \"$RODE_MISE_VALUE\"\n");
    let request = files::Request::Tool {
        request: project::Request::Runtimes {
            root: fixture.root(),
            settings: Default::default(),
        },
    };
    let reply = plan
        .local_request_with_helper(&request, Path::new(env!("CARGO_BIN_EXE_rode")))
        .unwrap();
    let Some(project::Response::Runtimes(tools)) = reply.tool else {
        panic!("Missing runtimes")
    };
    assert_eq!(
        tools
            .iter()
            .find(|tool| tool.name == "node")
            .unwrap()
            .version,
        "from-mise"
    );
    assert!(!plan.active);
    fs::write(fixture.root.path().join("mise-invalid"), "").unwrap();
    plan.local_request(&files::Request::List {
        path: fixture.root(),
    })
    .unwrap();
}

#[test]
fn real_mise_requires_trust_and_runs_a_project_task_without_installing_tools_when_requested() {
    let Ok(binary) = std::env::var("RODE_TEST_MISE") else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("mise.toml"),
        "[env]\nRODE_REAL_MISE='loaded'\n[tasks.smoke]\nrun='printf passed > task-result'\n",
    )
    .unwrap();
    let base = root.path().to_string_lossy().into_owned();
    let settings = environment::Settings {
        mise: true,
        runtimes: BTreeMap::from([("mise".into(), binary.clone())]),
        ..Default::default()
    };
    for (index, action) in [
        mise::Action::Inspect,
        mise::Action::Trust,
        mise::Action::Inspect,
    ]
    .into_iter()
    .enumerate()
    {
        let mut command = native_peer::command();
        command.current_dir(root.path());
        // TMPDIR may be inside a checkout with its own untrusted mise config.
        // Discover only this fixture's config, never its ancestors.
        command.env("MISE_CEILING_PATHS", root.path().parent().unwrap());
        for name in ["CONFIG", "DATA", "STATE", "CACHE"] {
            command.env(format!("MISE_{name}_DIR"), root.path().join(name));
        }
        let out = project::run(
            command,
            serde_json::to_vec(&files::Request::Tool {
                request: project::Request::Mise {
                    root: base.clone(),
                    settings: settings.clone(),
                    action,
                },
            })
            .unwrap(),
            Duration::from_secs(30),
        )
        .unwrap();
        if index == 0 {
            assert!(!out.success());
            assert!(out.stderr.contains("not trusted"));
        } else {
            assert!(out.success(), "{}", out.stderr);
        }
        // The first inspection fails; after the explicit trust, inspection discovers smoke.
        if matches!(action, mise::Action::Trust) {
            assert!(out.success(), "{}", out.stderr);
        }
        if out.success() && matches!(action, mise::Action::Inspect) {
            let reply: files::Response = serde_json::from_str(&out.stdout).unwrap();
            let Some(project::Response::Mise(snapshot)) = reply.tool else {
                panic!("Missing mise reply")
            };
            assert_eq!(snapshot.tasks[0].name, "smoke");
        }
    }
    let mut command = Command::new(binary);
    command
        .args(["run", "smoke"])
        .current_dir(root.path())
        .env("MISE_CEILING_PATHS", root.path().parent().unwrap())
        .env("MISE_AUTO_INSTALL", "0")
        .env("MISE_EXEC_AUTO_INSTALL", "0")
        .env("MISE_PARANOID", "1");
    for name in ["CONFIG", "DATA", "STATE", "CACHE"] {
        command.env(format!("MISE_{name}_DIR"), root.path().join(name));
    }
    assert!(command.status().unwrap().success());
    assert_eq!(
        fs::read_to_string(root.path().join("task-result")).unwrap(),
        "passed"
    );
}

//! Actual offline-tool enforcement while the agent controller reaches a local
//! model fixture. No credentials or real inference are involved.
#[path = "support/native.rs"]
mod native;
use rode::{
    agent::{Access, Config, Launch, Network, Provider, protocol::Chat},
    sandbox,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

fn plan(root: &std::path::Path) -> Option<sandbox::Plan> {
    let plan = native::prepare(
        root.to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            backend: sandbox::Backend::Landlock,
            share_network: false,
            ..Default::default()
        },
    )
    .unwrap();
    if !plan.active {
        assert_ne!(
            std::env::var("RODE_TEST_REQUIRE_LANDLOCK").as_deref(),
            Ok("1"),
            "{}",
            plan.reason
        );
        eprintln!("Landlock unavailable: {}", plan.reason);
        return None;
    }
    Some(plan)
}

fn wait(chat: &mut Chat) {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        chat.poll();
        if !chat.busy {
            break;
        }
        assert!(
            Instant::now() < until,
            "{}: {:?}",
            chat.status,
            chat.messages
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        chat.last_turn_succeeded,
        "{}: {:?}",
        chat.status, chat.messages
    );
}

#[test]
fn model_controller_can_connect_while_native_commands_and_children_stay_offline() {
    let root = tempfile::tempdir().unwrap();
    let Some(plan) = plan(root.path()) else {
        return;
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    fs::write(
        root.path().join("controller-probe-port"),
        listener.local_addr().unwrap().port().to_string(),
    )
    .unwrap();
    let code = r#"import socket, subprocess, sys
for family, kind in [(socket.AF_INET,socket.SOCK_STREAM),(socket.AF_INET,socket.SOCK_DGRAM),(socket.AF_UNIX,socket.SOCK_STREAM)]:
    try: socket.socket(family, kind)
    except PermissionError: pass
    else: raise AssertionError('Native command had network access')
subprocess.run([sys.executable, '-c', 'import socket\ntry: socket.socket()\nexcept PermissionError: print("CHILD_OFFLINE")\nelse: raise AssertionError("Child had network access")'], check=True)
open('built.txt','w').write('offline build')
print('NATIVE_OFFLINE_OK')"#;
    let steps = json!([
        {"params":{"tool":"rode_run_command","arguments":{"command":format!("/usr/bin/python3 -c {}", rode::ssh::quote(code))}}},
        {"params":{"tool":"rode_run_command","arguments":{"command":"printf diagnostic >&2; exit 17"}}},
        {"params":{"tool":"exec_command","arguments":{"cmd":"touch forbidden"}}},
        {"method":"item/commandExecution/requestApproval","params":{"command":"curl https://example.invalid"}},
        {"params":{"tool":"rode_write_file","arguments":{"path":"edited.txt","text":"native edit","revision":null}}},
        {"params":{"tool":"rode_run_command","arguments":{"command":"touch forbidden"},"threadId":"foreign"}},
        {"params":{"tool":"rode_run_command","arguments":{"command":"touch forbidden"},"turnId":"previous"}},
        {"params":{"tool":"rode_run_command","arguments":{"command":"touch forbidden"},"namespace":"foreign"}}
    ]);
    fs::write(root.path().join("file-tool-steps.json"), steps.to_string()).unwrap();
    let executable = root.path().join("agent.py");
    fs::write(&executable, include_str!("fixtures/file_only_agent.py")).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let launch = Launch {
        config: Config {
            codex: executable.to_string_lossy().into(),
            network: Network::ModelOnly,
            access: Access::Project,
            ..Default::default()
        },
        plan,
        remote: None,
        python: "off".into(),
    };
    let mut chat = Chat::default();
    chat.submit(launch, "Build and edit offline".into(), Arc::new(|| {}))
        .unwrap();
    wait(&mut chat);
    let (mut connection, _) = listener
        .accept()
        .expect("Model controller could not reach its endpoint");
    connection
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut probe = String::new();
    connection.read_to_string(&mut probe).unwrap();
    assert_eq!(probe, "MODEL_CONTROLLER_OK");
    assert_eq!(
        fs::read_to_string(root.path().join("built.txt")).unwrap(),
        "offline build"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("edited.txt")).unwrap(),
        "native edit"
    );
    assert!(!root.path().join("forbidden").exists());
    assert!(chat.prompts.is_empty());
    let calls: Vec<Value> = fs::read_to_string(root.path().join("file-agent-rpc.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(calls.iter().any(|v| {
        v["id"] == 100
            && v["result"]["success"] == true
            && v["result"]["contentItems"][0]["text"]
                .as_str()
                .unwrap()
                .contains("NATIVE_OFFLINE_OK")
    }));
    assert!(calls.iter().any(|v| {
        v["id"] == 101
            && v["result"]["contentItems"][0]["text"]
                .as_str()
                .unwrap()
                .contains("\"exit_code\":17")
    }));
    assert!(
        calls
            .iter()
            .any(|v| v["id"] == 102 && v["result"]["success"] == false)
    );
    assert!(
        calls
            .iter()
            .any(|v| v["id"] == 103 && v["result"]["decision"] == "decline")
    );
    let start = calls.iter().find(|v| v["method"] == "turn/start").unwrap();
    assert_eq!(start["params"]["sandboxPolicy"]["type"], "readOnly");
}

#[test]
fn mode_requires_supported_provider_and_an_enforced_offline_policy() {
    let root = tempfile::tempdir().unwrap();
    let mut config = Config {
        network: Network::ModelOnly,
        ..Default::default()
    };
    for provider in [Provider::Acp, Provider::Claude] {
        config.provider = provider;
        assert!(
            config
                .argv()
                .unwrap_err()
                .to_string()
                .contains("cannot enforce offline tools")
        );
    }
    for provider in [Provider::Codex, Provider::Local] {
        config.provider = provider;
        config.local_model = "fixture".into();
        assert!(config.argv().is_ok());
    }
    let policy = rode_agent_core::Plan {
        root: root.path().to_string_lossy().into(),
        ..Default::default()
    };
    assert!(policy.controller_prefix(Network::ModelOnly).is_err());
    let policy = rode_agent_core::Plan {
        active: true,
        prefix: vec!["helper".into(), "--share-net".into(), "--".into()],
        ..policy
    };
    assert!(policy.controller_prefix(Network::ModelOnly).is_err());
    let policy = rode_agent_core::Plan {
        prefix: vec![
            "helper".into(),
            "--".into(),
            "--mise-launch".into(),
            "--".into(),
        ],
        ..policy
    };
    assert!(policy.controller_prefix(Network::ModelOnly).is_err());
    let policy = rode_agent_core::Plan {
        prefix: vec!["helper".into(), "--landlock-launch".into(), "--".into()],
        ..policy
    };
    let original = policy.prefix.clone();
    assert!(
        policy
            .controller_prefix(Network::ModelOnly)
            .unwrap()
            .contains(&"--share-net".into())
    );
    assert_eq!(policy.prefix, original);
    assert!(!config.same_process(&Config {
        network: Network::Project,
        ..config.clone()
    }));
    assert!(!config.same_process(&Config {
        access: Access::ReadOnly,
        ..config.clone()
    }));
}

#[test]
fn native_commands_reject_invalid_limits_timeout_overflow_and_missing_enforcement() {
    let root = tempfile::tempdir().unwrap();
    let Some(plan) = plan(root.path()) else {
        return;
    };
    let env = std::env::vars().collect();
    let run = |args: Value| {
        rode_host_core::agent_commands::run(&plan, &args, &env, &AtomicBool::new(false))
    };
    for args in [
        json!({"command":""}),
        json!({"command":"true","timeout_seconds":0}),
        json!({"command":"true","timeout_seconds":121}),
        json!({"command":"true","cwd":"/"}),
    ] {
        assert!(run(args).is_err());
    }
    assert!(run(json!({"command":"sleep 30","timeout_seconds":1})).is_err());
    assert!(run(json!({"command":"/usr/bin/python3 -c 'print(\"x\"*300000)'"})).is_err());
    let cancel = AtomicBool::new(true);
    assert!(
        rode_host_core::agent_commands::run(
            &plan,
            &json!({"command":"touch cancelled"}),
            &env,
            &cancel
        )
        .is_err()
    );
    assert!(!root.path().join("cancelled").exists());
    let mut inactive = plan;
    inactive.active = false;
    assert!(
        rode_host_core::agent_commands::run(
            &inactive,
            &json!({"command":"touch bypass"}),
            &env,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(!root.path().join("bypass").exists());
}

#[test]
fn installed_codex_uses_native_offline_tools_with_a_live_model_connection() {
    let Ok(program) = std::env::var("RODE_TEST_CODEX") else {
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("project");
    fs::create_dir(&root).unwrap();
    let Some(mut policy) = plan(&root) else {
        return;
    };
    // The installed optional runtime remains read-only inside the outer policy.
    let mut args = policy.prefix.clone();
    let at = args.len() - 1;
    let runtime = std::path::Path::new(&program)
        .parent()
        .unwrap()
        .to_str()
        .unwrap();
    args.splice(at..at, ["--ro-bind".into(), runtime.into(), runtime.into()]);
    policy.prefix = args;
    let home = root.join("codex-home");
    fs::create_dir(&home).unwrap();
    let marker = root.join("disabled-integration");
    fs::write(home.join("config.toml"), format!(
        "notify=[\"/usr/bin/touch\",{}]\n[features]\ncode_mode=true\nhooks=true\n[mcp_servers.forbidden]\ncommand=\"/usr/bin/touch\"\nargs=[{}]\n",
        serde_json::to_string(marker.to_str().unwrap()).unwrap(), serde_json::to_string(marker.to_str().unwrap()).unwrap()
    )).unwrap();
    let wrapper = root.join("codex-wrapper");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport CODEX_HOME={}\nexec {} \"$@\"\n",
            rode::ssh::quote(home.to_str().unwrap()),
            rode::ssh::quote(&program)
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(base.path().join("offline-mode"), "").unwrap();
    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _server = Server(
        std::process::Command::new("python3")
            .args([
                "-c",
                include_str!("fixtures/responses.py"),
                base.path().to_str().unwrap(),
            ])
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(5);
    while !base.path().join("port").exists() {
        assert!(Instant::now() < until, "Model fixture did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let port = fs::read_to_string(base.path().join("port")).unwrap();
    let mut chat = Chat::default();
    chat.submit(
        Launch {
            config: Config {
                provider: Provider::Local,
                codex: wrapper.to_string_lossy().into(),
                network: Network::ModelOnly,
                local_model: "fixture-local-model".into(),
                local_url: format!("http://127.0.0.1:{port}/v1"),
                ..Default::default()
            },
            plan: policy,
            remote: None,
            python: "off".into(),
        },
        "Build offline without using external tools".into(),
        Arc::new(|| {}),
    )
    .unwrap();
    wait(&mut chat);
    assert_eq!(
        fs::read_to_string(root.join("offline-build.txt")).unwrap(),
        "OFFLINE_BUILD_OK"
    );
    assert!(
        fs::read_to_string(root.join("network.txt"))
            .unwrap()
            .contains("PermissionError")
    );
    assert!(
        !marker.exists(),
        "Disabled hooks, notifications or MCP command executed"
    );
    let request: Value =
        serde_json::from_str(&fs::read_to_string(base.path().join("request.json")).unwrap())
            .unwrap();
    let tools = request["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "rode_run_command"));
    for name in [
        "exec_command",
        "shell_command",
        "mcp__",
        "browser",
        "computer_use",
        "spawn_agent",
    ] {
        assert!(
            !serde_json::to_string(tools).unwrap().contains(name),
            "Unexpected tool {name}: {tools:?}"
        );
    }
}

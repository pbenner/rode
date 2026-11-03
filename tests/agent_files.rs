//! File-only launch policy, native callbacks and adversarial protocol requests.
#[path = "support/native.rs"]
mod native;
use rode::{
    agent::{Access, Config, Launch, Provider, protocol::Chat},
    sandbox,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn fixture(root: &Path, steps: Value) -> Launch {
    fs::write(root.join("file-tool-steps.json"), steps.to_string()).unwrap();
    let executable = root.join("agent.py");
    fs::write(&executable, include_str!("fixtures/file_only_agent.py")).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    Launch {
        config: Config {
            codex: executable.to_string_lossy().into(),
            access: Access::FilesOnly,
            ..Default::default()
        },
        plan: sandbox::Plan {
            root: root.to_string_lossy().into(),
            native_helper: Some(native::binary().into()),
            ..Default::default()
        },
        remote: None,
        python: "off".into(),
    }
}
fn wait(chat: &mut Chat) {
    let until = Instant::now() + Duration::from_secs(15);
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
}
fn log(root: &Path) -> Vec<Value> {
    fs::read_to_string(root.join("file-agent-rpc.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn file_only_tools_edit_only_the_project_and_decline_execution_and_escalation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    fs::create_dir(&root).unwrap();
    fs::write(temporary.path().join("private"), "private").unwrap();
    symlink(temporary.path(), root.join("escape")).unwrap();
    let launch = fixture(
        &root,
        json!([
            {"params":{"tool":"rode_list_files","arguments":{"path":"."}}},
            {"params":{"tool":"rode_write_file","arguments":{"path":"created.py","text":"print('hello')\n","revision":null}}},
            {"params":{"tool":"rode_read_file","arguments":{"path":"created.py"}}},
            {"params":{"tool":"exec_command","arguments":{"cmd":"touch forbidden"}}},
            {"params":{"tool":"rode_write_file","arguments":{"path":"../private","text":"damaged","revision":null}}},
            {"params":{"tool":"rode_read_file","arguments":{"path":"escape/private"}}},
            {"params":{"tool":"rode_write_file","arguments":{"path":"foreign.py","text":"foreign","revision":null},"threadId":"foreign"}},
            {"params":{"tool":"rode_write_file","arguments":{"path":"late.py","text":"late","revision":null},"turnId":"previous"}},
            {"method":"item/commandExecution/requestApproval","params":{"command":"touch forbidden"}},
            {"method":"item/fileChange/requestApproval","params":{"reason":"write outside"}},
            {"method":"item/permissions/requestApproval","params":{"permissions":{"fileSystem":{"write":["/"]}}}},
            {"method":"terminal/create","params":{"command":"touch forbidden"}}
            ,{"params":{"tool":"rode_write_file","arguments":{"path":"agent.py","text":"malicious replacement","revision":rode::files::digest(include_bytes!("fixtures/file_only_agent.py"))}}}
        ]),
    );
    let mut chat = Chat::default();
    chat.submit(launch.clone(), "Edit files".into(), Arc::new(|| {}))
        .unwrap();
    wait(&mut chat);
    assert!(chat.last_turn_succeeded, "{:?}", chat.messages);
    assert!(chat.prompts.is_empty());
    assert_eq!(
        fs::read_to_string(root.join("created.py")).unwrap(),
        "print('hello')\n"
    );
    assert_eq!(
        fs::read_to_string(temporary.path().join("private")).unwrap(),
        "private"
    );
    for name in ["forbidden", "foreign.py", "late.py"] {
        assert!(!root.join(name).exists());
    }
    let calls = log(&root);
    for id in [100, 101, 102] {
        assert_eq!(
            calls
                .iter()
                .find(|v| v["id"] == id && v.get("method").is_none())
                .unwrap()["result"]["success"],
            true
        );
    }
    for id in 103..108 {
        assert_eq!(
            calls
                .iter()
                .find(|v| v["id"] == id && v.get("method").is_none())
                .unwrap()["result"]["success"],
            false
        );
    }
    for id in [108, 109] {
        assert_eq!(
            calls
                .iter()
                .find(|v| v["id"] == id && v.get("method").is_none())
                .unwrap()["result"]["decision"],
            "decline"
        );
    }
    assert_eq!(
        calls
            .iter()
            .find(|v| v["id"] == 110 && v.get("method").is_none())
            .unwrap()["result"]["permissions"],
        json!({})
    );
    assert!(
        calls
            .iter()
            .find(|v| v["id"] == 111 && v.get("method").is_none())
            .unwrap()
            .get("error")
            .is_some()
    );
    assert_eq!(
        calls
            .iter()
            .find(|v| v["id"] == 112 && v.get("method").is_none())
            .unwrap()["result"]["success"],
        false
    );
    let turn = calls.iter().find(|v| v["method"] == "turn/start").unwrap();
    assert_eq!(turn["params"]["sandboxPolicy"]["type"], "readOnly");
    assert_eq!(turn["params"]["approvalPolicy"], "never");
    // Entering/leaving file-only access must replace the process, not just its next-turn policy.
    let mut normal = launch.config.clone();
    normal.access = Access::Project;
    assert!(!launch.config.same_process(&normal));
}

#[test]
fn an_ignored_shell_restriction_stops_before_any_thread_or_model_turn() {
    let temporary = tempfile::tempdir().unwrap();
    let launch = fixture(temporary.path(), json!([]));
    fs::write(temporary.path().join("ignore-shell-restriction"), "").unwrap();
    let mut chat = Chat::default();
    chat.submit(launch, "Must not send".into(), Arc::new(|| {}))
        .unwrap();
    wait(&mut chat);
    assert!(!chat.connected());
    assert!(!chat.last_turn_succeeded);
    assert!(
        !log(temporary.path())
            .iter()
            .any(|v| matches!(v["method"].as_str(), Some("thread/start" | "turn/start")))
    );
    assert!(
        chat.messages
            .iter()
            .any(|m| m.text.contains("did not confirm"))
    );
}

#[test]
fn read_only_projects_keep_file_tools_read_only_and_acp_cannot_claim_this_mode() {
    let temporary = tempfile::tempdir().unwrap();
    let mut launch = fixture(
        temporary.path(),
        json!([
            {"params":{"tool":"rode_write_file","arguments":{"path":"blocked","text":"blocked","revision":null}}}
        ]),
    );
    launch.plan.project_read_only = true;
    let mut chat = Chat::default();
    chat.submit(launch.clone(), "write".into(), Arc::new(|| {}))
        .unwrap();
    wait(&mut chat);
    assert!(chat.last_turn_succeeded);
    assert!(!temporary.path().join("blocked").exists());
    assert_eq!(
        log(temporary.path())
            .iter()
            .find(|v| v["id"] == 100 && v.get("method").is_none())
            .unwrap()["result"]["success"],
        false
    );
    launch.config.provider = Provider::Acp;
    assert!(
        launch
            .config
            .argv()
            .unwrap_err()
            .to_string()
            .contains("requires Codex")
    );
}

#[test]
fn an_oversized_encoded_file_returns_a_tool_error_without_disconnecting() {
    let temporary = tempfile::tempdir().unwrap();
    fs::write(temporary.path().join("escaped.txt"), vec![1; 1024 * 1024]).unwrap();
    let launch = fixture(
        temporary.path(),
        json!([
            {"params":{"tool":"rode_read_file","arguments":{"path":"escaped.txt"}}},
            {"params":{"tool":"rode_write_file","arguments":{"path":"after.txt","text":"still connected","revision":null}}}
        ]),
    );
    let mut chat = Chat::default();
    chat.submit(launch, "Read".into(), Arc::new(|| {})).unwrap();
    wait(&mut chat);
    assert!(chat.last_turn_succeeded, "{:?}", chat.messages);
    assert!(chat.connected());
    assert_eq!(
        fs::read_to_string(temporary.path().join("after.txt")).unwrap(),
        "still connected"
    );
    assert!(
        chat.messages
            .iter()
            .any(|m| m.text.contains("exceeds the protocol limit"))
    );
}

// Separate process: Landlock is irreversible and must not affect test threads.
#[test]
fn confined_file_helper_child() {
    use std::{
        io::{Read, Write},
        os::fd::AsRawFd,
    };
    let Ok(root) = std::env::var("RODE_FILE_CONFINEMENT_ROOT") else {
        return;
    };
    let directory = fs::File::open(&root).unwrap();
    let captured = fs::File::open(Path::new(&root).join("moving")).unwrap();
    rode_host_core::landlock::confine_agent_files(&directory, false).unwrap();
    fs::write(Path::new(&root).join("allowed"), "allowed").unwrap();
    assert!(std::process::Command::new("/bin/true").status().is_err());
    println!("CONFINED");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0]).unwrap();
    let redirected = format!("/proc/self/fd/{}/escaped", captured.as_raw_fd());
    assert!(fs::write(redirected, "must stay inside").is_err());
}

#[test]
fn captured_directories_cannot_be_moved_outside_the_helper_boundary() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    };
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    fs::create_dir_all(root.join("moving")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "confined_file_helper_child", "--nocapture"])
        .env("RODE_FILE_CONFINEMENT_ROOT", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(
            output.read_line(&mut line).unwrap(),
            0,
            "Confinement child exited before readiness"
        );
        if line.trim() == "CONFINED" {
            break;
        }
    }
    let outside = temporary.path().join("outside");
    fs::rename(root.join("moving"), &outside).unwrap();
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    // Drain the harness output before waiting, so stdout can never fill its pipe.
    std::io::copy(&mut output, &mut std::io::sink()).unwrap();
    assert!(child.wait().unwrap().success());
    assert!(!outside.join("escaped").exists());
    assert_eq!(fs::read_to_string(root.join("allowed")).unwrap(), "allowed");
}

#[test]
fn file_only_helpers_keep_sandbox_subpaths_read_only_and_bypass_project_providers() {
    for backend in [sandbox::Backend::Bwrap, sandbox::Backend::Landlock] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir(root.join("protected")).unwrap();
        fs::write(root.join("protected/source"), "protected").unwrap();
        let mut launch = fixture(
            root,
            json!([
                {"params":{"tool":"rode_write_file","arguments":{"path":"allowed","text":"allowed","revision":null}}},
                {"params":{"tool":"rode_write_file","arguments":{"path":"protected/source","text":"replacement","revision":rode::files::digest(b"protected")}}}
            ]),
        );
        launch.plan = native::prepare(
            root.to_str().unwrap(),
            &sandbox::Options {
                enabled: true,
                backend,
                read_only: vec![root.join("protected").to_string_lossy().into()],
                ..Default::default()
            },
        )
        .unwrap();
        if !launch.plan.active {
            continue;
        }
        launch.plan.environment.mise = true;
        launch
            .plan
            .environment
            .runtimes
            .insert("mise".into(), "/bin/false".into());
        let mut chat = Chat::default();
        chat.submit(launch, "edit".into(), Arc::new(|| {})).unwrap();
        wait(&mut chat);
        assert!(chat.last_turn_succeeded, "{:?}", chat.messages);
        assert_eq!(fs::read_to_string(root.join("allowed")).unwrap(), "allowed");
        assert_eq!(
            fs::read_to_string(root.join("protected/source")).unwrap(),
            "protected"
        );
        assert_eq!(
            log(root)
                .iter()
                .find(|v| v["id"] == 101 && v.get("method").is_none())
                .unwrap()["result"]["success"],
            false
        );
    }
}

#[test]
fn installed_codex_file_only_tools_work_without_shell_mcp_hooks_or_external_inference() {
    let Ok(program) = std::env::var("RODE_TEST_CODEX") else {
        return;
    };
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    fs::create_dir(&root).unwrap();
    let home = temporary.path().join("codex-home");
    fs::create_dir(&home).unwrap();
    let marker = temporary.path().join("must-not-execute");
    fs::write(home.join("config.toml"), format!(
        "notify = [\"/usr/bin/touch\", {}]\n[features]\ncode_mode=true\nhooks=true\n[mcp_servers.forbidden]\ncommand=\"/usr/bin/touch\"\nargs=[{}]\n",
        serde_json::to_string(marker.to_str().unwrap()).unwrap(),serde_json::to_string(marker.to_str().unwrap()).unwrap()
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
    fs::write(temporary.path().join("file-mode"), "").unwrap();
    fs::write(temporary.path().join("file-attack"), "").unwrap();
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
                temporary.path().to_str().unwrap(),
            ])
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(5);
    while !temporary.path().join("port").exists() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let port = fs::read_to_string(temporary.path().join("port")).unwrap();
    let launch = Launch {
        config: Config {
            provider: Provider::Local,
            codex: wrapper.to_string_lossy().into(),
            access: Access::FilesOnly,
            local_model: "fixture-local-model".into(),
            local_url: format!("http://127.0.0.1:{port}/v1"),
            ..Default::default()
        },
        plan: sandbox::Plan {
            root: root.to_string_lossy().into(),
            native_helper: Some(native::binary().into()),
            ..Default::default()
        },
        remote: None,
        python: "off".into(),
    };
    let mut chat = Chat::default();
    chat.submit(
        launch,
        "Edit a file without commands.".into(),
        Arc::new(|| {}),
    )
    .unwrap();
    wait(&mut chat);
    assert!(chat.last_turn_succeeded, "{:?}", chat.messages);
    assert_eq!(
        fs::read_to_string(root.join("native-file.txt")).unwrap(),
        "Native file-only edit.\n"
    );
    assert!(
        !marker.exists(),
        "Codex ran a disabled executable integration"
    );
    assert!(
        !temporary.path().join("outside-patch.txt").exists(),
        "Built-in patch tool escaped read-only access"
    );
    assert!(
        !root.join("builtin-patch.txt").exists(),
        "Built-in patch tool bypassed the native file boundary"
    );
    let request: Value =
        serde_json::from_str(&fs::read_to_string(temporary.path().join("request.json")).unwrap())
            .unwrap();
    let tools = request["tools"].as_array().unwrap();
    let serialized = serde_json::to_string(tools).unwrap();
    for forbidden in [
        "exec_command",
        "shell_command",
        "write_stdin",
        "mcp__",
        "browser",
        "computer_use",
        "spawn_agent",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "Unexpected tool {forbidden}: {tools:?}"
        );
    }
    assert!(serialized.contains("rode_write_file"));
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or_default();
        assert!(
            rode_agent_core::file_only::supported(name)
                || matches!(
                    name,
                    "update_plan"
                        | "request_user_input"
                        | "apply_patch"
                        | "get_goal"
                        | "create_goal"
                        | "update_goal"
                ),
            "Unexpected file-only tool: {tool}"
        );
    }
}

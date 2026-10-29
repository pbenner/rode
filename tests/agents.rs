//! Codex/ACP protocol, cancellation, sandbox scope and owned subprocess cleanup using
//! deterministic peers.

use rode::{
    agent::{
        Config, Launch, Provider,
        protocol::{Chat, Role},
    },
    sandbox,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project ' 🦀");
    fs::create_dir(&root).unwrap();
    fs::write(temp.path().join("private.txt"), "not part of the project").unwrap();
    let program = root.join("agent.py");
    fs::write(&program, include_str!("fixtures/agent.py")).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    (temp, root, program)
}
fn launch(root: &Path, program: &Path, provider: Provider, isolated: bool) -> Launch {
    let plan = sandbox::prepare(
        root.to_str().unwrap(),
        &sandbox::Options {
            enabled: isolated,
            ..Default::default()
        },
    )
    .unwrap();
    Launch {
        config: Config {
            provider,
            codex: program.to_string_lossy().into(),
            acp_program: program.to_string_lossy().into(),
            ..Default::default()
        },
        plan,
        remote: None,
        python: "auto".into(),
    }
}
fn wait(chat: &mut Chat, check: impl Fn(&Chat) -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        chat.poll();
        if check(chat) {
            return;
        }
        assert!(
            Instant::now() < until,
            "{}: {}",
            chat.status,
            chat.messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn submit(chat: &mut Chat, launch: Launch, text: &str) {
    chat.submit(launch, text.into(), Arc::new(|| {})).unwrap();
}
fn log(root: &Path) -> Vec<Value> {
    fs::read_to_string(root.join("agent-rpc.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn both_protocols_stream_keep_context_and_relay_explicit_approvals() {
    for provider in [Provider::Codex, Provider::Acp] {
        let (_temp, root, program) = fixture();
        let launch = launch(&root, &program, provider, false);
        let mut chat = Chat::default();
        submit(&mut chat, launch.clone(), "first");
        wait(&mut chat, |c| c.completed == 1);
        submit(&mut chat, launch.clone(), "second");
        wait(&mut chat, |c| c.completed == 2);
        assert!(
            chat.messages
                .iter()
                .any(|m| m.role == Role::Assistant && m.text.contains("second (turn 2)"))
        );
        assert_eq!(
            log(&root)
                .iter()
                .filter(|m| m["method"] == "initialize")
                .count(),
            1
        );
        submit(&mut chat, launch.clone(), "approve");
        wait(&mut chat, |c| !c.prompts.is_empty());
        assert!(!root.join("approved.txt").exists());
        let decline = chat.prompts[0].choices[1].result.clone();
        chat.answer(decline).unwrap();
        wait(&mut chat, |c| c.completed == 3);
        assert!(!root.join("approved.txt").exists());
        submit(&mut chat, launch, "approve");
        wait(&mut chat, |c| !c.prompts.is_empty());
        let allow = chat.prompts[0].choices[0].result.clone();
        chat.answer(allow).unwrap();
        wait(&mut chat, |c| c.completed == 4);
        assert!(root.join("approved.txt").exists());
    }
}

#[test]
fn questions_interrupts_and_final_output_survive_process_exit() {
    let (_temp, root, program) = fixture();
    let launch = launch(&root, &program, Provider::Codex, false);
    let mut chat = Chat::default();
    submit(&mut chat, launch.clone(), "question");
    wait(&mut chat, |c| !c.prompts.is_empty());
    chat.answer(json!({"answers":{"test_mode":{"answers":["All"]}}}))
        .unwrap();
    wait(&mut chat, |c| c.completed == 1);
    assert!(
        fs::read_to_string(root.join("answers.json"))
            .unwrap()
            .contains("All")
    );
    submit(&mut chat, launch.clone(), "wait");
    wait(&mut chat, |c| {
        c.messages.iter().any(|m| m.text.contains("long task"))
    });
    chat.cancel();
    wait(&mut chat, |c| !c.busy);
    assert!(chat.connected());
    submit(&mut chat, launch, "exit");
    wait(&mut chat, |c| !c.busy);
    wait(&mut chat, |c| !c.connected());
    assert!(
        chat.messages
            .iter()
            .any(|m| m.text.contains("Last reply before exit"))
    );
}

#[test]
fn cancelling_acp_permission_requests_returns_cancelled_and_keeps_the_session() {
    let (_temp, root, program) = fixture();
    let launch = launch(&root, &program, Provider::Acp, false);
    let mut chat = Chat::default();
    submit(&mut chat, launch, "approve");
    wait(&mut chat, |c| !c.prompts.is_empty());
    chat.cancel();
    wait(&mut chat, |c| !c.busy);
    assert!(
        log(&root)
            .iter()
            .any(|v| v["result"]["outcome"]["outcome"] == "cancelled")
    );
    assert!(chat.connected());
    assert!(!root.join("approved.txt").exists());
}

#[test]
fn actual_sandbox_encloses_the_agent_and_tools_without_fallback() {
    let (_temp, root, program) = fixture();
    let launch = launch(&root, &program, Provider::Codex, true);
    let active = launch.plan.active;
    let mut chat = Chat::default();
    submit(&mut chat, launch, "sandbox");
    wait(&mut chat, |c| c.completed == 1);
    let scope: Value =
        serde_json::from_str(&fs::read_to_string(root.join("agent-scope.json")).unwrap()).unwrap();
    assert_eq!(scope["cwd"], root.to_string_lossy().as_ref());
    assert_eq!(scope["private_visible"], !active);
    assert_eq!(
        fs::read_to_string(root.join("agent-created.txt")).unwrap(),
        "sandbox"
    );
    let turns = log(&root);
    let turn = turns.iter().find(|v| v["method"] == "turn/start").unwrap();
    if active {
        assert_eq!(turn["params"]["sandboxPolicy"]["type"], "externalSandbox");
    } else {
        assert!(turn["params"].get("sandboxPolicy").is_none());
        eprintln!("Host cannot start bwrap; explicit normal-mode launch checked.");
    }
}

#[test]
fn malformed_or_oversized_output_stops_the_process_and_reports_an_error() {
    for input in ["bad-protocol", "overflow"] {
        let (_temp, root, program) = fixture();
        let launch = launch(&root, &program, Provider::Codex, false);
        let mut chat = Chat::default();
        submit(&mut chat, launch, input);
        wait(&mut chat, |c| !c.busy);
        assert!(!chat.connected());
        assert!(chat.messages.iter().any(|m| m.role == Role::Notice));
    }
}

#[test]
fn local_provider_uses_only_the_configured_endpoint_and_no_cloud_credentials() {
    let config = Config {
        provider: Provider::Local,
        local_model: "installed-model".into(),
        local_url: "http://127.0.0.1:11434/v1".into(),
        ..Default::default()
    };
    let args = config.argv().unwrap();
    assert!(
        args.iter()
            .any(|a| a == "model_providers.rode_local.base_url=\"http://127.0.0.1:11434/v1\"")
    );
    assert!(
        args.iter()
            .any(|a| a == "model_providers.rode_local.requires_openai_auth=false")
    );
    assert!(
        Config {
            provider: Provider::Local,
            ..Default::default()
        }
        .argv()
        .is_err()
    );
    let mut bad = config;
    bad.local_url = "file:///etc/passwd".into();
    assert!(bad.argv().is_err());
}

#[test]
fn read_only_projects_block_agent_writes_when_bwrap_is_available() {
    let (_temp, root, program) = fixture();
    let mut request = launch(&root, &program, Provider::Codex, false);
    request.plan = sandbox::prepare(
        root.to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            project_read_only: true,
            ..Default::default()
        },
    )
    .unwrap();
    if !request.plan.active {
        eprintln!("Host cannot start bwrap; read-only agent test skipped.");
        return;
    }
    let mut chat = Chat::default();
    submit(&mut chat, request, "try-write");
    wait(&mut chat, |c| c.completed == 1);
    assert!(!root.join("blocked.txt").exists());
    assert!(
        chat.messages
            .iter()
            .any(|m| m.text.contains("Write blocked"))
    );
}

#[test]
fn dropping_a_session_stops_its_agent_and_command_children() {
    let (_temp, root, program) = fixture();
    let request = launch(&root, &program, Provider::Codex, false);
    let mut chat = Chat::default();
    submit(&mut chat, request, "wait");
    wait(&mut chat, |c| {
        c.messages.iter().any(|m| m.text.contains("long task"))
    });
    let pid = fs::read_to_string(root.join("agent-child.pid")).unwrap();
    drop(chat);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let running = fs::read_to_string(format!("/proc/{pid}/stat"))
            .is_ok_and(|s| s.split_whitespace().nth(2) != Some("Z"));
        if !running {
            break;
        }
        assert!(
            Instant::now() < until,
            "agent command child is still running"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_brief_executable_write_does_not_break_agent_startup() {
    let (_temp, root, program) = fixture();
    let writing = fs::OpenOptions::new().write(true).open(&program).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(writing);
    });
    let mut chat = Chat::default();
    submit(
        &mut chat,
        launch(&root, &program, Provider::Codex, false),
        "busy-script",
    );
    wait(&mut chat, |c| c.completed == 1);
    release.join().unwrap();
    assert!(
        chat.messages
            .iter()
            .any(|m| m.role == Role::Assistant && m.text.contains("busy-script"))
    );
}

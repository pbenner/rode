//! History is agent-owned: browsing never sends a prompt, and restore replaces
//! the current process only after a project-validated acknowledgement and replay.
use rode::{
    agent::{
        Access, Approval, Config, Launch, Provider,
        history::Browser,
        protocol::{Chat, Role},
    },
    sandbox,
};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn fixture(provider: Provider, isolated: bool) -> (tempfile::TempDir, Launch) {
    let temp = tempfile::tempdir().unwrap();
    let program = temp.path().join("agent.py");
    fs::write(&program, include_str!("fixtures/agent.py")).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let launch = Launch {
        config: Config {
            provider,
            codex: program.to_string_lossy().into(),
            acp_program: program.to_string_lossy().into(),
            ..Default::default()
        },
        plan: sandbox::prepare(
            temp.path().to_str().unwrap(),
            &sandbox::Options {
                enabled: isolated,
                ..Default::default()
            },
        )
        .unwrap(),
        remote: None,
        python: "off".into(),
    };
    (temp, launch)
}
fn wait(browser: &mut Browser) {
    let until = Instant::now() + Duration::from_secs(10);
    while browser.busy {
        browser.poll();
        assert!(Instant::now() < until, "{}", browser.status);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn wait_chat(chat: &mut Chat) {
    let until = Instant::now() + Duration::from_secs(10);
    while chat.busy {
        chat.poll();
        assert!(Instant::now() < until, "{}", chat.status);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn log(root: &Path) -> Vec<Value> {
    fs::read_to_string(root.join("agent-rpc.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn both_protocols_list_without_inference_filter_projects_page_and_resume_in_place() {
    for provider in [Provider::Codex, Provider::Acp] {
        for (requested, unavailable) in [(false, false), (true, false), (true, true)] {
            let (temp, mut launch) = fixture(provider, requested && !unavailable);
            if unavailable {
                // Exercise requested-but-inactive fallback even on hosts where
                // the real probe succeeds. Do not fabricate an active sandbox.
                launch.plan.requested = true;
                launch.plan.reason = "Sandbox unavailable (fixture)".into();
            }
            let active = launch.plan.active;
            launch.config.access = Access::Full;
            launch.config.approval = Approval::Never;
            let mut browser = Browser::open(launch.clone(), Arc::new(|| {})).unwrap();
            wait(&mut browser);
            assert!(browser.can_resume, "{}", browser.status);
            assert_eq!(browser.entries.len(), 1);
            assert_eq!(browser.entries[0].id, "saved-1");
            assert!(browser.more);
            let calls = log(temp.path());
            assert!(calls.iter().all(|v| !matches!(
                v["method"].as_str(),
                Some("thread/start" | "session/new" | "turn/start" | "session/prompt")
            )));
            browser.load_more().unwrap();
            wait(&mut browser);
            assert_eq!(browser.entries.len(), 2);
            assert!(!browser.more);
            assert!(browser.resume("foreign").is_err());
            browser.resume("saved-1").unwrap();
            wait(&mut browser);
            let restored = browser
                .take_restored()
                .unwrap_or_else(|| panic!("{}", browser.status));
            let mut chat = Chat::default();
            chat.restore(restored);
            assert!(chat.connected());
            let texts: Vec<_> = chat
                .messages
                .iter()
                .filter(|m| m.role != Role::Tool)
                .map(|m| m.text.as_str())
                .collect();
            assert_eq!(
                texts,
                [
                    "Earlier question 1",
                    "Earlier answer 1",
                    "Earlier question 2",
                    "Earlier answer 2"
                ]
            );
            chat.submit(launch, "Continue earlier work".into(), Arc::new(|| {}))
                .unwrap();
            wait_chat(&mut chat);
            assert!(
                chat.messages
                    .iter()
                    .any(|m| m.role == Role::Assistant && m.text.contains("Continue earlier work"))
            );
            let calls = log(temp.path());
            let prompt = calls
                .iter()
                .rfind(|v| matches!(v["method"].as_str(), Some("turn/start" | "session/prompt")))
                .unwrap();
            assert_eq!(
                prompt["params"][if provider == Provider::Acp {
                    "sessionId"
                } else {
                    "threadId"
                }],
                "saved-1"
            );
            assert_eq!(
                calls.iter().filter(|v| v["method"] == "initialize").count(),
                1
            );
            assert!(
                calls
                    .iter()
                    .all(|v| v["method"] != "thread/start" && v["method"] != "session/new")
            );
            if provider == Provider::Codex {
                let resume = calls
                    .iter()
                    .find(|v| v["method"] == "thread/resume")
                    .unwrap();
                assert_eq!(resume["params"]["sandbox"], "danger-full-access");
                assert_eq!(resume["params"]["approvalPolicy"], "never");
                assert_eq!(resume["params"]["excludeTurns"], true);
                assert_eq!(
                    prompt["params"]["sandboxPolicy"]["type"],
                    if active {
                        "externalSandbox"
                    } else {
                        "dangerFullAccess"
                    }
                );
            }
        }
    }
}

#[test]
fn unsupported_history_and_cancelled_replay_preserve_the_current_chat() {
    for flag in ["--no-history", "--wait-history"] {
        let (temp, mut launch) = fixture(Provider::Acp, false);
        let mut chat = Chat::default();
        chat.submit(
            launch.clone(),
            "Current conversation".into(),
            Arc::new(|| {}),
        )
        .unwrap();
        wait_chat(&mut chat);
        let count = chat.messages.len();
        let current_launch = launch.clone();
        launch.config.acp_arguments.push(flag.into());
        let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
        wait(&mut browser);
        if flag == "--no-history" {
            assert!(browser.status.contains("does not support"));
            assert!(
                !log(temp.path())
                    .iter()
                    .any(|v| v["method"] == "session/list")
            );
        } else {
            browser.resume("saved-1").unwrap();
            assert!(browser.busy);
        }
        drop(browser);
        assert!(chat.connected());
        assert_eq!(chat.messages.len(), count);
        chat.submit(current_launch, "Still here".into(), Arc::new(|| {}))
            .unwrap();
        wait_chat(&mut chat);
        assert_eq!(chat.completed, 2);
    }
}

#[test]
fn changed_saved_project_is_rejected_before_resume_without_replacing_chat() {
    let (temp, launch) = fixture(Provider::Codex, false);
    let program = temp.path().join("agent.py");
    let source = fs::read_to_string(&program)
        .unwrap()
        .replace("if \"--wrong-history-root\" in sys.argv:", "if True:");
    fs::write(program, source).unwrap();
    let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
    wait(&mut browser);
    browser.resume("saved-1").unwrap();
    wait(&mut browser);
    assert!(
        browser.status.contains("does not belong"),
        "{}",
        browser.status
    );
    assert!(browser.take_restored().is_none());
    assert!(
        log(temp.path())
            .iter()
            .all(|v| v["method"] != "thread/resume")
    );
}

#[test]
fn local_history_does_not_require_a_model_and_read_only_wins_over_saved_access() {
    let (temp, mut launch) = fixture(Provider::Local, false);
    launch.config.access = Access::ReadOnly;
    assert!(
        launch.config.argv().is_err(),
        "Sending still requires a local model"
    );
    let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
    wait(&mut browser);
    browser.resume("saved-1").unwrap();
    wait(&mut browser);
    assert!(browser.take_restored().is_some(), "{}", browser.status);
    let calls = log(temp.path());
    let resume = calls
        .iter()
        .find(|v| v["method"] == "thread/resume")
        .unwrap();
    assert_eq!(resume["params"]["sandbox"], "read-only");
    assert_eq!(resume["params"]["approvalPolicy"], "never");
    assert_eq!(resume["params"]["modelProvider"], "rode_local");
}

#[test]
fn acp_replay_hides_fragmented_style_hints_and_default_resets_a_restored_chat() {
    let (temp, mut launch) = fixture(Provider::Acp, false);
    launch.config.personality = rode::agent::Personality::JacksonLamb;
    let mut chat = Chat::default();
    chat.submit(launch.clone(), "first".into(), Arc::new(|| {}))
        .unwrap();
    wait_chat(&mut chat);
    let calls = log(temp.path());
    let hint = calls
        .iter()
        .find(|r| r["method"] == "session/prompt")
        .unwrap()["params"]["prompt"][1]["text"]
        .as_str()
        .unwrap();
    fs::write(temp.path().join("saved-response-style.txt"), hint).unwrap();
    launch.config.personality = rode::agent::Personality::Default;
    let mut browser = Browser::open(launch.clone(), Arc::new(|| {})).unwrap();
    wait(&mut browser);
    browser.resume("saved-1").unwrap();
    wait(&mut browser);
    chat.restore(
        browser
            .take_restored()
            .unwrap_or_else(|| panic!("{}", browser.status)),
    );
    assert_eq!(
        chat.messages
            .iter()
            .filter(|m| m.role == Role::User)
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        ["Earlier question 1", "Earlier question 2"]
    );
    chat.submit(launch, "normal tone".into(), Arc::new(|| {}))
        .unwrap();
    wait_chat(&mut chat);
    let calls = log(temp.path());
    let prompt = calls
        .iter()
        .rev()
        .find(|r| r["method"] == "session/prompt")
        .unwrap();
    assert_eq!(prompt["params"]["sessionId"], "saved-1");
    assert!(
        prompt["params"]["prompt"][1]["text"]
            .as_str()
            .unwrap()
            .contains("name=\"default\"")
    );
}

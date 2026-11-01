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

fn options(root: &Path, value: Value) {
    fs::write(root.join("history-fixture.json"), value.to_string()).unwrap();
}

fn deleted(root: &Path) -> Vec<String> {
    fs::read(root.join("history-deleted.json"))
        .ok()
        .map(|bytes| serde_json::from_slice(&bytes).unwrap())
        .unwrap_or_default()
}

#[test]
fn individual_deletion_is_acknowledged_persisted_and_never_requests_inference() {
    for provider in [Provider::Codex, Provider::Acp] {
        let (temp, launch) = fixture(provider, false);
        let mut browser = Browser::open(launch.clone(), Arc::new(|| {})).unwrap();
        wait(&mut browser);
        assert!(browser.can_delete);
        assert!(browser.delete(Some("foreign"), vec![]).is_err());
        assert!(
            browser
                .delete(Some("saved-1"), vec!["saved-1".into()])
                .is_err()
        );
        browser.delete(Some("saved-1"), vec![]).unwrap();
        wait(&mut browser);
        assert!(browser.status.contains("Deleted 1"), "{}", browser.status);
        assert!(browser.entries.is_empty());
        assert_eq!(deleted(temp.path()), ["saved-1"]);
        let mut refreshed = Browser::open(launch, Arc::new(|| {})).unwrap();
        wait(&mut refreshed);
        assert!(refreshed.entries.is_empty());
        refreshed.load_more().unwrap();
        wait(&mut refreshed);
        assert_eq!(refreshed.entries[0].id, "saved-2");
        assert!(log(temp.path()).iter().all(|r| !matches!(
            r["method"].as_str(),
            Some(
                "thread/start"
                    | "thread/resume"
                    | "turn/start"
                    | "session/new"
                    | "session/load"
                    | "session/prompt"
            )
        )));
    }
}

#[test]
fn individual_codex_deletion_protects_active_descendants_on_unloaded_and_archived_pages() {
    for (count, active, archived) in [
        (3, "saved-1", false),
        (300, "saved-299", false),
        (3, "archived-1", true),
    ] {
        let (temp, launch) = fixture(Provider::Codex, false);
        options(
            temp.path(),
            serde_json::json!({
                "count":count, "archived":archived, "active":[active],
                "parents":{(active):"saved-0"}
            }),
        );
        let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
        wait(&mut browser);
        browser.delete(Some("saved-0"), vec![]).unwrap();
        wait(&mut browser);
        assert!(deleted(temp.path()).is_empty(), "{}", browser.status);
        assert!(
            browser.status.contains("active child"),
            "{}",
            browser.status
        );
        assert!(
            log(temp.path())
                .iter()
                .all(|r| r["method"] != "thread/delete")
        );
    }
}

#[test]
fn delete_all_enumerates_beyond_the_browser_limit_and_keeps_open_active_and_foreign_chats() {
    for provider in [Provider::Codex, Provider::Acp] {
        let (temp, launch) = fixture(provider, false);
        options(
            temp.path(),
            serde_json::json!({
                "count":300,"archived":true,"active":["saved-43"],"other_provider":["saved-44"]
            }),
        );
        let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
        wait(&mut browser);
        assert_eq!(browser.entries.len(), 32);
        browser.delete(None, vec!["saved-42".into()]).unwrap();
        wait(&mut browser);
        let mut expected: Vec<_> = (0..300)
            .filter(|n| *n != 42 && *n != 43 && (provider == Provider::Acp || *n != 44))
            .map(|n| format!("saved-{n}"))
            .collect();
        if provider == Provider::Codex {
            expected.push("archived-1".into());
        }
        expected.sort();
        assert_eq!(deleted(temp.path()), expected, "{}", browser.status);
        assert!(browser.status.contains("kept 2"), "{}", browser.status);
        let calls = log(temp.path());
        let first_delete = calls
            .iter()
            .position(|r| {
                matches!(
                    r["method"].as_str(),
                    Some("thread/delete" | "session/delete")
                )
            })
            .unwrap();
        assert!(
            calls
                .iter()
                .skip(first_delete)
                .filter(|r| r["method"] == "thread/list" || r["method"] == "session/list")
                .count()
                == 1,
            "Only the final refresh may list during/after deletion"
        );
        assert!(
            calls
                .iter()
                .all(|r| r["method"] != "turn/start" && r["method"] != "session/prompt")
        );
    }
}

#[test]
fn deletion_revalidates_saved_scope_and_partial_failures_leave_unacknowledged_rows() {
    for provider in [Provider::Codex, Provider::Acp] {
        let (temp, launch) = fixture(provider, false);
        let mut browser = Browser::open(launch.clone(), Arc::new(|| {})).unwrap();
        wait(&mut browser);
        options(temp.path(), serde_json::json!({"wrong_root":true}));
        browser.delete(Some("saved-1"), vec![]).unwrap();
        wait(&mut browser);
        assert!(deleted(temp.path()).is_empty());
        assert_eq!(browser.entries.len(), 1);
        options(temp.path(), serde_json::json!({"delete_error":"saved-2"}));
        let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
        wait(&mut browser);
        browser.load_more().unwrap();
        wait(&mut browser);
        browser.delete(None, vec![]).unwrap();
        wait(&mut browser);
        assert_eq!(deleted(temp.path()), ["saved-1"]);
        assert!(
            browser.status.contains("Deleted 1 chats before"),
            "{}",
            browser.status
        );
        assert!(
            browser.status.contains("Delete failed"),
            "{}",
            browser.status
        );
        assert_eq!(browser.entries.len(), 1);
        assert_eq!(browser.entries[0].id, "saved-2");
    }
}

#[test]
fn unsupported_deletion_keeps_history_resumable_and_invalid_pagination_never_deletes() {
    for provider in [Provider::Codex, Provider::Acp] {
        let (temp, launch) = fixture(provider, false);
        options(temp.path(), serde_json::json!({"delete_supported":false}));
        let mut browser = Browser::open(launch.clone(), Arc::new(|| {})).unwrap();
        wait(&mut browser);
        if provider == Provider::Acp {
            assert!(!browser.can_delete);
            assert!(browser.delete(Some("saved-1"), vec![]).is_err());
            assert!(
                !log(temp.path())
                    .iter()
                    .any(|r| r["method"] == "session/delete")
            );
        } else {
            browser.delete(Some("saved-1"), vec![]).unwrap();
            wait(&mut browser);
            assert!(!browser.can_delete);
            assert!(browser.status.contains("does not support deleting"));
        }
        assert!(browser.can_resume);
        assert_eq!(browser.entries.len(), 1);
        browser.resume("saved-1").unwrap();
        wait(&mut browser);
        assert!(browser.take_restored().is_some());
        options(temp.path(), serde_json::json!({"repeat_cursor":true}));
        let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
        wait(&mut browser);
        browser.delete(None, vec![]).unwrap();
        wait(&mut browser);
        assert!(browser.status.contains("Repeated delete discovery cursor"));
        assert!(deleted(temp.path()).is_empty());
    }
}

#[test]
fn acp_missing_session_delete_is_idempotent_success() {
    let (temp, launch) = fixture(Provider::Acp, false);
    options(temp.path(), serde_json::json!({"missing_delete":true}));
    let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
    wait(&mut browser);
    browser.delete(Some("saved-1"), vec![]).unwrap();
    wait(&mut browser);
    assert!(browser.status.contains("Deleted 1"));
    assert!(browser.can_delete);
    assert!(browser.entries.is_empty());
}

#[test]
fn a_failed_refresh_reports_the_deletion_that_already_committed() {
    for provider in [Provider::Codex, Provider::Acp] {
        let (temp, launch) = fixture(provider, false);
        let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
        wait(&mut browser);
        options(temp.path(), serde_json::json!({"refresh_error":true}));
        browser.delete(Some("saved-1"), vec![]).unwrap();
        wait(&mut browser);
        assert_eq!(deleted(temp.path()), ["saved-1"]);
        assert!(
            browser
                .status
                .contains("Deleted 1 saved chats. Refresh failed"),
            "{}",
            browser.status
        );
        assert!(
            browser.status.contains("List refresh failed"),
            "{}",
            browser.status
        );
    }
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
fn both_protocols_hide_saved_personality_instructions_in_previews_and_recalled_prompts() {
    for provider in [Provider::Codex, Provider::Acp] {
        for hint in [
            include_str!("../crates/rode-agent-core/src/personality/lamb.txt"),
            "<rode_response_style version=\"2\" name=\"jackson_lamb\">\nOlder personality instructions.\n</rode_response_style>",
        ] {
            let (temp, launch) = fixture(provider, false);
            fs::write(temp.path().join("saved-response-style.txt"), hint).unwrap();
            // The backend cuts the preview inside the style's opening tag.
            let preview = format!("Earlier work on this project\n\n{hint}");
            options(
                temp.path(),
                serde_json::json!({"previews": {"saved-1": &preview[..55]}}),
            );
            let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
            wait(&mut browser);
            assert_eq!(browser.entries[0].title, "Saved conversation saved-1");
            assert_eq!(browser.entries[0].preview, "Earlier work on this project");
            browser.resume("saved-1").unwrap();
            wait(&mut browser);
            let mut chat = Chat::default();
            chat.restore(browser.take_restored().unwrap());
            assert_eq!(
                chat.messages
                    .iter()
                    .filter(|m| m.role == Role::User)
                    .map(|m| m.text.as_str())
                    .collect::<Vec<_>>(),
                ["Earlier question 1", "Earlier question 2"]
            );
            // Restoring display text must not rewrite the agent-owned transcript.
            assert_eq!(
                fs::read_to_string(temp.path().join("saved-response-style.txt")).unwrap(),
                hint
            );
            assert!(!log(temp.path()).iter().any(|call| matches!(
                call["method"].as_str(),
                Some("turn/start" | "session/prompt")
            )));
        }
    }
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

#[test]
fn bulk_deletion_handles_cascaded_children_with_inflight_reads_and_duplicate_notifications() {
    for notifications_first in [false, true] {
        let (temp, launch) = fixture(Provider::Codex, false);
        options(
            temp.path(),
            serde_json::json!({
                "count":4, "cascade":true, "notifications_first":notifications_first,
                "duplicate_notifications":true,
                "parents":{"saved-1":"saved-0", "saved-2":"saved-1"}
            }),
        );
        let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
        wait(&mut browser);
        browser.delete(None, vec![]).unwrap();
        wait(&mut browser);
        assert_eq!(
            deleted(temp.path()),
            ["saved-0", "saved-1", "saved-2", "saved-3"]
        );
        assert!(
            browser.status.contains("Deleted 4 saved chats"),
            "{}",
            browser.status
        );
        let calls = log(temp.path());
        assert_eq!(
            calls
                .iter()
                .filter(|r| r["method"] == "thread/delete")
                .count(),
            2
        );
        if !notifications_first {
            // The parent's response advances the queue before its notifications.
            assert!(
                calls
                    .iter()
                    .any(|r| r["method"] == "thread/read" && r["params"]["threadId"] == "saved-1")
            );
        }
    }
}

#[test]
fn a_missing_chat_without_a_confirmed_removal_still_stops_bulk_deletion() {
    let (temp, launch) = fixture(Provider::Codex, false);
    options(
        temp.path(),
        serde_json::json!({
            "count":5, "cascade":true, "read_error":"saved-3",
            "parents":{"saved-1":"saved-0", "saved-2":"saved-1"}
        }),
    );
    let mut browser = Browser::open(launch, Arc::new(|| {})).unwrap();
    wait(&mut browser);
    browser.delete(None, vec![]).unwrap();
    wait(&mut browser);
    assert_eq!(deleted(temp.path()), ["saved-0", "saved-1", "saved-2"]);
    assert!(
        browser.status.contains("Thread not found"),
        "{}",
        browser.status
    );
    assert!(
        browser.status.contains("Deleted 3 chats before"),
        "{}",
        browser.status
    );
    assert!(browser.entries.iter().any(|row| row.id == "saved-4"));
}

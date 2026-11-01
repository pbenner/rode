//! Codex/ACP protocol, cancellation, sandbox scope and owned subprocess cleanup using
//! deterministic peers.

use rode::{
    agent::{
        Access, Approval, Config, Launch, Personality, Provider,
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
fn personalities_change_only_reply_hints_without_restarting_or_changing_permissions() {
    for provider in [Provider::Codex, Provider::Local, Provider::Acp] {
        for isolated in [false, true] {
            let (_temp, root, program) = fixture();
            let mut launch = launch(&root, &program, provider, isolated);
            launch.config.local_model = "fixture-local".into();
            let argv = launch.config.argv().unwrap();
            let original = launch.config.clone();
            let mut chat = Chat::default();
            submit(&mut chat, launch.clone(), "first");
            wait(&mut chat, |c| c.completed == 1);
            let calls = log(&root);
            let first = calls
                .iter()
                .find(|v| matches!(v["method"].as_str(), Some("turn/start" | "session/prompt")))
                .unwrap();
            let field = if provider == Provider::Acp {
                "prompt"
            } else {
                "input"
            };
            assert_eq!(
                first["params"][field].as_array().unwrap().len(),
                1,
                "Fresh Default is unmodified"
            );

            launch.config.personality = Personality::JacksonLamb;
            assert!(original.same_process(&launch.config));
            assert_eq!(launch.config.argv().unwrap(), argv);
            submit(&mut chat, launch.clone(), "approve");
            wait(&mut chat, |c| !c.prompts.is_empty());
            assert!(!root.join("approved.txt").exists());
            chat.answer(chat.prompts[0].choices[1].result.clone())
                .unwrap();
            wait(&mut chat, |c| c.completed == 2);
            assert!(!root.join("approved.txt").exists());
            let calls = log(&root);
            let styled = calls
                .iter()
                .rev()
                .find(|v| v["method"] == first["method"])
                .unwrap();
            assert_eq!(styled["params"][field][0]["text"], "approve");
            let hint = styled["params"][field][1]["text"].as_str().unwrap();
            assert!(hint.contains("jackson_lamb"));
            assert!(!hint.contains("version="));
            assert!(hint.contains("not assumed knowledge"));
            assert!(hint.contains("final answer"));
            assert!(hint.contains("presentation guidance only"));
            for key in [
                "model",
                "effort",
                "approvalPolicy",
                "sandboxPolicy",
                "threadId",
                "sessionId",
            ] {
                assert_eq!(
                    styled["params"][key], first["params"][key],
                    "{provider:?}: {key}"
                );
            }
            assert!(
                styled["params"].get("personality").is_none(),
                "Do not alter a vendor operating-mode selector"
            );
            for (index, message) in ["continue", "one more follow-up"].iter().enumerate() {
                submit(&mut chat, launch.clone(), message);
                wait(&mut chat, |c| c.completed == 3 + index as u64);
                let calls = log(&root);
                let follow_up = calls
                    .iter()
                    .rev()
                    .find(|v| v["method"] == first["method"])
                    .unwrap();
                assert_eq!(follow_up["params"][field][0]["text"], *message);
                assert_eq!(follow_up["params"][field].as_array().unwrap().len(), 2);
                assert_eq!(follow_up["params"][field][1]["text"], hint);
                for key in [
                    "model",
                    "effort",
                    "approvalPolicy",
                    "sandboxPolicy",
                    "threadId",
                    "sessionId",
                ] {
                    assert_eq!(
                        follow_up["params"][key], styled["params"][key],
                        "{provider:?}: {key}"
                    );
                }
            }
            launch.config.personality = Personality::Default;
            submit(&mut chat, launch.clone(), "normal again");
            wait(&mut chat, |c| c.completed == 5);
            let calls = log(&root);
            let reset = calls
                .iter()
                .rev()
                .find(|v| v["method"] == first["method"])
                .unwrap();
            assert!(
                reset["params"][field][1]["text"]
                    .as_str()
                    .unwrap()
                    .contains("name=\"default\"")
            );
            assert_eq!(
                calls.iter().filter(|v| v["method"] == "initialize").count(),
                1
            );
            assert_eq!(
                chat.messages
                    .iter()
                    .filter(|m| m.role == Role::User)
                    .map(|m| m.text.as_str())
                    .collect::<Vec<_>>(),
                [
                    "first",
                    "approve",
                    "continue",
                    "one more follow-up",
                    "normal again"
                ]
            );
            chat.disconnect();
            submit(&mut chat, launch, "fresh");
            wait(&mut chat, |c| c.completed == 6);
            let calls = log(&root);
            let fresh = calls
                .iter()
                .rev()
                .find(|v| v["method"] == first["method"])
                .unwrap();
            assert_eq!(fresh["params"][field].as_array().unwrap().len(), 1);
        }
    }
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
        assert!(chat.last_turn_succeeded);
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
fn codex_model_reasoning_and_access_changes_apply_to_the_next_turn() {
    let (_temp, root, program) = fixture();
    let mut launch = launch(&root, &program, Provider::Codex, false);
    launch.config.model = "fixture-fast".into();
    launch.config.reasoning_effort = "low".into();
    let mut chat = Chat::default();
    submit(&mut chat, launch.clone(), "first");
    wait(&mut chat, |c| c.completed == 1 && c.models.len() == 2);
    assert!(chat.models.iter().all(|m| m.value.id != "hidden"));
    launch.config.model = "fixture-deep".into();
    launch.config.reasoning_effort = "high".into();
    launch.config.access = Access::Full;
    launch.config.approval = Approval::Never;
    submit(&mut chat, launch, "second");
    wait(&mut chat, |c| c.completed == 2);
    let log = log(&root);
    assert_eq!(
        log.iter().filter(|r| r["method"] == "initialize").count(),
        1
    );
    let request = log.iter().rfind(|r| r["method"] == "turn/start").unwrap();
    assert_eq!(request["params"]["model"], "fixture-deep");
    assert_eq!(request["params"]["effort"], "high");
    assert_eq!(request["params"]["approvalPolicy"], "never");
    assert_eq!(
        request["params"]["sandboxPolicy"]["type"],
        "dangerFullAccess"
    );
}

#[test]
fn never_ask_declines_requests_for_both_protocols_without_ending_the_conversation() {
    for provider in [Provider::Codex, Provider::Acp] {
        let (_temp, root, program) = fixture();
        let mut launch = launch(&root, &program, provider, false);
        launch.config.approval = Approval::Never;
        let mut chat = Chat::default();
        submit(&mut chat, launch.clone(), "approve");
        wait(&mut chat, |c| c.completed == 1);
        assert!(chat.prompts.is_empty());
        assert!(!root.join("approved.txt").exists());
        assert!(chat.connected());
        submit(&mut chat, launch, "next");
        wait(&mut chat, |c| c.completed == 2);
    }
}

#[test]
fn project_read_only_overrides_full_access_and_cannot_be_approved_away() {
    let (_temp, root, program) = fixture();
    let mut launch = launch(&root, &program, Provider::Codex, false);
    launch.plan.project_read_only = true;
    launch.config.access = Access::Full;
    let mut chat = Chat::default();
    submit(&mut chat, launch, "approve");
    wait(&mut chat, |c| c.completed == 1);
    assert!(!root.join("approved.txt").exists());
    let log = log(&root);
    assert_eq!(
        log.iter().find(|r| r["method"] == "thread/start").unwrap()["params"]["sandbox"],
        "read-only"
    );
    let turn = log.iter().find(|r| r["method"] == "turn/start").unwrap();
    assert_eq!(turn["params"]["sandboxPolicy"]["type"], "readOnly");
    assert_eq!(turn["params"]["approvalPolicy"], "never");
}

#[test]
fn acp_controls_are_acknowledged_in_dependency_order_before_the_next_prompt() {
    let (_temp, root, program) = fixture();
    let mut launch = launch(&root, &program, Provider::Acp, false);
    let mut chat = Chat::default();
    submit(&mut chat, launch.clone(), "first");
    wait(&mut chat, |c| c.completed == 1);
    assert_eq!(chat.controls.len(), 4);
    launch.config.acp_options = std::collections::BTreeMap::from([
        ("model".into(), json!("fixture-deep")),
        ("mode".into(), json!("code")),
        ("effort".into(), json!("high")),
        ("automatic".into(), json!(true)),
    ]);
    submit(&mut chat, launch, "second");
    wait(&mut chat, |c| c.completed == 2);
    let log = log(&root);
    assert_eq!(
        log.iter().filter(|r| r["method"] == "initialize").count(),
        1
    );
    let configured: Vec<_> = log
        .iter()
        .filter(|r| r["method"] == "session/set_config_option")
        .collect();
    assert_eq!(configured.len(), 4);
    assert_eq!(configured[0]["params"]["configId"], "model");
    assert_eq!(configured[3]["params"]["type"], "boolean");
    assert!(
        chat.controls
            .iter()
            .any(|c| c.id == "effort" && c.current == "high")
    );
    let prompt = log
        .iter()
        .rposition(|r| r["method"] == "session/prompt")
        .unwrap();
    let last_set = log
        .iter()
        .rposition(|r| r["method"] == "session/set_config_option")
        .unwrap();
    assert!(prompt > last_set);
}

#[test]
fn acp_legacy_modes_work_and_unacknowledged_options_never_send_a_prompt() {
    for legacy in [true, false] {
        let (_temp, root, program) = fixture();
        let mut launch = launch(&root, &program, Provider::Acp, false);
        launch.config.acp_arguments = vec![
            if legacy {
                "--legacy-controls"
            } else {
                "--ignore-controls"
            }
            .into(),
        ];
        let mut chat = Chat::default();
        submit(&mut chat, launch.clone(), "first");
        wait(&mut chat, |c| c.completed == 1);
        launch.config.acp_options.insert(
            if legacy { "rode:legacy-mode" } else { "mode" }.into(),
            json!("code"),
        );
        submit(&mut chat, launch, "second");
        wait(&mut chat, |c| !c.busy);
        let log = log(&root);
        assert_eq!(
            log.iter()
                .filter(|r| r["method"] == "session/prompt")
                .count(),
            if legacy { 2 } else { 1 }
        );
        if legacy {
            assert!(log.iter().any(|r| r["method"] == "session/set_mode"));
        } else {
            assert!(!chat.connected());
            assert!(
                chat.messages
                    .iter()
                    .any(|m| m.text.contains("did not acknowledge"))
            );
        }
    }
}

#[test]
fn legacy_read_only_preferences_remain_effective() {
    let config: Config = serde_json::from_value(json!({"read_only":true})).unwrap();
    assert_eq!(config.access(), Access::ReadOnly);
    assert_eq!(config.approval, Approval::Ask);
}

#[test]
fn cancelling_pending_acp_settings_never_submits_the_queued_prompt() {
    let (_temp, root, program) = fixture();
    let mut launch = launch(&root, &program, Provider::Acp, false);
    launch.config.acp_arguments = vec!["--wait-controls".into()];
    launch
        .config
        .acp_options
        .insert("mode".into(), json!("code"));
    let mut chat = Chat::default();
    submit(&mut chat, launch, "must not run");
    wait(&mut chat, |c| c.status == "Applying agent controls…");
    assert!(chat.has_deadline());
    chat.cancel();
    assert!(!chat.busy && !chat.connected());
    assert!(!log(&root).iter().any(|r| r["method"] == "session/prompt"));
}

#[test]
fn idle_acp_model_changes_refresh_capabilities_without_a_prompt_or_completed_turn() {
    let (_temp, root, program) = fixture();
    let launch = launch(&root, &program, Provider::Acp, false);
    let mut chat = Chat::default();
    submit(&mut chat, launch.clone(), "first");
    wait(&mut chat, |c| c.completed == 1);
    let mut next = launch.config;
    next.acp_options
        .insert("model".into(), json!("fixture-deep"));
    chat.configure_idle(&next).unwrap();
    wait(&mut chat, |c| !c.busy);
    assert_eq!(chat.completed, 1);
    let effort = chat.controls.iter().find(|c| c.id == "effort").unwrap();
    assert!(effort.accepts(&json!("high")));
    assert_eq!(
        log(&root)
            .iter()
            .filter(|r| r["method"] == "session/prompt")
            .count(),
        1
    );
}

#[test]
fn unavailable_saved_acp_options_do_not_leave_an_idle_session_busy() {
    for idle_change in [false, true] {
        let (_temp, root, program) = fixture();
        let mut launch = launch(&root, &program, Provider::Acp, false);
        let mut chat = Chat::default();
        submit(&mut chat, launch.clone(), "first");
        wait(&mut chat, |c| c.completed == 1);
        launch
            .config
            .acp_options
            .insert("unavailable-option".into(), json!("value"));
        let result = if idle_change {
            chat.configure_idle(&launch.config)
        } else {
            chat.submit(launch, "must not run".into(), Arc::new(|| {}))
        };
        assert!(result.is_err());
        assert!(!chat.busy && !chat.connected());
        assert_eq!(
            log(&root)
                .iter()
                .filter(|r| r["method"] == "session/prompt")
                .count(),
            1
        );
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
    assert!(!chat.last_turn_succeeded);
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
    let mut launch = launch(&root, &program, Provider::Codex, true);
    launch.config.access = Access::Full;
    launch.config.approval = Approval::Never;
    launch.plan = sandbox::prepare(
        root.to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            share_network: false,
            ..Default::default()
        },
    )
    .unwrap();
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
        assert_eq!(
            turn["params"]["sandboxPolicy"]["networkAccess"],
            "restricted"
        );
    } else {
        assert_eq!(turn["params"]["sandboxPolicy"]["type"], "dangerFullAccess");
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
        assert!(!chat.last_turn_succeeded);
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

#[test]
fn file_contents_reach_codex_local_and_capable_acp_agents_without_payloads_in_transcripts() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rode_agent_core::attachment::Attachment;
    let image = image::RgbImage::from_fn(360, 360, |x, y| {
        let n = (x + y * 360).wrapping_mul(2654435761);
        image::Rgb([n as u8, (n >> 8) as u8, (n >> 16) as u8])
    });
    let mut bytes = std::io::Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let data = STANDARD.encode(bytes.into_inner());
    assert!(
        data.len() > 256 * 1024,
        "Exercise the larger image-bearing transport"
    );
    for provider in [Provider::Codex, Provider::Local, Provider::Acp] {
        let (_temp, root, program) = fixture();
        let mut launch = launch(&root, &program, provider, false);
        launch.config.local_model = "fixture-local".into();
        launch.config.acp_arguments = vec!["--image-input".into()];
        let mut chat = Chat::default();
        let files = vec![
            Attachment::Image {
                name: "view.png".into(),
                mime_type: "image/png".into(),
                data: data.clone(),
            },
            Attachment::Document {
                name: "notes.md".into(),
                text: "Document body λ".into(),
            },
        ];
        chat.submit_with_attachments(launch, "Compare these".into(), files, Arc::new(|| {}))
            .unwrap();
        wait(&mut chat, |c| c.completed == 1 && !c.busy);
        let calls = log(&root);
        let turn = calls
            .iter()
            .find(|v| matches!(v["method"].as_str(), Some("turn/start" | "session/prompt")))
            .unwrap();
        let blocks = if provider == Provider::Acp {
            &turn["params"]["prompt"]
        } else {
            &turn["params"]["input"]
        };
        assert_eq!(blocks[0]["text"], "Compare these");
        let image = blocks
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["type"] == "image")
            .unwrap();
        if provider == Provider::Acp {
            assert_eq!(image["data"], data);
            assert_eq!(image["mimeType"], "image/png");
        } else {
            assert_eq!(image["url"], format!("data:image/png;base64,{data}"));
        }
        assert!(blocks.as_array().unwrap().iter().any(|b| {
            b["text"]
                .as_str()
                .is_some_and(|s| s.contains("Document body λ"))
        }));
        let user = chat.messages.iter().find(|m| m.role == Role::User).unwrap();
        assert!(user.text.contains("view.png, notes.md"));
        assert!(!user.text.contains(&data[..100]));
        assert!(!user.text.contains("Document body"));
    }
}

#[test]
fn acp_agents_without_image_capability_never_receive_image_blocks() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rode_agent_core::attachment::Attachment;
    let (_temp, root, program) = fixture();
    let mut chat = Chat::default();
    let file = Attachment::Image {
        name: "image.png".into(),
        mime_type: "image/png".into(),
        data: STANDARD.encode(b"\x89PNG\r\n\x1a\n"),
    };
    chat.submit_with_attachments(
        launch(&root, &program, Provider::Acp, false),
        "inspect".into(),
        vec![file],
        Arc::new(|| {}),
    )
    .unwrap();
    wait(&mut chat, |c| !c.busy);
    assert!(
        chat.messages
            .iter()
            .any(|m| m.text.contains("does not advertise image input"))
    );
    assert!(!log(&root).iter().any(|v| v["method"] == "session/prompt"));
}

#[test]
fn ssh_stdio_supervisor_forwards_attachment_sized_messages_without_an_ssh_server() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
        sync::mpsc,
    };
    let root = tempfile::tempdir().unwrap();
    let mut child = Command::new("python3")
        .args(["-c", include_str!("../src/remote_agent.py")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let launch = json!({"root":root.path(),"prefix":[],"argv":["python3","-c","import sys; print(len(sys.stdin.buffer.readline()), flush=True)"],"python":"off"});
    writeln!(input, "{launch}").unwrap();
    let payload = serde_json::to_vec(&json!({"image":"a".repeat(400_000)})).unwrap();
    input.write_all(&payload).unwrap();
    input.write_all(b"\n").unwrap();
    input.flush().unwrap();
    let output = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(output).read_line(&mut line);
        let _ = tx.send(line);
    });
    let result = rx.recv_timeout(Duration::from_secs(10));
    if result.is_err() {
        let _ = child.kill();
    }
    let line = result.expect("Remote supervisor did not forward the attachment payload");
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("Remote supervisor did not exit");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
    assert_eq!(line.trim().parse::<usize>().unwrap(), payload.len() + 1);
}

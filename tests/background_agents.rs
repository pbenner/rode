//! Real native controllers and disposable stdio bridges. Fixtures never use a
//! model, account or inference endpoint, and own isolated runtime directories.
use rode_agent_core::{
    Config, Plan, Provider,
    history::Owner,
    service::{Command as Action, Info, Spec, State},
};
use rode_session::{Request, Response};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

fn wait(check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !check() {
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for native agent work"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}
struct Fixture {
    base: tempfile::TempDir,
    runtime: std::path::PathBuf,
    root: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("project");
        fs::create_dir(&root).unwrap();
        let path = root.join("agent.py");
        fs::write(&path, include_str!("fixtures/persistent_agent.py")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = base.path().join("runtime");
        Self {
            base,
            runtime,
            root,
        }
    }
    fn rpc(&self, request: &Request) -> anyhow::Result<Response> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rode-sessiond"))
            .arg("request")
            .env("RODE_SESSION_DIR", &self.runtime)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(request)?)?;
        let output = child.wait_with_output()?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Response>(&output.stdout)?.checked()
    }
    fn infos(&self) -> Vec<Info> {
        let Response::Agents(infos) = self.rpc(&Request::Agents).unwrap() else {
            panic!()
        };
        infos
    }
    fn create(&self, provider: Provider, persistent: bool, prompt: &str) -> Info {
        self.create_with_attachments(provider, persistent, prompt, vec![])
    }
    fn create_with_attachments(
        &self,
        provider: Provider,
        persistent: bool,
        prompt: &str,
        attachments: Vec<rode_agent_core::attachment::Attachment>,
    ) -> Info {
        self.create_agent(provider, persistent, Some(prompt), attachments)
    }
    fn create_agent(
        &self,
        provider: Provider,
        persistent: bool,
        prompt: Option<&str>,
        attachments: Vec<rode_agent_core::attachment::Attachment>,
    ) -> Info {
        let executable = self.root.join("agent.py").to_string_lossy().into_owned();
        let config = Config {
            provider,
            codex: executable.clone(),
            acp_program: executable.clone(),
            claude_program: executable.clone(),
            claude_arguments: vec!["--acp".into()],
            acp_arguments: vec!["--acp".into()],
            local_model: "fixture-only".into(),
            ..Default::default()
        };
        let spec = Spec {
            id: rode_session::new_id().unwrap(),
            name: "Background fixture".into(),
            policy: "a".repeat(64),
            mode: "NORMAL MODE".into(),
            config,
            plan: Plan {
                root: self.root.to_string_lossy().into(),
                ..Default::default()
            },
            executable,
            file_tool_plan: None,
            env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
            python: "off".into(),
            persistent,
            text: prompt.map(str::to_owned),
            attachments,
        };
        let Response::Agent(info) = self
            .rpc(&Request::CreateAgent {
                spec: Box::new(spec),
            })
            .unwrap()
        else {
            panic!()
        };
        info
    }
    fn attach(&self, info: &Info) -> View {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rode-sessiond"))
            .args(["attach-agent", &info.id, &info.policy])
            .env("RODE_SESSION_DIR", &self.runtime)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            let mut previous = None;
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let state = serde_json::from_str::<rode_agent_core::service::Update>(&line)
                    .unwrap()
                    .apply(previous.as_ref())
                    .unwrap();
                previous = Some(state.clone());
                if tx.send(state).is_err() {
                    break;
                }
            }
        });
        View {
            child,
            input,
            events,
            last: std::cell::RefCell::new(None),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for info in self.infos() {
            let _ = self.rpc(&Request::EndAgent {
                id: info.id,
                policy: info.policy,
            });
        }
        let _ = &self.base;
    }
}
struct View {
    child: Child,
    input: ChildStdin,
    events: mpsc::Receiver<State>,
    last: std::cell::RefCell<Option<State>>,
}

#[test]
fn an_idle_agent_exit_pauses_existing_and_future_queued_work() {
    use rode_agent_core::queue::Action as Queue;
    for provider in [Provider::Codex, Provider::Claude, Provider::Acp] {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("continue"), "").unwrap();
        let info = fixture.create(provider, true, "completed work");
        let mut view = fixture.attach(&info);
        view.until(|s| s.chat.completed == 1 && !s.chat.busy);
        let pid: i32 = fs::read_to_string(fixture.root.join("agent.pid"))
            .unwrap()
            .parse()
            .unwrap();
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let failed = view.until(|s| s.chat.recoverable && !s.chat.stopping);
        assert_eq!(
            failed.chat.completed, 1,
            "Idle failure completes no additional turn"
        );
        assert!(failed.queue.paused);
        let held = view.queue(
            "queue-after-idle-exit",
            Queue::Push {
                id: 1,
                text: "keep this prompt".into(),
                attachments: vec![],
            },
        );
        assert!(held.queue.paused);
        assert_eq!(held.queue.items.len(), 1);
        view.queue(
            "recover-idle-exit",
            Queue::Recover {
                fresh: false,
                config: Box::new(failed.chat.config),
            },
        );
        let recovered = view.until(|s| s.chat.connected && !s.chat.busy && !s.chat.recoverable);
        assert_eq!(recovered.chat.completed, 1);
        assert!(recovered.queue.paused);
        assert_eq!(recovered.queue.items.len(), 1);
        view.send(Action::End);
        view.until(|s| !s.info.alive);
    }
}

#[test]
fn diagnostic_lines_up_to_one_mib_keep_the_native_agent_connected() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Acp] {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("continue"), "").unwrap();
        let info = fixture.create(provider, true, "long-stderr");
        let view = fixture.attach(&info);
        let state = view.until(|s| s.chat.completed == 1 && !s.chat.busy);
        assert!(state.chat.last_turn_succeeded, "{:?}", state.chat.messages);
        assert!(state.chat.connected);
        assert!(!state.chat.recoverable);
    }
}

#[test]
fn overflow_recovery_keeps_queued_attachments_and_never_repeats_the_failed_prompt() {
    use rode_agent_core::{attachment::Attachment, queue::Action as Queue};
    for provider in [Provider::Codex, Provider::Claude, Provider::Acp] {
        for stream in ["stdout", "stderr"] {
            let fixture = Fixture::new();
            fs::write(fixture.root.join("record-inputs"), "").unwrap();
            let prompt = format!("overflow-{stream}");
            let info = fixture.create(provider, true, &prompt);

            let mut view = fixture.attach(&info);
            view.until(|s| s.chat.connected && s.chat.busy);
            view.queue(
                "keep-follow-up",
                Queue::Push {
                    id: 1,
                    text: "queued follow-up".into(),
                    attachments: vec![Attachment::Document {
                        name: "notes.txt".into(),
                        text: "preserved snapshot".into(),
                    }],
                },
            );
            fs::write(fixture.root.join("continue"), "").unwrap();
            let failed = view.until(|s| s.chat.recoverable && !s.chat.busy && !s.chat.stopping);
            assert!(
                failed.info.alive,
                "The recoverable controller must retain its queue"
            );
            assert!(!failed.chat.connected);
            assert!(failed.queue.paused);
            assert_eq!(failed.queue.items.len(), 1);
            let limit = if stream == "stdout" {
                rode_agent_core::LINE_LIMIT
            } else {
                rode_agent_core::STDERR_LINE_LIMIT
            };
            assert!(
                failed
                    .chat
                    .messages
                    .iter()
                    .any(|message| message.text.contains(stream)
                        && message.text.contains(&format!("{limit} bytes"))),
                "{:?}",
                failed.chat.messages
            );
            let rejected = view.queue("cannot-resume-dead-agent", Queue::Pause { paused: false });
            assert!(rejected.error.contains("Recover"));
            assert!(rejected.queue.paused);
            let fresh = stream == "stderr";
            view.queue(
                "recover",
                Queue::Recover {
                    fresh,
                    config: Box::new(failed.chat.config.clone()),
                },
            );
            let ready = view.until(|s| s.chat.connected && !s.chat.busy && !s.chat.recoverable);
            assert_eq!(
                ready.chat.completed, 1,
                "Recovery must not submit inference"
            );
            assert!(ready.queue.paused);
            assert!(
                !ready
                    .chat
                    .messages
                    .iter()
                    .any(|m| m.text.contains("saved replay"))
            );
            assert_eq!(ready.queue.items[0].files, vec!["notes.txt"]);
            let rpc: Vec<serde_json::Value> =
                fs::read_to_string(fixture.root.join("backend-rpc.jsonl"))
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            if !fresh {
                let method = if provider.is_acp() {
                    "session/load"
                } else {
                    "thread/resume"
                };
                let resume = rpc
                    .iter()
                    .find(|m| m["method"] == method)
                    .expect("Saved conversation was not resumed");
                if provider == Provider::Codex {
                    assert_eq!(resume["params"]["excludeTurns"], true);
                }
            }
            let inputs = fixture.root.join("recorded-inputs.jsonl");
            assert_eq!(fs::read_to_string(&inputs).unwrap().lines().count(), 1);
            view.queue("resume-kept-work", Queue::Pause { paused: false });
            let done =
                view.until(|s| s.chat.completed == 2 && !s.chat.busy && s.queue.items.is_empty());
            assert!(done.chat.last_turn_succeeded);
            let inputs = fs::read_to_string(inputs).unwrap();
            assert_eq!(inputs.lines().count(), 2);
            assert_eq!(inputs.matches(&prompt).count(), 1);
            assert!(inputs.contains("preserved snapshot"));
        }
    }
}

#[test]
fn failed_saved_reconnect_can_fall_back_to_a_new_context_without_losing_work() {
    use rode_agent_core::queue::Action as Queue;
    for provider in [Provider::Codex, Provider::Claude, Provider::Acp] {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("no-resume"), "").unwrap();
        let info = fixture.create(provider, true, "overflow-stdout");
        let mut view = fixture.attach(&info);
        let working = view.until(|s| s.chat.connected && s.chat.busy);
        view.queue(
            "keep",
            Queue::Push {
                id: 1,
                text: "kept work".into(),
                attachments: vec![],
            },
        );
        fs::write(fixture.root.join("continue"), "").unwrap();
        view.until(|s| s.chat.recoverable && !s.chat.stopping && !s.chat.busy);
        view.queue(
            "try-saved",
            Queue::Recover {
                fresh: false,
                config: Box::new(working.chat.config.clone()),
            },
        );
        let failed = view.until(|s| {
            s.chat.recoverable && !s.chat.stopping && !s.chat.busy && s.chat.completed == 2
        });
        assert!(failed.queue.paused);
        assert_eq!(failed.queue.items[0].text, "kept work");
        assert_eq!(
            failed.chat.conversation, "native-thread",
            "A mismatched reconnect must not replace the protected conversation"
        );
        view.queue(
            "fresh-fallback",
            Queue::Recover {
                fresh: true,
                config: Box::new(working.chat.config),
            },
        );
        let ready = view.until(|s| s.chat.connected && !s.chat.busy && !s.chat.recoverable);
        assert_eq!(ready.queue.items[0].text, "kept work");
        assert!(ready.queue.paused);
    }
}

#[test]
fn native_file_only_controller_edits_through_its_captured_host_policy() {
    native_tool_controller(false);
}

#[test]
fn model_only_controller_retains_offline_commands_after_reattach_and_recovery() {
    native_tool_controller(true);
}

fn native_tool_controller(model_only: bool) {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("agent.py"),
        include_str!("fixtures/file_only_agent.py"),
    )
    .unwrap();
    fs::write(fixture.root.join("file-tool-steps.json"), serde_json::json!([
        {"params":{"tool":"rode_write_file","arguments":{"path":"native-source.py","text":"native controller\n","revision":null}}},
        {"params":{"tool":"exec_command","arguments":{"cmd":"touch forbidden"}}},
        {"params":{"tool":"rode_run_command","arguments":{"command":"/usr/bin/python3 -c 'import socket; socket.socket()' > offline-network 2>&1; result=$?; test \"$result\" -ne 0 && touch offline-command"}}}
    ]).to_string()).unwrap();
    let mut plan = rode::sandbox::Plan {
        root: fixture.root.to_string_lossy().into(),
        native_helper: Some(env!("CARGO_BIN_EXE_rode-sessiond").into()),
        ..Default::default()
    };
    if model_only {
        plan = rode::sandbox::prepare_with_helper(
            &plan.root,
            &rode::sandbox::Options {
                enabled: true,
                backend: rode::sandbox::Backend::Landlock,
                share_network: false,
                ..Default::default()
            },
            std::path::Path::new(env!("CARGO_BIN_EXE_rode-sessiond")),
        )
        .unwrap();
        if !plan.active {
            assert_ne!(
                std::env::var("RODE_TEST_REQUIRE_LANDLOCK").as_deref(),
                Ok("1"),
                "{}",
                plan.reason
            );
            return;
        }
    }
    let access = if model_only {
        rode_agent_core::Access::Project
    } else {
        rode_agent_core::Access::FilesOnly
    };
    let spec = Spec {
        id: rode_session::new_id().unwrap(),
        name: "File-only native fixture".into(),
        policy: "a".repeat(64),
        mode: "NORMAL MODE".into(),
        config: Config {
            codex: fixture.root.join("agent.py").to_string_lossy().into(),
            access,
            network: if model_only {
                rode_agent_core::network::Network::ModelOnly
            } else {
                Default::default()
            },
            ..Default::default()
        },
        plan: Plan {
            root: plan.root.clone(),
            active: plan.active,
            project_read_only: plan.project_read_only,
            prefix: plan.prefix.clone(),
        },
        file_tool_plan: Some(serde_json::to_value(&plan).unwrap()),
        executable: fixture.root.join("agent.py").to_string_lossy().into(),
        env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        python: "off".into(),
        persistent: true,
        text: Some("edit files".into()),
        attachments: vec![],
    };
    let Response::Agent(info) = fixture
        .rpc(&Request::CreateAgent {
            spec: Box::new(spec),
        })
        .unwrap()
    else {
        panic!()
    };
    let view = fixture.attach(&info);
    let state = view.until(|state| state.chat.completed == 1);
    assert!(state.chat.last_turn_succeeded, "{:?}", state.chat.messages);
    assert_eq!(
        fs::read_to_string(fixture.root.join("native-source.py")).unwrap(),
        "native controller\n"
    );
    assert!(!fixture.root.join("forbidden").exists());
    assert_eq!(fixture.root.join("offline-command").exists(), model_only);
    if model_only {
        assert!(
            fs::read_to_string(fixture.root.join("offline-network"))
                .unwrap()
                .contains("PermissionError")
        );
    }
    drop(view);
    wait(|| {
        fixture
            .infos()
            .iter()
            .any(|i| i.id == info.id && !i.attached)
    });
    let mut view = fixture.attach(&info);
    let state = view.until(|state| state.chat.connected);
    assert_eq!(state.chat.config.access, access);
    // Recovery must rebuild the verified file-only tools, never resume a thread
    // with command tools enabled or repeat the completed edit automatically.
    let pid: i32 = fs::read_to_string(fixture.root.join("file-agent.pid"))
        .unwrap()
        .parse()
        .unwrap();
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    let failed = view.until(|s| s.chat.recoverable && !s.chat.stopping && !s.chat.busy);
    let denied = view.queue(
        "file-only-cannot-resume",
        rode_agent_core::queue::Action::Recover {
            fresh: false,
            config: Box::new(failed.chat.config.clone()),
        },
    );
    assert!(denied.error.contains("Native-tool agents require"));
    view.queue(
        "restart-confined-tools",
        rode_agent_core::queue::Action::Recover {
            fresh: true,
            config: Box::new(failed.chat.config),
        },
    );
    let ready = view.until(|s| s.chat.connected && !s.chat.busy && !s.chat.recoverable);
    assert_eq!(ready.chat.config.access, access);
    assert!(!fixture.root.join("forbidden").exists());
    let calls: Vec<serde_json::Value> =
        fs::read_to_string(fixture.root.join("file-agent-rpc.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c["method"] == "thread/start")
            .count(),
        2
    );
    assert_eq!(
        calls.iter().filter(|c| c["method"] == "turn/start").count(),
        1
    );
}
impl View {
    fn send(&mut self, action: Action) {
        serde_json::to_writer(&mut self.input, &action).unwrap();
        self.input.write_all(b"\n").unwrap();
        self.input.flush().unwrap();
    }
    fn queue(&mut self, request: &str, operation: rode_agent_core::queue::Action) -> State {
        self.send(Action::Queue {
            request: request.into(),
            operation,
        });
        self.until(|s| s.queue.acknowledged == request)
    }
    fn until(&self, check: impl Fn(&State) -> bool) -> State {
        if let Some(state) = self.last.borrow().as_ref().filter(|state| check(state)) {
            return state.clone();
        }
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let state = self
                .events
                .recv_timeout(Duration::from_secs(12))
                .expect("No agent state");
            *self.last.borrow_mut() = Some(state.clone());
            if check(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "Agent state did not settle: {}",
                state.info.status
            );
        }
    }
}
impl Drop for View {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn restored_prompt_recall_survives_native_deltas_and_reattachment() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Acp] {
        let fixture = Fixture::new();
        fs::write(
            fixture.root.join("agent.py"),
            include_str!("fixtures/agent.py"),
        )
        .unwrap();
        fs::write(
            fixture.root.join("history-fixture.json"),
            r#"{"replay_tools":80}"#,
        )
        .unwrap();
        let info = fixture.create_agent(provider, true, None, vec![]);
        let mut view = fixture.attach(&info);
        view.until(|s| s.history.as_ref().is_some_and(|h| !h.busy && h.can_resume));
        view.send(Action::HistoryResume {
            id: "saved-1".into(),
        });
        let restored = view.until(|s| s.history.is_none() && s.chat.connected);
        assert!(
            !restored
                .chat
                .messages
                .iter()
                .any(|m| m.text == "Earlier question 1")
        );
        assert_eq!(
            restored.chat.input_history.as_ref().unwrap(),
            &["Earlier question 1", "Earlier question 2"]
        );
        let mut delta = restored.clone();
        delta.chat.input_history = None;
        delta.chat.messages.clear();
        let order = restored
            .chat
            .messages
            .iter()
            .map(|m| m.id.clone())
            .collect();
        let assembled = rode_agent_core::service::Update {
            state: delta,
            order,
        }
        .apply(Some(&restored))
        .unwrap();
        assert_eq!(assembled.chat.input_history, restored.chat.input_history);
        let mut incomplete = restored.clone();
        incomplete.chat.input_history = None;
        let order = incomplete
            .chat
            .messages
            .iter()
            .map(|m| m.id.clone())
            .collect();
        assert!(
            rode_agent_core::service::Update {
                state: incomplete,
                order
            }
            .apply(None)
            .is_err()
        );
        view.send(Action::Submit {
            text: "new follow-up".into(),
            attachments: vec![],
            config: Box::new(restored.chat.config),
        });
        let done = view.until(|s| s.chat.completed == 1 && !s.chat.busy);
        assert_eq!(
            done.chat.input_history.as_ref().unwrap(),
            &["Earlier question 1", "Earlier question 2", "new follow-up"]
        );
        drop(view);
        wait(|| {
            fixture
                .infos()
                .iter()
                .any(|i| i.id == info.id && !i.attached)
        });
        let mut reattached = fixture.attach(&info);
        let ready = reattached.until(|s| s.chat.connected && !s.chat.busy);
        assert_eq!(ready.chat.input_history, done.chat.input_history);
        reattached.send(Action::End);
        reattached.until(|s| !s.info.alive);
    }
}

#[test]
fn history_marks_attached_and_detached_owners_without_trying_to_resume_them() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Acp] {
        let fixture = Fixture::new();
        fs::write(
            fixture.root.join("agent.py"),
            include_str!("fixtures/agent.py"),
        )
        .unwrap();
        fs::write(
            fixture.root.join("history-fixture.json"),
            r#"{"include_open":true}"#,
        )
        .unwrap();
        let owner = fixture.create(provider, true, "current");
        let current = fixture.attach(&owner);
        let state = current.until(|s| s.chat.connected && s.chat.completed == 1);
        let conversation = state.chat.conversation;

        let info = fixture.create_agent(provider, false, None, vec![]);
        assert!(
            fixture
                .infos()
                .iter()
                .any(|item| item.id == info.id && item.history_browser),
            "A starting history browser must remain routable with its browser role"
        );
        let mut browser = fixture.attach(&info);
        let state = browser.until(|s| s.history.as_ref().is_some_and(|h| !h.busy && h.can_resume));
        let history = state.history.unwrap();
        assert_eq!(history.owners.get(&conversation), Some(&Owner::Attached));
        assert!(!history.owners.contains_key("saved-1"));
        assert!(
            fixture
                .infos()
                .iter()
                .any(|item| item.id == info.id && item.history_browser),
            "An idle history browser lost its role"
        );
        browser.send(Action::HistoryResume {
            id: conversation.clone(),
        });
        let state = browser.until(|s| !s.error.is_empty());
        assert!(state.error.contains("already has an active writer"));
        assert!(state.history.unwrap().can_resume);
        let calls = fs::read_to_string(fixture.root.join("agent-rpc.jsonl")).unwrap();
        assert!(!calls.lines().any(|line| {
            let call: serde_json::Value = serde_json::from_str(line).unwrap();
            matches!(
                call["method"].as_str(),
                Some("thread/resume" | "session/load")
            )
        }));

        drop(current);
        wait(|| {
            fixture
                .infos()
                .iter()
                .any(|i| i.id == owner.id && !i.attached)
        });
        browser.send(Action::HistoryMore);
        browser.until(|s| {
            s.history
                .as_ref()
                .is_some_and(|h| !h.busy && h.owners.get(&conversation) == Some(&Owner::Detached))
        });
        fixture
            .rpc(&Request::EndAgent {
                id: owner.id.clone(),
                policy: owner.policy.clone(),
            })
            .unwrap();
        // Finished records can already have been reaped by discovery.
        wait(|| fixture.infos().iter().all(|i| i.id != owner.id || !i.alive));

        let refreshed = fixture.create_agent(provider, false, None, vec![]);
        let restored_id = refreshed.id.clone();
        let mut refreshed = fixture.attach(&refreshed);
        let state =
            refreshed.until(|s| s.history.as_ref().is_some_and(|h| !h.busy && h.can_resume));
        assert!(!state.history.unwrap().owners.contains_key(&conversation));
        refreshed.send(Action::HistoryResume { id: conversation });
        refreshed.until(|s| s.history.is_none() && s.chat.connected);
        assert!(
            fixture
                .infos()
                .iter()
                .any(|item| item.id == restored_id && item.alive && !item.history_browser),
            "The restored chat did not appear in Running"
        );
        browser.send(Action::End);
        browser.until(|s| !s.info.alive);
        assert!(
            fixture
                .infos()
                .iter()
                .all(|item| item.id != info.id || item.history_browser),
            "A closed history browser appeared in Running"
        );
    }
}

#[test]
fn history_deletion_uses_host_backend_and_protects_another_open_controller() {
    for (provider, browser_provider) in [
        (Provider::Codex, Provider::Codex),
        (Provider::Claude, Provider::Claude),
        (Provider::Acp, Provider::Acp),
        (Provider::Claude, Provider::Acp),
        (Provider::Acp, Provider::Claude),
    ] {
        let fixture = Fixture::new();
        fs::write(
            fixture.root.join("agent.py"),
            include_str!("fixtures/agent.py"),
        )
        .unwrap();
        fs::write(
            fixture.root.join("history-fixture.json"),
            serde_json::json!({"include_open":true,"parents":
                if provider == Provider::Codex { serde_json::json!({"thread-1":"saved-1"}) } else { serde_json::json!({}) }
            }).to_string(),
        )
        .unwrap();
        let info = fixture.create(provider, true, "current");
        let mut current = fixture.attach(&info);
        let state = current.until(|s| s.chat.connected && s.chat.completed == 1);
        let conversation = state.chat.conversation.clone();
        let config = state.chat.config.clone();
        assert!(!conversation.is_empty());
        let info = fixture.create_agent(browser_provider, false, None, vec![]);
        let mut browser = fixture.attach(&info);
        let listed = browser.until(|s| s.history.as_ref().is_some_and(|h| !h.busy && h.can_delete));
        assert_eq!(
            listed.history.unwrap().owners.get(&conversation),
            Some(&Owner::Attached)
        );
        browser.send(Action::HistoryDelete {
            id: None,
            protected: vec![],
        });
        let state = browser.until(|s| {
            s.history
                .as_ref()
                .is_some_and(|h| !h.busy && h.status.starts_with("Deleted"))
        });
        let history = state.history.unwrap();
        assert!(
            history.status.contains(if provider == Provider::Codex {
                "kept 2"
            } else {
                "kept 1"
            }),
            "{}",
            history.status
        );
        let deleted: Vec<String> =
            serde_json::from_slice(&fs::read(fixture.root.join("history-deleted.json")).unwrap())
                .unwrap();
        assert_eq!(
            deleted,
            if provider == Provider::Codex {
                vec!["saved-2"]
            } else {
                vec!["saved-1", "saved-2"]
            }
        );
        assert!(history.entries.iter().any(|e| e.id == conversation));
        current.send(Action::Submit {
            text: "still open".into(),
            attachments: vec![],
            config: Box::new(config),
        });
        let state = current.until(|s| s.chat.completed == 2);
        assert_eq!(state.chat.conversation, conversation);
    }
}

#[test]
fn persistent_agents_finish_without_a_view_and_resume_the_same_conversation() {
    for provider in Provider::ALL {
        let fixture = Fixture::new();
        assert!(fixture.infos().is_empty());
        assert!(
            !fixture.runtime.join("socket").exists(),
            "Discovery started a service"
        );
        let info = fixture.create(provider, true, "work");
        let view = fixture.attach(&info);
        view.until(|s| s.chat.connected && s.chat.busy);
        wait(|| fixture.root.join("work-started").exists());
        drop(view);
        wait(|| {
            fixture
                .infos()
                .iter()
                .any(|i| i.id == info.id && !i.attached)
        });
        fs::write(fixture.root.join("continue"), "").unwrap();
        wait(|| fixture.root.join("work-done").exists());
        let mut view = fixture.attach(&info);
        let state = view.until(|s| s.chat.completed == 1 && !s.chat.busy);
        assert!(
            state
                .chat
                .messages
                .iter()
                .any(|m| m.text.contains("background complete 1"))
        );
        view.send(Action::Submit {
            attachments: vec![],
            text: "second".into(),
            config: Box::new(state.chat.config),
        });
        let state = view.until(|s| s.chat.completed == 2 && !s.chat.busy);
        assert_eq!(
            state
                .chat
                .messages
                .iter()
                .filter(|m| m.role == rode_agent_core::protocol::Role::User)
                .count(),
            2
        );
        view.send(Action::End);
        view.until(|s| !s.info.alive);
        drop(view);
        wait(|| fixture.infos().is_empty());
        wait(|| !fixture.runtime.join("socket").exists());
    }
}
#[test]
fn persistence_toggles_in_place_and_off_ends_the_last_view() {
    let fixture = Fixture::new();
    let info = fixture.create(Provider::Codex, false, "work");
    let mut view = fixture.attach(&info);
    view.until(|s| s.chat.connected);
    wait(|| fixture.root.join("agent.pid").exists());
    let pid = fs::read_to_string(fixture.root.join("agent.pid")).unwrap();
    view.send(Action::Persistence { persistent: true });
    view.until(|s| s.info.persistent);
    drop(view);
    wait(|| {
        fixture
            .infos()
            .iter()
            .any(|i| i.id == info.id && !i.attached)
    });
    let mut view = fixture.attach(&info);
    view.until(|s| s.chat.connected);
    assert_eq!(
        fs::read_to_string(fixture.root.join("agent.pid")).unwrap(),
        pid
    );
    view.send(Action::Persistence { persistent: false });
    view.until(|s| !s.info.persistent);
    drop(view);
    wait(|| fixture.infos().is_empty());
    assert!(!fixture.root.join("work-done").exists());
}
#[test]
fn unanswered_questions_survive_detachment_and_hold_queued_work_until_an_explicit_answer() {
    let fixture = Fixture::new();
    let info = fixture.create(Provider::Codex, true, "question");
    let mut view = fixture.attach(&info);
    let state = view.until(|s| s.chat.prompts.len() == 1);
    let question = state.chat.prompts[0].clone();
    assert!(state.chat.busy);
    assert!(question.blocking);
    assert_eq!(question.questions[0].descriptions, ["Run the full suite"]);
    view.queue(
        "after-question",
        rode_agent_core::queue::Action::Push {
            id: 1,
            text: "next task".into(),
            attachments: vec![],
        },
    );
    drop(view);
    fs::write(fixture.root.join("continue"), "").unwrap();
    wait(|| {
        fixture
            .infos()
            .iter()
            .any(|i| i.id == info.id && !i.attached)
    });
    let mut view = fixture.attach(&info);
    let state = view.until(|s| s.chat.prompts.len() == 1);
    assert!(state.chat.busy);
    assert_eq!(state.chat.completed, 0);
    assert_eq!(state.queue.items.len(), 1);
    assert!(!fixture.root.join("work-done").exists());
    assert!(!fixture.root.join("question-answer.json").exists());
    view.send(Action::Answer {
        id: question.id,
        result: serde_json::json!({"answers":{"scope":{"answers":["Focused tests please"]}}}),
    });
    view.until(|s| s.chat.completed == 2 && !s.chat.busy);
    let answer: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fixture.root.join("question-answer.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        answer["answers"]["scope"]["answers"][0],
        "Focused tests please"
    );
    view.send(Action::End);
    view.until(|s| !s.info.alive);
}

#[test]
fn offline_approvals_wait_and_wrong_policy_or_stale_answers_are_rejected() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Acp] {
        let fixture = Fixture::new();
        let info = fixture.create(provider, true, "approval");
        let mut view = fixture.attach(&info);
        let state = view.until(|s| s.chat.prompts.len() == 1);
        let question = state.chat.prompts[0].clone();
        view.queue(
            "after-approval",
            rode_agent_core::queue::Action::Push {
                id: 1,
                text: "next queued task".into(),
                attachments: vec![],
            },
        );
        drop(view);
        fs::write(fixture.root.join("continue"), "").unwrap();
        std::thread::sleep(Duration::from_millis(250));
        assert!(!fixture.root.join("work-done").exists());
        assert!(
            fixture
                .rpc(&Request::EndAgent {
                    id: info.id.clone(),
                    policy: "b".repeat(64)
                })
                .is_err()
        );
        let mut view = fixture.attach(&info);
        view.until(|s| s.chat.prompts.len() == 1);
        view.send(Action::Answer {
            id: "stale".into(),
            result: question.choices[0].result.clone(),
        });
        view.until(|s| !s.error.is_empty() && s.chat.prompts.len() == 1);
        view.send(Action::Answer {
            id: question.id,
            result: question.choices[0].result.clone(),
        });
        view.until(|s| s.chat.completed == 2 && !s.chat.busy);
        assert!(fixture.root.join("work-done").exists());
        view.send(Action::End);
        view.until(|s| !s.info.alive);
    }
}

#[test]
fn asynchronous_questions_survive_detachment_and_hold_queued_work_until_answered() {
    use rode_agent_core::queue::Action as Queue;
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("agent.py"),
        include_str!("fixtures/agent.py"),
    )
    .unwrap();
    let info = fixture.create(Provider::Codex, true, "async-question-finished");
    let mut view = fixture.attach(&info);
    let state = view.until(|state| !state.chat.busy && !state.chat.prompts.is_empty());
    let question_id = state.chat.prompts[0].id.clone();
    assert!(state.chat.prompts[0].is_async_message());
    view.queue(
        "held",
        Queue::Push {
            id: 1,
            text: "queued after the answer".into(),
            attachments: vec![],
        },
    );
    let state = view.until(|state| state.queue.items.len() == 1);
    assert_eq!(state.chat.completed, 1);
    assert!(!fixture.root.join("agent-created.txt").exists());
    drop(view);
    wait(|| {
        fixture
            .infos()
            .iter()
            .any(|agent| agent.id == info.id && !agent.attached)
    });
    let mut view = fixture.attach(&info);
    let state = view.until(|state| !state.chat.prompts.is_empty());
    assert_eq!(state.chat.prompts[0].id, question_id);
    assert!(state.chat.prompts[0].partial_answers);
    view.send(Action::Answer {
        id: question_id.clone(),
        result: serde_json::json!({"answers":{"question-0":{"answers":["Disabled"]}}}),
    });
    let state = view.until(|state| !state.chat.busy && state.chat.completed == 2);
    assert!(state.chat.prompts[0].questions[0].answered);
    assert!(!state.chat.prompts[0].questions[1].answered);
    assert_eq!(state.queue.items.len(), 1);
    assert!(!fixture.root.join("agent-created.txt").exists());
    drop(view);
    wait(|| {
        fixture
            .infos()
            .iter()
            .any(|agent| agent.id == info.id && !agent.attached)
    });
    let mut view = fixture.attach(&info);
    let state = view.until(|state| !state.chat.prompts.is_empty());
    assert!(state.chat.prompts[0].questions[0].answered);
    view.send(Action::Answer {
        id: question_id,
        result: serde_json::json!({"answers":{
            "question-1":{"answers":["Use the model-only connection"]}
        }}),
    });
    let state = view.until(|state| state.chat.completed == 4 && !state.chat.busy);
    assert!(state.chat.prompts.is_empty());
    assert!(state.queue.items.is_empty());
    assert!(
        fs::read_to_string(fixture.root.join("async-answers.json"))
            .unwrap()
            .contains("Use the model-only connection")
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("agent-created.txt")).unwrap(),
        "queued after the answer"
    );
    view.send(Action::End);
    view.until(|state| !state.info.alive);
}

#[test]
fn automatic_queue_preserves_pause_edit_and_failure_holds_while_detached() {
    use rode_agent_core::queue::Action as Queue;
    for provider in [Provider::Codex, Provider::Claude, Provider::Acp] {
        for hold in ["pause", "edit", "failure"] {
            let fixture = Fixture::new();
            if hold == "failure" {
                fs::write(fixture.root.join("fail-first"), "").unwrap();
            }
            let info = fixture.create(provider, true, "work");
            let mut view = fixture.attach(&info);
            view.until(|s| s.chat.connected && s.chat.busy);
            view.queue(
                "push",
                Queue::Push {
                    id: 1,
                    text: "held follow-up".into(),
                    attachments: vec![],
                },
            );
            match hold {
                "pause" => {
                    view.queue("hold", Queue::Pause { paused: true });
                }
                "edit" => {
                    view.queue("hold", Queue::Edit { id: 1 });
                }
                _ => {}
            }
            drop(view);
            wait(|| {
                fixture
                    .infos()
                    .iter()
                    .any(|i| i.id == info.id && !i.attached)
            });
            fs::write(fixture.root.join("continue"), "").unwrap();
            wait(|| fixture.root.join("work-done").exists());
            let mut view = fixture.attach(&info);
            let state = view.until(|s| s.chat.completed == 1 && !s.chat.busy);
            assert_eq!(state.queue.items.len(), 1, "{provider:?} {hold}");
            assert_eq!(
                fs::read_to_string(fixture.root.join("work-done")).unwrap(),
                "1"
            );
            if hold == "edit" {
                assert_eq!(state.queue.editing, Some(1));
                view.queue("release", Queue::CancelEdit { id: 1 });
            } else {
                assert!(state.queue.paused);
                view.queue("release", Queue::Pause { paused: false });
            }
            view.until(|s| s.chat.completed == 2 && !s.chat.busy && s.queue.items.is_empty());
            view.send(Action::End);
            view.until(|s| !s.info.alive);
        }
    }
}

#[test]
fn stop_holds_automatic_follow_ups_until_explicit_resume() {
    use rode_agent_core::queue::Action as Queue;
    let fixture = Fixture::new();
    let info = fixture.create(Provider::Codex, true, "work");
    let mut view = fixture.attach(&info);
    view.until(|s| s.chat.connected && s.chat.busy);
    view.queue(
        "push",
        Queue::Push {
            id: 1,
            text: "after stop".into(),
            attachments: vec![],
        },
    );
    view.send(Action::Cancel);
    view.until(|s| s.chat.completed == 1 && !s.chat.busy && s.queue.paused);
    drop(view);
    wait(|| {
        fixture
            .infos()
            .iter()
            .any(|i| i.id == info.id && !i.attached)
    });
    fs::write(fixture.root.join("continue"), "").unwrap();
    let mut view = fixture.attach(&info);
    let state = view.until(|s| s.chat.completed == 1 && !s.chat.busy && s.queue.paused);
    assert_eq!(state.queue.items.len(), 1);
    assert!(!fixture.root.join("work-done").exists());
    view.queue("resume", Queue::Pause { paused: false });
    view.until(|s| s.chat.completed == 2 && !s.chat.busy && s.queue.items.is_empty());
    view.send(Action::End);
    view.until(|s| !s.info.alive);
}

#[test]
fn automatic_queue_configuration_cannot_change_the_captured_backend() {
    use rode_agent_core::queue::Action as Queue;
    let fixture = Fixture::new();
    let info = fixture.create(Provider::Codex, true, "work");
    let mut view = fixture.attach(&info);
    let state = view.until(|s| s.chat.connected && s.chat.busy);
    let original = state.chat.config;
    let mut invalid = original.clone();
    invalid.codex = "/different-agent".into();
    let rejected = view.queue(
        "invalid",
        Queue::Configure {
            config: Box::new(invalid),
        },
    );
    assert!(!rejected.error.is_empty());
    assert!(rejected.queue.paused);
    assert_eq!(rejected.chat.config, original);
    view.queue(
        "push",
        Queue::Push {
            id: 1,
            text: "next task".into(),
            attachments: vec![],
        },
    );
    drop(view);
    wait(|| {
        fixture
            .infos()
            .iter()
            .any(|i| i.id == info.id && !i.attached)
    });
    fs::write(fixture.root.join("continue"), "").unwrap();
    let mut view = fixture.attach(&info);
    view.until(|s| s.chat.completed == 1 && !s.chat.busy && s.queue.paused);
    view.queue(
        "restore",
        Queue::Configure {
            config: Box::new(original),
        },
    );
    view.queue("resume", Queue::Pause { paused: false });
    view.until(|s| s.chat.completed == 2 && !s.chat.busy && s.queue.items.is_empty());
    view.send(Action::End);
    view.until(|s| !s.info.alive);
}

#[test]
fn native_agent_creation_and_follow_ups_preserve_large_document_snapshots() {
    use rode_agent_core::attachment::Attachment;
    let fixture = Fixture::new();
    let document = Attachment::Document {
        name: "large.txt".into(),
        text: "complete document contents\n".repeat(14000),
    };
    let info =
        fixture.create_with_attachments(Provider::Codex, false, "first", vec![document.clone()]);

    fs::write(fixture.root.join("continue"), "").unwrap();
    let mut view = fixture.attach(&info);
    let state = view.until(|s| s.chat.completed == 1 && !s.chat.busy);
    assert!(
        state
            .chat
            .messages
            .iter()
            .any(|m| m.text.contains("Attachments: large.txt"))
    );
    assert!(
        !state
            .chat
            .messages
            .iter()
            .any(|m| m.text.contains("complete document contents"))
    );
    view.send(Action::Submit {
        text: "follow up".into(),
        attachments: vec![document],
        config: Box::new(state.chat.config),
    });
    let state = view.until(|s| s.chat.completed == 2 && !s.chat.busy);
    assert!(
        state
            .chat
            .messages
            .iter()
            .any(|m| m.text == "follow up\n\nAttachments: large.txt")
    );
    view.send(Action::End);
    view.until(|s| !s.info.alive);
}

#[test]
fn queued_prompts_and_files_survive_view_replacement_without_reordering_or_duplicate_dispatch() {
    use rode_agent_core::{attachment::Attachment, queue::Action as Queue};
    for provider in Provider::ALL {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("record-inputs"), "").unwrap();
        let info = fixture.create(provider, true, "work");

        let mut view = fixture.attach(&info);
        view.until(|s| s.chat.connected && s.chat.busy);
        let file = Attachment::Document {
            name: "snapshot.txt".into(),
            text: "Retained snapshot λ\n".repeat(18000),
        };
        view.queue(
            "push-first",
            Queue::Push {
                id: 1,
                text: "first".into(),
                attachments: vec![file],
            },
        );
        view.queue("pause", Queue::Pause { paused: true });
        view.queue(
            "push-second",
            Queue::Push {
                id: 2,
                text: "second".into(),
                attachments: vec![],
            },
        );
        view.queue(
            "reorder",
            Queue::Move {
                id: 2,
                earlier: true,
            },
        );
        view.queue("edit", Queue::Edit { id: 1 });
        view.queue(
            "save",
            Queue::Save {
                id: 1,
                text: "edited first".into(),
            },
        );
        drop(view);
        wait(|| {
            fixture
                .infos()
                .iter()
                .any(|i| i.id == info.id && !i.attached)
        });
        fs::write(fixture.root.join("continue"), "").unwrap();
        wait(|| fixture.root.join("work-done").exists());
        assert_eq!(
            fs::read_to_string(fixture.root.join("work-done")).unwrap(),
            "1"
        );
        let mut view = fixture.attach(&info);
        let state = view.until(|s| s.chat.completed == 1 && !s.chat.busy);
        assert!(state.queue.paused);
        assert_eq!(
            state
                .queue
                .items
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>(),
            ["second", "edited first"]
        );
        assert_eq!(state.queue.items[1].files, ["snapshot.txt"]);
        assert!(
            serde_json::to_vec(&state).unwrap().len() < 64 * 1024,
            "Attachment contents must stay on the host"
        );
        view.queue("resume", Queue::Pause { paused: false });
        let state = view.until(|s| s.chat.completed == 3 && !s.chat.busy);
        let duplicate = view.queue(
            "duplicate",
            Queue::Dispatch {
                id: 2,
                config: Box::new(state.chat.config.clone()),
            },
        );
        assert!(duplicate.error.is_empty());
        assert!(duplicate.queue.items.is_empty());
        assert_eq!(duplicate.queue.dispatched.len(), 2);
        assert!(!duplicate.queue.paused);
        drop(view);
        wait(|| {
            fixture
                .infos()
                .iter()
                .any(|i| i.id == info.id && !i.attached)
        });
        let mut view = fixture.attach(&info);
        let state = view.until(|s| s.chat.completed == 3);
        assert!(state.queue.items.is_empty());
        let calls: Vec<serde_json::Value> =
            fs::read_to_string(fixture.root.join("recorded-inputs.jsonl"))
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[1][0]["text"], "second");
        assert_eq!(calls[2][0]["text"], "edited first");
        assert!(calls[2].as_array().unwrap().iter().any(|b| {
            b["text"]
                .as_str()
                .is_some_and(|s| s.contains("Retained snapshot λ"))
        }));
        view.send(Action::End);
        view.until(|s| !s.info.alive);
    }
}

#[test]
fn queued_prompts_dispatch_without_any_desktop_view_and_keep_captured_files_and_settings() {
    use rode_agent_core::{
        attachment::Attachment, personality::Personality, queue::Action as Queue,
    };
    for provider in Provider::ALL {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("record-inputs"), "").unwrap();
        let info = fixture.create(provider, true, "work");

        let mut view = fixture.attach(&info);
        let state = view.until(|s| s.chat.connected && s.chat.busy);
        let mut config = state.chat.config;
        config.personality = Personality::Panne;
        view.queue(
            "settings",
            Queue::Configure {
                config: Box::new(config.clone()),
            },
        );
        view.queue(
            "push-first",
            Queue::Push {
                id: 1,
                text: "detached first".into(),
                attachments: vec![Attachment::Document {
                    name: "snapshot.txt".into(),
                    text: "Immutable detached snapshot λ".into(),
                }],
            },
        );
        view.queue(
            "push-second",
            Queue::Push {
                id: 2,
                text: "detached second".into(),
                attachments: vec![],
            },
        );
        drop(view);
        wait(|| {
            fixture
                .infos()
                .iter()
                .any(|i| i.id == info.id && !i.attached)
        });
        fs::write(fixture.root.join("continue"), "").unwrap();
        wait(|| fs::read_to_string(fixture.root.join("work-done")).is_ok_and(|v| v == "3"));
        assert!(
            fixture
                .infos()
                .iter()
                .any(|i| i.id == info.id && !i.attached && i.alive)
        );
        let mut view = fixture.attach(&info);
        let state =
            view.until(|s| s.chat.completed == 3 && !s.chat.busy && s.queue.items.is_empty());
        assert_eq!(state.chat.config, config);
        assert_eq!(
            state
                .queue
                .dispatched
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["detached first", "detached second"]
        );
        assert!(!state.queue.paused);
        let calls: Vec<serde_json::Value> =
            fs::read_to_string(fixture.root.join("recorded-inputs.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[1][0]["text"], "detached first");
        assert_eq!(calls[2][0]["text"], "detached second");
        assert!(
            calls[1]
                .to_string()
                .contains("Immutable detached snapshot λ")
        );
        assert!(calls[1].to_string().contains("panne"));
        view.send(Action::End);
        view.until(|s| !s.info.alive);
    }
}

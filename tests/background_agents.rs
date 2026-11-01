//! Real native controllers and disposable stdio bridges. Fixtures never use a
//! model, account or inference endpoint, and own isolated runtime directories.
use rode_agent_core::{
    Config, Plan, Provider,
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
fn history_deletion_uses_host_backend_and_protects_another_open_controller() {
    for provider in [Provider::Codex, Provider::Acp] {
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
        let info = fixture.create_agent(provider, false, None, vec![]);
        let mut browser = fixture.attach(&info);
        browser.until(|s| s.history.as_ref().is_some_and(|h| !h.busy && h.can_delete));
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
            config,
        });
        let state = current.until(|s| s.chat.completed == 2);
        assert_eq!(state.chat.conversation, conversation);
    }
}

#[test]
fn persistent_agents_finish_without_a_view_and_resume_the_same_conversation() {
    for provider in [Provider::Codex, Provider::Local, Provider::Acp] {
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
            config: state.chat.config,
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
fn offline_approvals_wait_and_wrong_policy_or_stale_answers_are_rejected() {
    for provider in [Provider::Codex, Provider::Acp] {
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
fn automatic_queue_preserves_pause_edit_and_failure_holds_while_detached() {
    use rode_agent_core::queue::Action as Queue;
    for provider in [Provider::Codex, Provider::Acp] {
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
fn automatic_queue_upgrade_preserves_primary_and_compatible_legacy_services() {
    for compatible in [false, true] {
        let fixture = Fixture::new();
        let directory = if compatible {
            fixture.runtime.join("protocol-8")
        } else {
            fixture.runtime.clone()
        };
        fs::create_dir_all(directory.parent().unwrap()).unwrap();
        fs::set_permissions(
            directory.parent().unwrap(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let id = rode_session::new_id().unwrap();
        let mut legacy = Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/session_v1.py"
            ))
            .arg(&directory)
            .arg(&fixture.root)
            .arg(&id)
            .arg("legacy-policy")
            .env("RODE_TEST_RUNTIME_CAPABLE", "1")
            .spawn()
            .unwrap();
        struct Stop<'a>(&'a mut Child);
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                // The fixture handles TERM by closing its owned shell and PTY.
                unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
                let _ = self.0.wait();
            }
        }
        let _stop = Stop(&mut legacy);
        wait(|| directory.join("socket").exists());
        let info = fixture.create(Provider::Codex, true, "work");
        assert!(info.automatic_queue);
        let Response::Sessions(sessions) = fixture.rpc(&Request::List).unwrap() else {
            panic!()
        };
        assert!(sessions.iter().any(|s| s.id == id && s.alive));
        let actions = fs::read_to_string(directory.join("requests")).unwrap();
        assert!(!actions.lines().any(|s| s == "create_agent"));
        if !compatible {
            assert!(actions.lines().any(|s| s == "supports_runtimes"));
            assert!(actions.lines().any(|s| s == "supports_automatic_queue"));
            assert!(
                fixture
                    .runtime
                    .join(format!("protocol-{}/socket", rode_session::PROTOCOL))
                    .exists()
            );
        }
    }
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
    assert!(info.supports_attachments);
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
        config: state.chat.config,
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
    for provider in [Provider::Codex, Provider::Local, Provider::Acp] {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("record-inputs"), "").unwrap();
        let info = fixture.create(provider, true, "work");
        assert!(info.supports_queue);
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
    for provider in [Provider::Codex, Provider::Local, Provider::Acp] {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("record-inputs"), "").unwrap();
        let info = fixture.create(provider, true, "work");
        assert!(info.automatic_queue);
        let mut view = fixture.attach(&info);
        let state = view.until(|s| s.chat.connected && s.chat.busy);
        let mut config = state.chat.config;
        config.personality = Personality::JacksonLamb;
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
        assert!(calls[1].to_string().contains("jackson_lamb"));
        view.send(Action::End);
        view.until(|s| !s.info.alive);
    }
}

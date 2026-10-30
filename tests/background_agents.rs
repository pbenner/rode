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
            text: Some(prompt.into()),
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
}
impl View {
    fn send(&mut self, action: Action) {
        serde_json::to_writer(&mut self.input, &action).unwrap();
        self.input.write_all(b"\n").unwrap();
        self.input.flush().unwrap();
    }
    fn until(&self, check: impl Fn(&State) -> bool) -> State {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let state = self
                .events
                .recv_timeout(Duration::from_secs(12))
                .expect("No agent state");
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
        let view = fixture.attach(&info);
        let state = view.until(|s| s.chat.prompts.len() == 1);
        let question = state.chat.prompts[0].clone();
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
        view.until(|s| s.chat.completed == 1);
        assert!(fixture.root.join("work-done").exists());
        view.send(Action::End);
        view.until(|s| !s.info.alive);
    }
}

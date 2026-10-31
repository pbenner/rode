//! Native host-owned kernels/debuggers retain protocol state across desktop loss.
use rode::{debugger, process, sandbox, sessions};
use rode_runtime_core::service::{
    Command as Action, DebugAction, Envelope, Info, Spec, Start, Update,
};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

struct Fixture {
    root: tempfile::TempDir,
    ids: Vec<(String, String)>,
}
impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            ids: vec![],
        }
    }
    fn runtime(&self) -> std::path::PathBuf {
        self.root.path().join("runtime")
    }
    fn rpc(&self, request: rode_session::Request) -> anyhow::Result<rode_session::Response> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rode-sessiond"))
            .arg("request")
            .env("RODE_SESSION_DIR", self.runtime())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&request)?)?;
        let output = child.wait_with_output()?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<rode_session::Response>(&output.stdout)?.checked()
    }
    fn infos(&self) -> Vec<Info> {
        let rode_session::Response::Runtimes(infos) =
            self.rpc(rode_session::Request::Runtimes).unwrap()
        else {
            panic!()
        };
        infos
    }
    fn create(&mut self, start: Start, argv: Vec<String>, python: &str, persistent: bool) -> Info {
        let mut plan = sandbox::Plan::default();
        plan.root = self.root.path().to_string_lossy().into();
        let spec = Spec {
            sources: BTreeMap::new(),
            id: rode_session::new_id().unwrap(),
            name: "Runtime test".into(),
            root: plan.root.clone(),
            policy: sessions::fingerprint(&plan),
            mode: plan.label().into(),
            document: self.root.path().join("demo.ipynb").to_string_lossy().into(),
            argv,
            prefix: vec![],
            env: BTreeMap::from([(
                "PATH".into(),
                format!(
                    "{}:/usr/bin:/bin",
                    std::path::Path::new(python)
                        .parent()
                        .unwrap_or(std::path::Path::new("/usr/bin"))
                        .display()
                ),
            )]),
            python: python.into(),
            python_program: false,
            persistent,
            target: None,
            start,
        };
        self.ids.push((spec.id.clone(), spec.policy.clone()));
        let rode_session::Response::Runtime(info) = self
            .rpc(rode_session::Request::CreateRuntime {
                spec: Box::new(spec),
            })
            .unwrap()
        else {
            panic!()
        };
        *info
    }
    fn attach(&self, info: &Info) -> process::Session {
        let mut plan = sandbox::Plan::default();
        plan.root = self.root.path().to_string_lossy().into();
        process::Session::start(
            process::Launch {
                argv: vec![
                    "env".into(),
                    format!("RODE_SESSION_DIR={}", self.runtime().display()),
                    env!("CARGO_BIN_EXE_rode-sessiond").into(),
                    "attach-runtime".into(),
                    info.id.clone(),
                    info.policy.clone(),
                ],
                plan,
                remote: None,
                python: "off".into(),
                python_program: false,
                native_debug: false,
                line_limit: 16 * 1024 * 1024,
            },
            Arc::new(|| {}),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for (id, policy) in &self.ids {
            let _ = self.rpc(rode_session::Request::EndRuntime {
                id: id.clone(),
                policy: policy.clone(),
            });
        }
        let until = Instant::now() + Duration::from_secs(9);
        while !self.infos().is_empty() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.rpc(rode_session::Request::Shutdown);
    }
}
fn action(session: &process::Session, command: Action) -> String {
    let id = rode_session::new_id().unwrap();
    session
        .send(
            &serde_json::to_value(Envelope {
                id: id.clone(),
                command,
            })
            .unwrap(),
        )
        .unwrap();
    id
}
fn wait(session: &process::Session, mut check: impl FnMut(&Update) -> bool) -> Update {
    let until = Instant::now() + Duration::from_secs(35);
    loop {
        while let Some(event) = session.poll() {
            match event {
                process::Event::Message(value) => {
                    let update: Update = serde_json::from_value(value).unwrap();
                    if check(&update) {
                        return update;
                    }
                    if let Update::Error(error) = update {
                        panic!("{error}");
                    }
                }
                process::Event::Error(error) => panic!("{error}"),
                process::Event::Exited(code) => panic!("Unexpected view exit {code:?}"),
                _ => {}
            }
        }
        assert!(Instant::now() < until, "Runtime update timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn until(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn real_kernel_queue_variables_and_outputs_survive_detach_and_exclusive_reattachment() {
    let Ok(python) = std::env::var("RODE_TEST_JUPYTER") else {
        eprintln!("Set RODE_TEST_JUPYTER for real persistent notebook coverage");
        return;
    };
    let mut fixture = Fixture::new();
    let info = fixture.create(
        Start::Notebook {
            name: "python3".into(),
        },
        vec![
            python.clone(),
            "-u".into(),
            "-c".into(),
            include_str!("../src/notebook_kernel.py").into(),
        ],
        &python,
        true,
    );
    let session = fixture.attach(&info);
    wait(
        &session,
        |u| matches!(u,Update::State(s)if!s.language.is_null()),
    );
    let marker = fixture.root.path().join("done");
    let runs = vec![
        rode_runtime_core::notebook::Run {
            uid: 1,
            cell: "first".into(),
            version: 0,
            code: "import time\ncounter=41\ntime.sleep(0.5)\nprint(counter)".into(),
        },
        rode_runtime_core::notebook::Run {
            uid: 2,
            cell: "second".into(),
            version: 0,
            code: format!(
                "counter+=1\nprint(counter)\nopen({:?},'w').write(str(counter))",
                marker.to_string_lossy()
            ),
        },
    ];
    let token = action(&session, Action::Run { runs: runs.clone() });
    wait(
        &session,
        |u| matches!(u,Update::State(s)if s.acknowledgements.contains(&token)),
    );
    drop(session);
    until(|| marker.exists());
    assert_eq!(fs::read_to_string(&marker).unwrap(), "42");
    until(|| !fixture.infos()[0].attached);
    let mut wrong = info.clone();
    wrong.policy = "0".repeat(64);
    let error = fixture
        .rpc(rode_session::Request::AttachRuntime {
            id: wrong.id,
            policy: wrong.policy,
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("original project"), "{error}");
    let session = fixture.attach(&info);
    let result = wait(
        &session,
        |u| matches!(u,Update::Cell(c)if c.run.cell=="second"&&c.done.is_some()),
    );
    let Update::Cell(cell) = result else { panic!() };
    assert_eq!(cell.run.code, runs[1].code);
    assert!(cell.outputs.iter().any(|o| o.value["text"] == "42\n"));
    let duplicate = fixture.attach(&info);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut denied = false;
    while !denied {
        while let Some(event) = duplicate.poll() {
            if matches!(event, process::Event::Exited(_) | process::Event::Error(_)) {
                denied = true;
            }
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(duplicate);
    let pid_file = fixture.root.path().join("kernel.pid");
    let token = action(
        &session,
        Action::Run {
            runs: vec![rode_runtime_core::notebook::Run {
                uid: 3,
                cell: "third".into(),
                version: 0,
                code: format!(
                    "import os\ncounter+=1\nprint(counter)\nopen({:?},'w').write(str(os.getpid()))",
                    pid_file.to_string_lossy()
                ),
            }],
        },
    );
    wait(
        &session,
        |u| matches!(u,Update::State(s)if s.acknowledgements.contains(&token)),
    );
    let result = wait(
        &session,
        |u| matches!(u,Update::Cell(c)if c.run.cell=="third"&&c.done.is_some()),
    );
    let Update::Cell(cell) = result else { panic!() };
    assert!(cell.outputs.iter().any(|o| o.value["text"] == "43\n"));

    // An atomic Run All can exceed the ordinary 4 MiB service-request limit.
    // Its acknowledgement and discovery state must not repeat all that source.
    let marker = fixture.root.path().join("large-queue-done");
    let padding = "x".repeat(116 * 1024);
    let runs: Vec<_> = (0..40)
        .map(|index| rode_runtime_core::notebook::Run {
            uid: 100 + index,
            cell: format!("large-{index}"),
            version: 0,
            code: format!(
                "#{padding}\n{}counter+=1\n{}",
                if index == 0 { "time.sleep(0.3)\n" } else { "" },
                if index == 39 {
                    format!(
                        "print(counter)\nopen({:?},'w').write(str(counter))",
                        marker.to_string_lossy()
                    )
                } else {
                    String::new()
                }
            ),
        })
        .collect();
    assert!(serde_json::to_vec(&runs).unwrap().len() > 4 * 1024 * 1024);
    let token = action(&session, Action::Run { runs });
    let update = wait(
        &session,
        |u| matches!(u,Update::State(s)if s.acknowledgements.contains(&token)),
    );
    assert!(serde_json::to_vec(&update).unwrap().len() < 32 * 1024);
    drop(session);
    until(|| marker.exists());
    assert_eq!(fs::read_to_string(marker).unwrap(), "83");
    until(|| !fixture.infos()[0].attached);
    let session = fixture.attach(&info);
    let result = wait(
        &session,
        |u| matches!(u,Update::Cell(c)if c.run.cell=="large-39"&&c.done.is_some()),
    );
    let Update::Cell(cell) = result else { panic!() };
    assert!(cell.outputs.iter().any(|o| o.value["text"] == "83\n"));
    let token = action(&session, Action::Persistence { persistent: false });
    wait(
        &session,
        |u| matches!(u,Update::State(s)if!s.info.persistent&&s.acknowledgements.contains(&token)),
    );
    drop(session);
    until(|| fixture.infos().is_empty());
    let pid = fs::read_to_string(pid_file).unwrap();
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "Runtime removal was acknowledged before its kernel exited"
    );
}

#[test]
fn debugger_pause_stack_variables_and_terminal_survive_detach_with_epoch_guards() {
    let mut fixture = Fixture::new();
    let root = fixture.root.path();
    let file = root.join("main.py");
    fs::write(&file, "answer=42\n").unwrap();
    let adapter = root.join("adapter.py");
    fs::write(&adapter, include_str!("fixtures/debug_adapter.py")).unwrap();
    let mut settings = debugger::Settings::default();
    settings.config.backend = debugger::Backend::Custom;
    settings.config.adapter = "python3".into();
    settings.config.adapter_arguments = vec![adapter.to_string_lossy().into()];
    let start = debugger::Start {
        root: root.to_string_lossy().into(),
        file: file.to_string_lossy().into(),
        python: "python3".into(),
        config: settings.config.clone(),
    };
    let info = fixture.create(
        Start::Debugger {
            launch: Box::new(start),
            settings: Box::new(settings),
        },
        vec![
            "python3".into(),
            "-u".into(),
            "-c".into(),
            include_str!("../src/remote_debugger.py").into(),
        ],
        "/usr/bin/python3",
        true,
    );
    let session = fixture.attach(&info);
    wait(
        &session,
        |u| matches!(u,Update::State(s)if s.debugger.as_ref().is_some_and(|d|d.phase==debugger::protocol::Phase::Paused&&!d.variables.is_empty())),
    );
    drop(session);
    until(|| !fixture.infos()[0].attached);
    let session = fixture.attach(&info);
    let state = wait(
        &session,
        |u| matches!(u,Update::State(s)if s.debugger.as_ref().is_some_and(|d|d.phase==debugger::protocol::Phase::Paused&&!d.variables.is_empty())),
    );
    let Update::State(state) = state else {
        panic!()
    };
    let view = state.debugger.unwrap();
    assert!(!view.frames.is_empty());
    assert!(!view.scopes.is_empty());
    let wrong = action(
        &session,
        Action::Debug {
            epoch: view.epoch.wrapping_sub(1),
            operation: Box::new(DebugAction::Evaluate {
                expression: "answer".into(),
            }),
        },
    );
    let rejected = wait(
        &session,
        |u| matches!(u,Update::State(s)if s.acknowledgements.contains(&wrong)),
    );
    let Update::State(rejected) = rejected else {
        panic!()
    };
    assert!(
        rejected.error.contains("stop changed"),
        "{}",
        rejected.error
    );
    let token = action(
        &session,
        Action::Debug {
            epoch: view.epoch,
            operation: Box::new(DebugAction::Control {
                command: "next".into(),
            }),
        },
    );
    let changed = wait(
        &session,
        |u| matches!(u,Update::State(s)if s.acknowledgements.contains(&token)&&s.debugger.as_ref().is_some_and(|d|d.phase==debugger::protocol::Phase::Paused&&d.epoch>view.epoch)),
    );
    let Update::State(changed) = changed else {
        panic!()
    };
    assert!(changed.debugger.unwrap().epoch > view.epoch);
    fixture
        .rpc(rode_session::Request::EndRuntime {
            id: info.id,
            policy: info.policy,
        })
        .unwrap();
    until(|| fixture.infos().is_empty());
    drop(session);
}

#[test]
fn display_update_overflow_stops_the_kernel_and_replays_its_earlier_outputs() {
    let mut fixture = Fixture::new();
    let bridge = r#"
import sys, json

def emit(kind, **values):
    print(json.dumps(dict(kind=kind, **values)), flush=True)

for line in sys.stdin:
    message = json.loads(line)
    if message['cmd'] == 'start':
        emit('ready', python='/usr/bin/python3', language_info={'name':'python'}, host='fixture')
    elif message['cmd'] == 'execute':
        update = message['code'] == 'update'
        emit('output', id=message['id'],
             output={'output_type':'display_data', 'data':{'text/plain':'x'*(512*1024 if update else 1)}, 'metadata':{}},
             display_id='shared', update=update)
        if not update:
            emit('done', id=message['id'], status='ok')
    elif message['cmd'] == 'shutdown':
        break
"#;
    let info = fixture.create(
        Start::Notebook {
            name: "python3".into(),
        },
        vec!["python3".into(), "-u".into(), "-c".into(), bridge.into()],
        "/usr/bin/python3",
        true,
    );
    let session = fixture.attach(&info);
    wait(
        &session,
        |u| matches!(u, Update::State(s) if !s.language.is_null()),
    );
    let runs = (0..20)
        .map(|uid| rode_runtime_core::notebook::Run {
            uid,
            cell: format!("cell-{uid}"),
            version: 0,
            code: format!("seed-{uid}"),
        })
        .collect();
    let token = action(&session, Action::Run { runs });
    wait(
        &session,
        |u| matches!(u, Update::State(s) if !s.info.busy && s.acknowledgements.contains(&token)),
    );
    action(
        &session,
        Action::Run {
            runs: vec![rode_runtime_core::notebook::Run {
                uid: 100,
                cell: "update".into(),
                version: 0,
                code: "update".into(),
            }],
        },
    );
    wait(
        &session,
        |u| matches!(u, Update::State(s) if !s.info.alive && s.error.contains("8 MiB")),
    );
    drop(session);
    until(|| !fixture.infos()[0].attached);
    let session = fixture.attach(&info);
    let mut replayed = BTreeMap::new();
    wait(&session, |update| {
        if let Update::Cell(cell) = update {
            replayed.insert(cell.run.cell.clone(), cell.clone());
        }
        replayed.len() == 20
    });
    assert_eq!(replayed.len(), 20);
    for cell in replayed.values() {
        assert_eq!(cell.outputs.len(), 1);
        assert_eq!(cell.outputs[0].value["data"]["text/plain"], "x");
        assert_eq!(cell.done.as_deref(), Some("ok"));
    }
    assert!(serde_json::to_vec(&replayed).unwrap().len() < 8 * 1024 * 1024);
}

#[test]
fn history_capable_legacy_service_preserves_its_shell_and_creates_runtimes_in_a_compatible_service()
{
    let mut fixture = Fixture::new();
    let legacy_id = rode_session::new_id().unwrap();
    let mut legacy = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/session_v1.py"
        ))
        .arg(fixture.runtime())
        .arg(fixture.root.path())
        .arg(&legacy_id)
        .arg("legacy-policy")
        .env("RODE_TEST_HISTORY_CAPABLE", "1")
        .spawn()
        .unwrap();
    struct StopLegacy<'a>(&'a mut std::process::Child);
    impl Drop for StopLegacy<'_> {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.0.id() as i32, libc::SIGTERM);
            }
            let _ = self.0.wait();
        }
    }
    let _stop = StopLegacy(&mut legacy);
    until(|| fixture.runtime().join("socket").exists());
    let info = fixture.create(
        Start::Notebook {
            name: "python3".into(),
        },
        vec![
            "python3".into(),
            "-u".into(),
            "-c".into(),
            include_str!("fixtures/notebook_bridge.py").into(),
        ],
        "off",
        true,
    );
    assert!(
        fixture
            .runtime()
            .join(format!("protocol-{}/socket", rode_session::PROTOCOL))
            .exists()
    );
    let session = fixture.attach(&info);
    wait(
        &session,
        |u| matches!(u,Update::State(s) if !s.language.is_null()),
    );
    let rode_session::Response::Sessions(infos) = fixture.rpc(rode_session::Request::List).unwrap()
    else {
        panic!()
    };
    assert!(
        infos
            .iter()
            .any(|session| session.id == legacy_id && session.alive)
    );
    let actions = fs::read_to_string(fixture.runtime().join("requests")).unwrap();
    assert!(actions.lines().any(|action| action == "supports_runtimes"));
    assert!(!actions.lines().any(|action| action == "create_runtime"));
}

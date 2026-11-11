//! Agent history, execution, bundled runtimes and persistence across disconnects.
use super::*;

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn agent_history_deletion_roundtrip_over_ssh_and_proxyjump() {
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    for jump in ["", "rode-jump"] {
        let base = tempfile::tempdir().unwrap();
        let remote = Remote::new(Profile {
            host: "rode-test".into(),
            config: config.clone(),
            jump: jump.into(),
            ..Default::default()
        })
        .unwrap();
        let login = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while !remote.connected() {
            assert!(
                login.is_alive() && Instant::now() < until,
                "{}",
                login.contents()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let plan = remote
            .request(&Request::Prepare {
                path: base.path().to_string_lossy().into(),
                sandbox: Default::default(),
            })
            .unwrap()
            .project
            .unwrap();
        agent_roundtrip(&remote, &plan);
        drop(login);
        remote.cleanup();
    }
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn background_agents_survive_ssh_disconnect_and_keep_their_sandbox() {
    background_agent_lifetime_roundtrip(rode_agent_core::service::Persistence::On);
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn auto_background_agents_finish_queued_work_then_exit_after_ssh_disconnect() {
    background_agent_lifetime_roundtrip(rode_agent_core::service::Persistence::Auto);
}

pub(super) fn background_agent_lifetime_roundtrip(
    persistent: rode_agent_core::service::Persistence,
) {
    use rode_agent_core::{
        queue::Action as Queue,
        service::{Command, Persistence, Spec, State, Update},
    };
    use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, sync::Arc};
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    let connect = |profile: Profile| {
        let remote = Remote::new(profile).unwrap();
        let login = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while !remote.connected() {
            assert!(
                login.is_alive() && Instant::now() < until,
                "{}",
                login.contents()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        (remote, login)
    };
    for jump in ["", "rode-jump"] {
        for sandbox in [false, true] {
            let base = tempfile::tempdir().unwrap();
            let root = base.path().join("project");
            std::fs::create_dir(&root).unwrap();
            std::fs::write(base.path().join("private.txt"), "outside project").unwrap();
            std::fs::write(root.join("record-inputs"), "").unwrap();
            let executable = root.join("agent.py");
            std::fs::write(&executable, include_str!("../fixtures/persistent_agent.py")).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            let (remote, login) = connect(Profile {
                host: "rode-test".into(),
                config: config.clone(),
                jump: jump.into(),
                ..Default::default()
            });
            let plan = remote
                .request(&Request::Prepare {
                    path: root.to_string_lossy().into(),
                    sandbox: rode::sandbox::Options {
                        enabled: sandbox,
                        ..Default::default()
                    },
                })
                .unwrap()
                .project
                .unwrap();
            let policy = rode::sessions::fingerprint(&plan);
            let runtime = base.path().join("runtime").to_string_lossy().into_owned();
            let rpc = |remote: &Remote, request| {
                let response = remote
                    .request(&Request::Tool {
                        request: rode::project::Request::Sessions {
                            root: plan.root.clone(),
                            operation: rode::sessions::Operation::Agents {
                                mise: false,
                                request: Box::new(request),
                            },
                            directory: Some(runtime.clone()),
                        },
                    })
                    .unwrap();
                let Some(rode::project::Response::Sessions(reply)) = response.tool else {
                    panic!()
                };
                reply
            };
            let spec = Spec {
                id: rode_session::new_id().unwrap(),
                name: "SSH background fixture".into(),
                policy: policy.clone(),
                mode: plan.label().into(),
                config: rode_agent_core::Config {
                    codex: executable.to_string_lossy().into(),
                    ..Default::default()
                },
                plan: rode_agent_core::Plan {
                    root: plan.root.clone(),
                    active: plan.active,
                    project_read_only: plan.project_read_only,
                    prefix: plan.prefix.clone(),
                },
                executable: executable.to_string_lossy().into(),
                file_tool_plan: Some(serde_json::to_value(&plan).unwrap()),
                env: BTreeMap::new(),
                python: "off".into(),
                persistent,
                text: Some("work".into()),
                attachments: vec![],
            };
            let reply = rpc(
                &remote,
                rode_session::Request::CreateAgent {
                    spec: Box::new(spec),
                },
            );
            let rode_session::Response::Agent(info) = &reply.response else {
                panic!()
            };
            let info = info.clone();
            let attach = |remote: &Remote| {
                let control = rode::sandbox::Plan {
                    root: plan.root.clone(),
                    ..Default::default()
                };
                let mut argv = vec![reply.executable.clone()];
                argv.extend(reply.prefix.clone());
                argv.extend(["attach-agent".into(), info.id.clone(), policy.clone()]);
                rode::process::Session::start(
                    rode::process::Launch {
                        argv,
                        remote: Some(remote.clone()),
                        plan: control,
                        python: "off".into(),
                        python_program: false,
                        line_limit: 4 * 1024 * 1024,
                        native_debug: false,
                    },
                    Arc::new(|| {}),
                )
                .unwrap()
            };
            let mut state: Option<State> = None;
            let first = attach(&remote);
            let until = Instant::now() + Duration::from_secs(12);
            while state.as_ref().is_none_or(|s| !s.chat.connected)
                || !root.join("work-started").exists()
            {
                if let Some(event) = first.poll() {
                    match event {
                        rode::process::Event::Message(value) => {
                            state = Some(
                                serde_json::from_value::<Update>(value)
                                    .unwrap()
                                    .apply(state.as_ref())
                                    .unwrap(),
                            )
                        }
                        rode::process::Event::Error(error)
                        | rode::process::Event::Stderr(error) => panic!("{error}"),
                        _ => {}
                    }
                }
                assert!(Instant::now() < until, "SSH agent did not initialize");
                std::thread::sleep(Duration::from_millis(25));
            }
            let pid = std::fs::read(root.join("agent.pid")).unwrap();
            let scope: serde_json::Value =
                serde_json::from_slice(&std::fs::read(root.join("agent-scope.json")).unwrap())
                    .unwrap();
            assert_eq!(scope["cwd"], plan.root);
            assert_eq!(scope["private_visible"], !plan.active);

            for id in 1..=2 {
                first
                    .send(
                        &serde_json::to_value(Command::Queue {
                            request: format!("ssh-queue-{id}"),
                            operation: Queue::Push {
                                id,
                                text: format!("queued over SSH {id}"),
                                attachments: vec![],
                            },
                        })
                        .unwrap(),
                    )
                    .unwrap();
            }
            let until = Instant::now() + Duration::from_secs(8);
            while state
                .as_ref()
                .is_none_or(|s| s.queue.acknowledged != "ssh-queue-2" || s.queue.items.len() != 2)
            {
                if let Some(rode::process::Event::Message(value)) = first.poll() {
                    state = Some(
                        serde_json::from_value::<Update>(value)
                            .unwrap()
                            .apply(state.as_ref())
                            .unwrap(),
                    );
                }
                assert!(Instant::now() < until, "SSH queue was not acknowledged");
                std::thread::sleep(Duration::from_millis(25));
            }
            drop(first);
            drop(login);
            remote.cleanup();
            std::fs::write(root.join("continue"), "continue").unwrap();
            let until = Instant::now() + Duration::from_secs(8);
            while std::fs::read_to_string(root.join("work-done"))
                .ok()
                .as_deref()
                != Some("3")
            {
                assert!(
                    Instant::now() < until,
                    "Agent queue stopped when SSH disconnected"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
            let (next, login) = connect(remote.profile.clone());
            let second = if persistent == Persistence::On {
                let second = attach(&next);
                let until = Instant::now() + Duration::from_secs(12);
                state = None;
                while state.as_ref().is_none_or(|s| {
                    s.chat.completed != 3 || s.chat.busy || !s.queue.items.is_empty()
                }) {
                    if let Some(rode::process::Event::Message(value)) = second.poll() {
                        state = Some(
                            serde_json::from_value::<Update>(value)
                                .unwrap()
                                .apply(state.as_ref())
                                .unwrap(),
                        );
                    }
                    assert!(Instant::now() < until, "SSH agent did not reattach");
                    std::thread::sleep(Duration::from_millis(25));
                }
                rpc(
                    &next,
                    rode_session::Request::EndAgent {
                        id: info.id.clone(),
                        policy: policy.clone(),
                    },
                );
                Some(second)
            } else {
                None
            };
            assert_eq!(std::fs::read(root.join("agent.pid")).unwrap(), pid);
            let recorded: Vec<serde_json::Value> =
                std::fs::read_to_string(root.join("recorded-inputs.jsonl"))
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            assert_eq!(recorded.len(), 3);
            assert_eq!(recorded[1][0]["text"], "queued over SSH 1");
            assert_eq!(recorded[2][0]["text"], "queued over SSH 2");
            let until = Instant::now() + Duration::from_secs(8);
            loop {
                let rode_session::Response::Agents(infos) =
                    rpc(&next, rode_session::Request::Agents).response
                else {
                    panic!()
                };
                if !infos.iter().any(|i| i.id == info.id) {
                    break;
                }
                assert!(Instant::now() < until, "Remote agent did not stop");
                std::thread::sleep(Duration::from_millis(50));
            }
            if persistent == Persistence::Auto {
                let until = Instant::now() + Duration::from_secs(8);
                while std::path::Path::new(&runtime).join("socket").exists() {
                    assert!(
                        Instant::now() < until,
                        "Auto left an unused remote service running"
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            drop(second);
            drop(login);
            next.cleanup();
        }
    }
}

pub(super) fn bundled_runtime_roundtrip(remote: &Remote, root: &std::path::Path) {
    use serde_json::{Value, json};
    let bytes = std::fs::read("/usr/bin/true").unwrap();
    let target = json!({"size":bytes.len(),"sha256":rode::files::digest(&bytes)});
    let mut spec = json!({"action":"install","version":"0-ssh-fixture",
        "targets":{"x86_64":target,"aarch64":target},
        "notices":[{"file":"LICENSE","sha256":rode::files::digest(b"fixture license\n")}],
        "notice_data":{"LICENSE":"fixture license\n"}});
    let parent = root.join("private-runtime");
    std::fs::create_dir(&parent).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cache = parent.join("cache ' 🦀");
    let native_helper = remote
        .request(&Request::Prepare {
            path: root.to_string_lossy().into(),
            sandbox: Default::default(),
        })
        .unwrap()
        .project
        .unwrap()
        .native_helper
        .unwrap();
    let call = |spec: &Value, bytes: Vec<u8>| {
        let mut command = Command::new(remote.client_program());
        command.args(remote.channel_args(false));
        command.arg(format!(
            "RODE_CODEX_CACHE={} {} --runtime-cache",
            rode::ssh::quote(cache.to_str().unwrap()),
            rode::ssh::quote(&native_helper)
        ));
        let mut input = serde_json::to_vec(spec).unwrap();
        input.push(b'\n');
        input.extend(bytes);
        let text = rode::project::run(command, input, Duration::from_secs(10))
            .unwrap()
            .checked()
            .unwrap();
        serde_json::from_str::<Value>(&text).unwrap()
    };
    let first = call(&spec, bytes.clone());
    spec["action"] = "probe".into();
    let cached = call(&spec, vec![]);
    assert_eq!(cached["cached"], true);
    assert_eq!(first["executable"], cached["executable"]);
    assert!(
        remote.connected(),
        "Runtime deployment closed the authentication master"
    );
    if std::env::var_os("RODE_TEST_CODEX_BUNDLE").is_some() {
        actual_bundled_codex_roundtrip(remote, root);
    }
}

pub(super) fn actual_bundled_codex_roundtrip(remote: &Remote, root: &std::path::Path) {
    use rode::agent::{Config, Event, Launch, Provider, Session};
    use serde_json::{Value, json};
    use std::sync::Arc;
    // Direct and ProxyJump cases share the fixture host's runtime cache. A
    // complete first upload can exceed the cache's bounded lock wait; isolate
    // these optional real-runtime checks without serializing transport tests.
    static RUNTIME_CACHE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _cache = RUNTIME_CACHE.lock().unwrap();
    let home = std::env::var("RODE_TEST_CODEX_REMOTE_HOME").expect("Use scripts/test-ssh.sh");
    for isolated in [false, true] {
        let plan = remote
            .request(&Request::Prepare {
                path: root.to_string_lossy().into(),
                sandbox: rode::sandbox::Options {
                    enabled: isolated,
                    share_network: false,
                    read_write: vec![home.clone()],
                    ..Default::default()
                },
            })
            .unwrap()
            .project
            .unwrap();
        let session = Session::start(
            Launch {
                config: Config {
                    provider: Provider::Local,
                    local_model: "fixture-local-model".into(),
                    local_url: "http://127.0.0.1:9/v1".into(),
                    ..Default::default()
                },
                plan,
                remote: Some(remote.clone()),
                python: "off".into(),
            },
            Arc::new(|| {}),
        )
        .unwrap();
        let reply = |id: u64| {
            let until = Instant::now() + Duration::from_secs(200);
            let mut diagnostics = String::new();
            loop {
                match session.poll() {
                    Some(Event::Message(value)) if value["id"] == id => {
                        assert!(value.get("error").is_none(), "{value}\n{diagnostics}");
                        break value["result"].clone();
                    }
                    Some(Event::Error(error)) => panic!("{error}\n{diagnostics}"),
                    Some(Event::Stderr(text)) => diagnostics.push_str(&text),
                    Some(Event::Exited(code)) => {
                        panic!("Bundled Codex exited: {code:?}\n{diagnostics}")
                    }
                    _ => {}
                }
                assert!(
                    Instant::now() < until,
                    "Bundled SSH Codex timed out: {diagnostics}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        session.send(&json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"rode","title":"RODE","version":"0.1.0"}}})).unwrap();
        let initialized: Value = reply(1);
        assert!(initialized["userAgent"].is_string());
        session
            .send(&json!({"method":"initialized","params":{}}))
            .unwrap();
        session.send(&json!({"id":2,"method":"thread/start","params":{"cwd":root,"model":"fixture-local-model","modelProvider":"rode_local","approvalPolicy":"on-request","sandbox":"workspace-write"}})).unwrap();
        assert_eq!(reply(2)["thread"]["modelProvider"], "rode_local");
        session.stop();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if matches!(session.poll(), Some(Event::Exited(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(remote.connected());
    }
}

pub(super) fn agent_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    use rode::agent::{Access, Approval, Config, Launch, Provider, protocol::Chat};
    use std::{os::unix::fs::PermissionsExt, sync::Arc};
    let root = std::path::Path::new(&plan.root);
    let program = root.join("agent 'quoted'.py");
    std::fs::write(&program, include_str!("../fixtures/agent.py")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.parent().unwrap().join("private.txt"), "hidden").unwrap();
    for provider in [Provider::Codex, Provider::Claude, Provider::Acp] {
        let deleted = root.join("history-deleted.json");
        if deleted.exists() {
            std::fs::remove_file(&deleted).unwrap();
        }
        let mut launch = Launch {
            config: Config {
                provider,
                codex: program.to_string_lossy().into(),
                acp_program: program.to_string_lossy().into(),
                claude_program: program.to_string_lossy().into(),
                personality: rode::agent::Personality::JacksonLamb,
                ..Default::default()
            },
            plan: plan.clone(),
            remote: Some(remote.clone()),
            python: "auto".into(),
        };
        let mut chat = Chat::default();
        chat.submit(launch.clone(), "remote".into(), Arc::new(|| {}))
            .unwrap();
        wait_agent(&mut chat, |c| c.completed == 1);
        let calls = std::fs::read_to_string(root.join("agent-rpc.jsonl")).unwrap();
        let prompt: serde_json::Value = calls
            .lines()
            .rev()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|r| matches!(r["method"].as_str(), Some("turn/start" | "session/prompt")))
            .unwrap();
        let field = if provider.is_acp() { "prompt" } else { "input" };
        assert_eq!(prompt["params"][field][0]["text"], "remote");
        assert!(
            prompt["params"][field][1]["text"]
                .as_str()
                .unwrap()
                .contains("jackson_lamb")
        );
        let scope: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("agent-scope.json")).unwrap())
                .unwrap();
        assert_eq!(scope["cwd"], plan.root);
        assert_eq!(scope["private_visible"], !plan.active);
        assert_eq!(
            std::fs::read_to_string(root.join("agent-created.txt")).unwrap(),
            "remote"
        );
        let _ = std::fs::remove_file(root.join("approved.txt"));
        chat.submit(launch.clone(), "approve".into(), Arc::new(|| {}))
            .unwrap();
        wait_agent(&mut chat, |c| !c.prompts.is_empty());
        assert!(!root.join("approved.txt").exists());
        chat.answer(chat.prompts[0].choices[0].result.clone())
            .unwrap();
        wait_agent(&mut chat, |c| c.completed == 2);
        assert!(root.join("approved.txt").exists());
        chat.submit(launch.clone(), "wait".into(), Arc::new(|| {}))
            .unwrap();
        wait_agent(&mut chat, |c| {
            c.messages.iter().any(|m| m.text.contains("long task"))
        });
        chat.cancel();
        wait_agent(&mut chat, |c| !c.busy);
        assert!(chat.connected());
        launch.config.approval = Approval::Never;
        launch.config.access = Access::Full;
        if provider.is_acp() {
            launch
                .config
                .acp_options_mut()
                .insert("model".into(), serde_json::json!("fixture-deep"));
            launch
                .config
                .acp_options_mut()
                .insert("mode".into(), serde_json::json!("code"));
        } else {
            launch.config.model = "fixture-deep".into();
            launch.config.reasoning_effort = "high".into();
        }
        std::fs::remove_file(root.join("approved.txt")).unwrap();
        let completed = chat.completed;
        chat.submit(launch.clone(), "approve".into(), Arc::new(|| {}))
            .unwrap();
        wait_agent(&mut chat, |c| c.completed > completed);
        assert!(chat.prompts.is_empty());
        assert!(!root.join("approved.txt").exists());
        if provider.is_acp() {
            assert!(
                chat.controls
                    .iter()
                    .any(|c| c.id == "model" && c.current == "fixture-deep")
            );
        } else {
            let log = std::fs::read_to_string(root.join("agent-rpc.jsonl")).unwrap();
            let turn = log
                .lines()
                .rev()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .find(|v| v["method"] == "turn/start")
                .unwrap();
            assert_eq!(turn["params"]["model"], "fixture-deep");
            assert_eq!(turn["params"]["approvalPolicy"], "never");
            if plan.active {
                assert_eq!(turn["params"]["sandboxPolicy"]["type"], "externalSandbox");
            }
        }
        // History opens a separate channel on the same authenticated master.
        // Restoring adopts that channel and applies this project's current grants.
        let mut browser =
            rode::agent::history::Browser::open(launch.clone(), Arc::new(|| {})).unwrap();
        let until = Instant::now() + Duration::from_secs(15);
        while browser.busy {
            browser.poll();
            assert!(Instant::now() < until, "{}", browser.status);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            browser.entries.iter().any(|e| e.id == "saved-1"),
            "{}",
            browser.status
        );
        browser.resume("saved-1").unwrap();
        while browser.busy {
            browser.poll();
            assert!(Instant::now() < until, "{}", browser.status);
            std::thread::sleep(Duration::from_millis(10));
        }
        chat.restore(
            browser
                .take_restored()
                .unwrap_or_else(|| panic!("{}", browser.status)),
        );
        assert!(chat.messages.iter().any(|m| m.text == "Earlier answer 1"));
        // Native history deletion must mutate only this project's backend on
        // the SSH host, while the independent restored chat keeps working.
        let mut browser = rode::agent::history::Browser::open_service_scoped(
            launch.clone(),
            rode::agent::history::Scope::AllProjects,
            Arc::new(|| {}),
        )
        .unwrap();
        let wait_history = |browser: &mut rode::agent::history::Browser| {
            let until = Instant::now() + Duration::from_secs(20);
            while browser.busy {
                browser.poll();
                assert!(Instant::now() < until, "{}", browser.status);
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait_history(&mut browser);
        assert_eq!(
            browser.scope,
            rode::agent::history::Scope::AllProjects,
            "{}",
            browser.status
        );
        assert!(browser.entries.iter().any(|entry| entry.id == "foreign"));
        assert!(!browser.can_delete);
        assert!(browser.resume("foreign").is_err());
        browser
            .set_scope(rode::agent::history::Scope::Project)
            .unwrap();
        wait_history(&mut browser);
        assert!(browser.can_delete, "{}", browser.status);
        browser.load_more().unwrap();
        wait_history(&mut browser);
        browser
            .delete(
                Some("saved-2"),
                chat.conversation_id()
                    .map(str::to_owned)
                    .into_iter()
                    .collect(),
            )
            .unwrap();
        wait_history(&mut browser);
        assert!(browser.status.contains("Deleted 1"), "{}", browser.status);
        assert_eq!(std::fs::read_to_string(deleted).unwrap(), r#"["saved-2"]"#);
        drop(browser);
        assert!(remote.connected());
        std::fs::remove_file(root.join("agent-child.pid")).unwrap();
        chat.submit(launch, "wait".into(), Arc::new(|| {})).unwrap();
        wait_agent(&mut chat, |_| {
            std::fs::read_to_string(root.join("agent-child.pid"))
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .is_some()
        });
        // The fixture's recorded PID belongs to the project sandbox. Resolve
        // its unique argv marker in our namespace before checking cleanup.
        let pid = fixture_process::host_pid(&root.join("agent-child.pid"));
        drop(chat);
        let until = Instant::now() + Duration::from_secs(5);
        while fixture_process::running(pid) {
            assert!(
                Instant::now() < until,
                "Closing SSH agent left its tool process alive"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            remote.connected(),
            "Closing agent chat must preserve the authenticated SSH master"
        );
        assert!(
            remote
                .request(&Request::List {
                    path: plan.root.clone()
                })
                .is_ok()
        );
    }
}

pub(super) fn wait_agent(
    chat: &mut rode::agent::protocol::Chat,
    ready: impl Fn(&rode::agent::protocol::Chat) -> bool,
) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        chat.poll();
        if ready(chat) {
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

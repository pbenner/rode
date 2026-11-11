//! Project-host tools, sandbox enforcement and terminal sizing.
use super::*;

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn file_only_native_tools_stay_on_the_project_host_over_ssh_and_proxyjump() {
    remote_native_tools(false);
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn model_only_tools_keep_network_off_over_ssh_and_proxyjump() {
    remote_native_tools(true);
}

pub(super) fn remote_native_tools(model_only: bool) {
    use rode_agent_core::{
        Access, Config, Plan,
        service::{Spec, State, Update},
    };
    use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, sync::Arc};
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    for jump in ["", "rode-jump"] {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let endpoint = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        endpoint.set_nonblocking(true).unwrap();
        if model_only {
            std::fs::write(
                root.join("controller-probe-port"),
                endpoint.local_addr().unwrap().port().to_string(),
            )
            .unwrap();
        }
        let program = root.join("agent.py");
        // The shared peer selects the system interpreter covered by this policy.
        std::fs::write(&program, include_str!("../fixtures/file_only_agent.py")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("file-tool-steps.json"),serde_json::json!([
            {"params":{"tool":"rode_write_file","arguments":{"path":"remote-source","text":"remote native edit","revision":null}}},
            {"params":{"tool":"exec_command","arguments":{"cmd":"touch forbidden"}}},
            {"params":{"tool":"rode_read_file","arguments":{"path":"../private"}}},
            {"params":{"tool":"rode_run_command","arguments":{"command":"/usr/bin/python3 -c 'import socket; socket.socket()' > offline-network 2>&1; result=$?; test \"$result\" -ne 0 && printf REMOTE_OFFLINE_OK > offline-build"}}}
        ]).to_string()).unwrap();
        std::fs::write(base.path().join("private"), "private").unwrap();
        let remote = Remote::new(Profile {
            host: "rode-test".into(),
            config: config.clone(),
            jump: jump.into(),
            ..Default::default()
        })
        .unwrap();
        let login = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
        let until = Instant::now() + Duration::from_secs(15);
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
                path: root.to_string_lossy().into(),
                sandbox: rode::sandbox::Options {
                    enabled: model_only,
                    backend: rode::sandbox::Backend::Landlock,
                    share_network: !model_only,
                    ..Default::default()
                },
            })
            .unwrap()
            .project
            .unwrap();
        if model_only && !plan.active {
            assert_ne!(
                std::env::var("RODE_TEST_REQUIRE_LANDLOCK").as_deref(),
                Ok("1"),
                "{}",
                plan.reason
            );
            return;
        }
        let policy = rode::sessions::fingerprint(&plan);
        let rpc = |request| {
            let response = remote
                .request(&Request::Tool {
                    request: rode::project::Request::Sessions {
                        root: plan.root.clone(),
                        operation: rode::sessions::Operation::Agents {
                            mise: false,
                            request: Box::new(request),
                        },
                        directory: Some(base.path().join("runtime").to_string_lossy().into()),
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
            name: "SSH file-only fixture".into(),
            policy: policy.clone(),
            mode: plan.label().into(),
            config: Config {
                codex: program.to_string_lossy().into(),
                access: if model_only {
                    Access::Project
                } else {
                    Access::FilesOnly
                },
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
            executable: program.to_string_lossy().into(),
            env: BTreeMap::new(),
            python: "off".into(),
            persistent: false.into(),
            text: Some("remote edit".into()),
            attachments: vec![],
        };
        let reply = rpc(rode_session::Request::CreateAgent {
            spec: Box::new(spec),
        });
        let rode_session::Response::Agent(info) = &reply.response else {
            panic!()
        };
        let mut argv = vec![reply.executable.clone()];
        argv.extend(reply.prefix.clone());
        argv.extend(["attach-agent".into(), info.id.clone(), policy.clone()]);
        let pipe = rode::process::Session::start(
            rode::process::Launch {
                argv,
                remote: Some(remote.clone()),
                plan: rode::sandbox::Plan {
                    root: plan.root.clone(),
                    ..Default::default()
                },
                python: "off".into(),
                python_program: false,
                line_limit: 4 * 1024 * 1024,
                native_debug: false,
            },
            Arc::new(|| {}),
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(15);
        let mut state: Option<State> = None;
        loop {
            if let Some(event) = pipe.poll() {
                match event {
                    rode::process::Event::Message(value) => {
                        state = Some(
                            serde_json::from_value::<Update>(value)
                                .unwrap()
                                .apply(state.as_ref())
                                .unwrap(),
                        )
                    }
                    rode::process::Event::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
            if state.as_ref().is_some_and(|s| s.chat.completed == 1) {
                break;
            }
            assert!(
                Instant::now() < until,
                "SSH file-only controller did not finish: {state:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let state = state.unwrap();
        assert!(state.chat.last_turn_succeeded, "{:?}", state.chat.messages);
        assert_eq!(
            std::fs::read_to_string(root.join("remote-source")).unwrap(),
            "remote native edit"
        );
        assert!(!root.join("forbidden").exists());
        if model_only {
            assert!(
                endpoint.accept().is_ok(),
                "Remote model controller could not reach its endpoint"
            );
            assert_eq!(
                std::fs::read_to_string(root.join("offline-build")).unwrap(),
                "REMOTE_OFFLINE_OK"
            );
            assert!(
                std::fs::read_to_string(root.join("offline-network"))
                    .unwrap()
                    .contains("PermissionError")
            );
        } else {
            assert!(!root.join("offline-build").exists());
        }
        let calls = std::fs::read_to_string(root.join("file-agent-rpc.jsonl")).unwrap();
        for id in [101, 102] {
            let result = calls
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .find(|v| v["id"] == id && v.get("method").is_none())
                .unwrap();
            assert_eq!(result["result"]["success"], false);
        }
        rpc(rode_session::Request::EndAgent {
            id: info.id.clone(),
            policy,
        });
        drop(pipe);
        remote.cleanup();
    }
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn landlock_file_and_command_policy_on_direct_and_proxyjump_hosts() {
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    for jump in ["", "rode-jump"] {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("project ' 🦀");
        std::fs::create_dir(&root).unwrap();
        let secret = base.path().join("secret");
        std::fs::write(&secret, "private").unwrap();
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
                path: root.to_string_lossy().into(),
                sandbox: rode::sandbox::Options {
                    enabled: true,
                    backend: rode::sandbox::Backend::Landlock,
                    share_network: false,
                    ..Default::default()
                },
            })
            .unwrap()
            .project
            .unwrap();
        if !plan.active {
            assert_ne!(
                std::env::var("RODE_TEST_REQUIRE_LANDLOCK").as_deref(),
                Ok("1"),
                "{}",
                plan.reason
            );
            eprintln!("SSH Landlock unavailable: {}", plan.reason);
            return;
        }
        assert_eq!(plan.label(), "SANDBOX · Landlock");
        let path = root.join("saved").to_string_lossy().into_owned();
        remote
            .request_in(
                &Request::Write {
                    path: path.clone(),
                    text: "ssh-landlock".into(),
                    expected: None,
                },
                Some(&plan),
            )
            .unwrap();
        assert_eq!(
            remote
                .request_in(&Request::Read { path }, Some(&plan))
                .unwrap()
                .text,
            "ssh-landlock"
        );
        assert!(
            remote
                .request_in(
                    &Request::Read {
                        path: secret.to_string_lossy().into()
                    },
                    Some(&plan)
                )
                .is_err()
        );
        // Run an actual remote child under the captured prefix, including UDP denial.
        let mut args = plan.prefix.clone();
        args.extend(policy_python(
            r#"
import os, socket, sqlite3
from pathlib import Path
assert os.getcwd() == os.environ['EXPECTED_ROOT']
if Path('/etc/resolv.conf').is_file():
    Path('/etc/resolv.conf').read_bytes()
if Path('/var/lib/command-not-found/commands.db').is_file():
    with sqlite3.connect('file:/var/lib/command-not-found/commands.db?mode=ro', uri=True) as db:
        assert db.execute('select count(*) from sqlite_master').fetchone() is not None
try:
    socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
except PermissionError:
    print('LANDLOCK_SSH_OK')
else:
    raise AssertionError('UDP allowed')
"#,
        ));
        let mut command = Command::new(remote.client_program());
        command.args(remote.channel_args(false)).arg(format!(
            "EXPECTED_ROOT={} {}",
            rode::ssh::quote(root.to_str().unwrap()),
            args.iter()
                .map(|arg| rode::ssh::quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("LANDLOCK_SSH_OK"));
        assert_eq!(std::fs::read_to_string(secret).unwrap(), "private");
        drop(login);
    }
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn mise_tools_and_tasks_resolve_on_the_project_host_over_ssh_and_proxyjump() {
    use std::os::unix::fs::PermissionsExt;
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    for jump in ["", "rode-jump"] {
        let root = tempfile::tempdir().unwrap();
        let mise = root.path().join("mise");
        std::fs::write(&mise, include_str!("../fixtures/mise.py")).unwrap();
        std::fs::set_permissions(&mise, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir(root.path().join("mise-bin")).unwrap();
        let node = root.path().join("mise-bin/node");
        std::fs::write(&node, "#!/bin/sh\nprintf '%s\\n' \"$RODE_MISE_VALUE\"\n").unwrap();
        std::fs::set_permissions(node, std::fs::Permissions::from_mode(0o755)).unwrap();
        let remote = Remote::new(Profile {
            host: "rode-test".into(),
            config: config.clone(),
            jump: jump.into(),
            ..Default::default()
        })
        .unwrap();
        let login = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        while !remote.connected() {
            assert!(
                login.is_alive() && Instant::now() < end,
                "{}",
                login.contents()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let path = root.path().to_string_lossy().into_owned();
        let mut plan = remote
            .request(&Request::Prepare {
                path: path.clone(),
                sandbox: Default::default(),
            })
            .unwrap()
            .project
            .unwrap();
        plan.environment.mise = true;
        plan.environment
            .runtimes
            .insert("mise".into(), mise.to_string_lossy().into_owned());
        let metadata = |action| Request::Tool {
            request: rode::project::Request::Mise {
                root: path.clone(),
                settings: plan.environment.clone(),
                action,
            },
        };
        assert!(
            remote
                .request_in(&metadata(rode::mise::Action::Inspect), Some(&plan))
                .is_err()
        );
        remote
            .request_in(&metadata(rode::mise::Action::Trust), Some(&plan))
            .unwrap();
        let reply = remote
            .request_in(&metadata(rode::mise::Action::Inspect), Some(&plan))
            .unwrap();
        let Some(rode::project::Response::Mise(snapshot)) = reply.tool else {
            panic!("Missing mise metadata")
        };
        assert_eq!(snapshot.tasks[0].name, "smoke");
        let reply = remote
            .request_in(
                &Request::Tool {
                    request: rode::project::Request::Runtimes {
                        root: path.clone(),
                        settings: Default::default(),
                    },
                },
                Some(&plan),
            )
            .unwrap();
        let Some(rode::project::Response::Runtimes(tools)) = reply.tool else {
            panic!("Missing runtime versions")
        };
        assert_eq!(
            tools
                .iter()
                .find(|tool| tool.name == "node")
                .unwrap()
                .version,
            "from-mise"
        );
        assert!(remote.connected());
        assert!(!root.path().join("mise-installed").exists());
        drop(login);
        remote.cleanup();
    }
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn native_terminal_size_tracks_panes_over_ssh_and_proxyjump() {
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    for jump in ["", "rode-jump"] {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let remote = Remote::new(Profile {
            host: "rode-test".into(),
            config: config.clone(),
            jump: jump.into(),
            ..Default::default()
        })
        .unwrap();
        let login = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
        let until = Instant::now() + Duration::from_secs(8);
        while !remote.connected() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(remote.connected());
        let plan = rode::sandbox::Plan {
            root: root.to_string_lossy().into(),
            ..Default::default()
        };
        let response = remote
            .request(&Request::Tool {
                request: rode::project::Request::Sessions {
                    root: plan.root.clone(),
                    operation: rode::sessions::Operation::Create {
                        id: rode_session::new_id().unwrap(),
                        name: "Resize test".into(),
                        policy: rode::sessions::fingerprint(&plan),
                        environment: Box::new(plan.environment.clone()),
                        plan: Box::new(plan),
                        python: "off".into(),
                        persistent: false,
                        allocation: None,
                    },
                    directory: Some(base.path().join("sessions").to_string_lossy().into()),
                },
            })
            .unwrap();
        let Some(rode::project::Response::Sessions(reply)) = response.tool else {
            panic!("No terminal service reply")
        };
        let rode_session::Response::Session(info) = &reply.response else {
            panic!("No terminal session")
        };
        let mut terminal = native_attach(&remote, &reply, info);
        terminal_ui::check_sizes(&mut terminal, &root);
        drop(terminal);
        drop(login);
        remote.cleanup();
    }
}

//! Run through scripts/test-ssh.sh, which creates an isolated localhost SSH server.
use rode::{
    files::Request,
    ssh::{Profile, Remote},
    terminal::Terminal,
};
use std::{
    process::Command,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn openssh_mux_roundtrip_and_disconnect() {
    roundtrip("");
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn openssh_proxyjump_roundtrip_and_disconnect() {
    roundtrip("rode-jump");
}

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
fn interactive_slurm_shells_receive_scheduler_environment_over_ssh_and_proxyjump() {
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
        let end = Instant::now() + Duration::from_secs(10);
        while !remote.connected() {
            assert!(
                login.is_alive() && Instant::now() < end,
                "{}",
                login.contents()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        for sandbox in [false, true] {
            let root = base.path().join(if sandbox { "sandbox" } else { "normal" });
            std::fs::create_dir(&root).unwrap();
            let response = remote
                .request(&Request::Prepare {
                    path: root.to_string_lossy().into(),
                    sandbox: rode::sandbox::Options {
                        enabled: sandbox,
                        ..Default::default()
                    },
                })
                .unwrap();
            let plan = response.project.unwrap();
            if sandbox && !plan.active {
                eprintln!("Compute sandbox fixture unavailable: {}", plan.reason);
            }
            interactive_allocation_roundtrip(&remote, &plan);
        }
        drop(login);
        remote.cleanup();
    }
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn slurm_notebooks_execute_and_stop_over_ssh_and_proxyjump() {
    use rode::{
        notebook::kernel::{Event, Kernel, Run},
        slurm,
    };
    use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};
    let Ok(python) = std::env::var("RODE_TEST_JUPYTER") else {
        eprintln!("Set RODE_TEST_JUPYTER to exercise Slurm notebooks over SSH");
        return;
    };
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    for jump in ["", "rode-jump"] {
        let root = tempfile::tempdir().unwrap();
        let tools = root.path().join("tools");
        fs::create_dir(&tools).unwrap();
        for name in ["srun", "squeue"] {
            let path = tools.join(name);
            fs::write(&path, include_str!("fixtures/slurm.py")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let state = root.path().join("slurm-state");
        fs::create_dir(&state).unwrap();
        let record = state.join("job.json");
        fs::write(
            &record,
            r#"{"id":"4242","state":"RUNNING","name":"notebook"}"#,
        )
        .unwrap();
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
            std::thread::sleep(Duration::from_millis(25));
        }
        let response = remote
            .request(&Request::Prepare {
                path: root.path().to_string_lossy().into(),
                sandbox: Default::default(),
            })
            .unwrap();
        let target = slurm::Target {
            id: "4242".into(),
            name: "notebook".into(),
            queue: tools.join("squeue").to_string_lossy().into(),
            runner: tools.join("srun").to_string_lossy().into(),
        };
        let mut kernel = Kernel::start_on(
            response.project.unwrap(),
            Some(remote.clone()),
            python.clone(),
            "python3".into(),
            Some(target),
            Arc::new(|| {}),
        )
        .unwrap();
        kernel.run(vec![Run { cell: String::new(),
uid: 1, version: 0, code: "import os\nprint('SSH_NOTEBOOK', os.environ['SLURM_JOB_ID'], os.environ['CUDA_VISIBLE_DEVICES'])".into() }]).unwrap();
        let until = Instant::now() + Duration::from_secs(35);
        let mut output = String::new();
        loop {
            let mut done = false;
            for event in kernel.poll() {
                match event {
                    Event::Output { output: value, .. } => {
                        output.push_str(value["text"].as_str().unwrap_or(""))
                    }
                    Event::Done(_, _) => done = true,
                    Event::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
            if done {
                break;
            }
            assert!(Instant::now() < until, "{}", kernel.status);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(output.contains("SSH_NOTEBOOK 4242 0,1"), "{output}");
        fs::remove_file(&record).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if kernel
                .poll()
                .iter()
                .any(|event| matches!(event, Event::Error(_)))
            {
                break;
            }
            assert!(Instant::now() < until, "Kernel survived expired allocation");
            std::thread::sleep(Duration::from_millis(25));
        }
        drop(kernel);
        assert!(
            remote.connected(),
            "Kernel cleanup must preserve the workspace SSH master"
        );
        drop(login);
        remote.cleanup();
    }
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn persistent_slurm_runtimes_reattach_after_complete_ssh_disconnect() {
    use rode::{
        debugger::{
            self,
            protocol::{Client, Phase},
        },
        notebook::kernel::{Event, Kernel, Run},
        runtime,
    };
    use rode_runtime_core::service::Info;
    use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, sync::Arc};
    let Ok(python) = std::env::var("RODE_TEST_JUPYTER") else {
        eprintln!("Set RODE_TEST_JUPYTER for persistent Slurm runtime coverage");
        return;
    };
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    struct Cleanup(rode::sandbox::Plan, Vec<Info>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for info in &self.1 {
                let _ = runtime::request_with_upload(
                    &self.0,
                    None,
                    rode_session::Request::EndRuntime {
                        id: info.id.clone(),
                        policy: info.policy.clone(),
                    },
                    false,
                );
            }
        }
    }
    let connect = |jump: &str| {
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
        (remote, login)
    };
    let infos = |plan: &rode::sandbox::Plan, remote: &Remote| {
        let reply = runtime::request_with_upload(
            plan,
            Some(remote),
            rode_session::Request::Runtimes,
            false,
        )
        .unwrap();
        let rode_session::Response::Runtimes(infos) = reply.response else {
            panic!()
        };
        infos
    };
    for jump in ["", "rode-jump"] {
        let base = tempfile::tempdir().unwrap();
        let root = base.path();
        let tools = root.join("tools");
        fs::create_dir(&tools).unwrap();
        for name in ["srun", "squeue"] {
            let path = tools.join(name);
            fs::write(&path, include_str!("fixtures/slurm.py")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let state = root.join("slurm-state");
        fs::create_dir(&state).unwrap();
        fs::write(
            state.join("job.json"),
            r#"{"id":"4242","state":"RUNNING","name":"persistent"}"#,
        )
        .unwrap();
        let (remote, login) = connect(jump);
        let plan = remote
            .request(&Request::Prepare {
                path: root.to_string_lossy().into(),
                sandbox: Default::default(),
            })
            .unwrap()
            .project
            .unwrap();
        let mut cleanup = Cleanup(plan.clone(), vec![]);
        let target = rode::slurm::Target {
            id: "4242".into(),
            name: "persistent".into(),
            queue: tools.join("squeue").to_string_lossy().into(),
            runner: tools.join("srun").to_string_lossy().into(),
        };
        let done = root.join("queue-done");
        let first = root.join("queue-started");
        let runs = vec![
            Run {
                uid: 1,
                cell: "first".into(),
                version: 0,
                code: format!(
                    "import os,time\ncounter=41\nopen({:?},'w').write(os.environ['SLURM_JOB_ID'])\ntime.sleep(1)",
                    first.to_string_lossy()
                ),
            },
            Run {
                uid: 2,
                cell: "second".into(),
                version: 0,
                code: format!(
                    "counter+=1\nprint(counter)\nopen({:?},'w').write(str(counter))",
                    done.to_string_lossy()
                ),
            },
        ];
        let mut kernel = Kernel::start_persistent(
            plan.clone(),
            Some(remote.clone()),
            python.clone(),
            "python3".into(),
            Some(target.clone()),
            root.join("demo.ipynb").to_string_lossy().into(),
            Arc::new(|| {}),
        )
        .unwrap();
        kernel.bind(runs.clone());
        kernel.run(runs.clone()).unwrap();
        let until = Instant::now() + Duration::from_secs(35);
        while !first.exists() || kernel.pending() {
            for e in kernel.poll() {
                if let Event::Error(error) = e {
                    panic!("{error}");
                }
            }
            assert!(Instant::now() < until, "{}", kernel.status);
            std::thread::sleep(Duration::from_millis(10));
        }
        let kernel_info = infos(&plan, &remote)
            .into_iter()
            .find(|info| Some(info.id.as_str()) == kernel.native_id())
            .unwrap();
        cleanup.1.push(kernel_info.clone());
        drop(kernel);
        drop(login);
        remote.cleanup();
        let until = Instant::now() + Duration::from_secs(10);
        while !done.exists() {
            assert!(Instant::now() < until, "Detached Run All did not finish");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fs::read_to_string(&done).unwrap(), "42");
        let (remote, login) = connect(jump);
        let until = Instant::now() + Duration::from_secs(10);
        while infos(&plan, &remote)
            .iter()
            .any(|i| i.id == kernel_info.id && i.attached)
        {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut kernel = Kernel::attach(
            plan.clone(),
            Some(remote.clone()),
            kernel_info.clone(),
            Arc::new(|| {}),
        )
        .unwrap();
        let mut rebound = runs.clone();
        for run in &mut rebound {
            run.uid += 100;
            run.version = 3;
        }
        kernel.bind(rebound);
        let until = Instant::now() + Duration::from_secs(35);
        let mut restored = false;
        while !restored {
            for e in kernel.poll() {
                match e {
                    Event::Output { run, output, .. }
                        if run.uid == 102 && output["text"] == "42\n" =>
                    {
                        restored = true
                    }
                    Event::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
            assert!(Instant::now() < until, "No source-bound output replay");
            std::thread::sleep(Duration::from_millis(10));
        }
        let file = root.join("main.py");
        fs::write(&file, "answer=42\n").unwrap();
        let adapter = root.join("adapter.py");
        fs::write(&adapter, include_str!("fixtures/debug_adapter.py")).unwrap();
        let mut settings = debugger::Settings::default();
        settings.config.backend = debugger::Backend::Custom;
        settings.config.adapter = "python3".into();
        settings.config.adapter_arguments = vec![adapter.to_string_lossy().into()];
        let mut client = Client::start_managed(
            plan.clone(),
            Some(remote.clone()),
            file.to_string_lossy().into(),
            python.clone(),
            settings,
            Some(target),
            true,
            BTreeMap::new(),
            Arc::new(|| {}),
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(35);
        while client.phase != Phase::Paused || client.variables.is_empty() {
            client.poll();
            assert!(client.error.is_empty(), "{}", client.error);
            assert!(Instant::now() < until, "{}", client.status);
            std::thread::sleep(Duration::from_millis(10));
        }
        let epoch = client.inspection_epoch();
        let debug_info = infos(&plan, &remote)
            .into_iter()
            .find(|info| Some(info.id.as_str()) == client.native_id())
            .unwrap();
        cleanup.1.push(debug_info.clone());
        drop(client);
        drop(kernel);
        drop(login);
        remote.cleanup();
        let (remote, login) = connect(jump);
        let until = Instant::now() + Duration::from_secs(10);
        while infos(&plan, &remote)
            .iter()
            .any(|i| i.id == debug_info.id && i.attached)
        {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut client = Client::attach(
            plan.clone(),
            Some(remote.clone()),
            debug_info.clone(),
            Arc::new(|| {}),
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(35);
        while !client.attached_ready() || client.variables.is_empty() {
            client.poll();
            assert!(client.error.is_empty(), "{}", client.error);
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(client.inspection_epoch(), epoch);
        assert_eq!(client.phase, Phase::Paused);
        let mut duplicate = Client::attach(
            plan.clone(),
            Some(remote.clone()),
            debug_info,
            Arc::new(|| {}),
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while duplicate.error.is_empty() || duplicate.lifetime_pending() {
            duplicate.poll();
            assert!(
                Instant::now() < until,
                "Rejected attachment blocked window exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(duplicate.phase, Phase::Ended);
        assert!(duplicate.control("next").is_err());
        drop(duplicate);
        client.control("next").unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while client.inspection_epoch() == epoch || client.phase != Phase::Paused {
            client.poll();
            assert!(client.error.is_empty(), "{}", client.error);
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        client.stop();
        let until = Instant::now() + Duration::from_secs(12);
        while client.is_stopping() {
            client.poll();
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            infos(&plan, &remote)
                .iter()
                .any(|i| i.id == kernel_info.id && i.alive),
            "Debugger shutdown ended the notebook kernel"
        );
        assert!(remote.connected());
        runtime::request_with_upload(
            &plan,
            Some(&remote),
            rode_session::Request::EndRuntime {
                id: kernel_info.id,
                policy: kernel_info.policy,
            },
            false,
        )
        .unwrap();
        drop(client);
        drop(login);
        remote.cleanup();
    }
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn background_agents_survive_ssh_disconnect_and_keep_their_sandbox() {
    use rode_agent_core::{
        queue::Action as Queue,
        service::{Command, Spec, State, Update},
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
            std::fs::write(&executable, include_str!("fixtures/persistent_agent.py")).unwrap();
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
            let rpc = |remote: &Remote, request, upload| {
                let response = remote
                    .request(&Request::Tool {
                        request: rode::project::Request::Sessions {
                            root: plan.root.clone(),
                            operation: rode::sessions::Operation::Agents {
                                request: Box::new(request),
                            },
                            helper: Some(native_test_helper(upload)),
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
                env: BTreeMap::new(),
                python: "off".into(),
                persistent: true,
                text: Some("work".into()),
                attachments: vec![],
            };
            let reply = rpc(
                &remote,
                rode_session::Request::CreateAgent {
                    spec: Box::new(spec),
                },
                true,
            );
            let rode_session::Response::Agent(info) = &reply.response else {
                panic!()
            };
            let info = info.clone();
            let attach = |remote: &Remote| {
                let mut control = rode::sandbox::Plan::default();
                control.root = plan.root.clone();
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
            assert!(info.automatic_queue);
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
            let second = attach(&next);
            let until = Instant::now() + Duration::from_secs(12);
            state = None;
            while state
                .as_ref()
                .is_none_or(|s| s.chat.completed != 3 || s.chat.busy || !s.queue.items.is_empty())
            {
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
            rpc(
                &next,
                rode_session::Request::EndAgent {
                    id: info.id.clone(),
                    policy: policy.clone(),
                },
                false,
            );
            let until = Instant::now() + Duration::from_secs(8);
            loop {
                let rode_session::Response::Agents(infos) =
                    rpc(&next, rode_session::Request::Agents, false).response
                else {
                    panic!()
                };
                if !infos.iter().any(|i| i.id == info.id) {
                    break;
                }
                assert!(Instant::now() < until, "Remote agent did not stop");
                std::thread::sleep(Duration::from_millis(50));
            }
            drop(second);
            drop(login);
            next.cleanup();
        }
    }
}

fn roundtrip(jump: &str) {
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    let root = tempfile::tempdir().unwrap();
    let path = root
        .path()
        .join("remote 'quoted' $file 🦀.rs")
        .to_str()
        .unwrap()
        .to_string();
    let remote = Remote::new(Profile {
        host: "rode-test".into(),
        config,
        jump: jump.into(),
        root: root.path().to_str().unwrap().into(),
        ..Default::default()
    })
    .unwrap();
    let terminal = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
    let start = Instant::now();
    while !remote.connected() && terminal.is_alive() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(remote.connected(), "{}", terminal.contents());
    // Separate RODE instances use separate masters even for the same SSH alias.
    let other = Remote::new(remote.profile.clone()).unwrap();
    assert_ne!(remote.master_args(), other.master_args());
    let other_terminal = Terminal::remote(&eframe::egui::Context::default(), &other).unwrap();
    let start = Instant::now();
    while !other.connected()
        && other_terminal.is_alive()
        && start.elapsed() < Duration::from_secs(10)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(other.connected(), "{}", other_terminal.contents());
    drop(other_terminal);
    other.cleanup();
    assert!(
        remote.connected(),
        "Closing another instance's SSH master disconnected this one"
    );
    let write = remote
        .request(&Request::Write {
            path: path.clone(),
            text: "fn main() { println!(\"hello remote host 🦀\"); }\n".into(),
            expected: None,
        })
        .unwrap();
    let read = remote
        .request(&Request::Read { path: path.clone() })
        .unwrap();
    assert_eq!(write.revision, read.revision);
    assert!(read.text.contains("hello remote host 🦀"));
    // A multi-megabyte save exercises simultaneous pipe draining; it must not deadlock.
    let large = "αβγ Rust remote host\n".repeat(100_000);
    remote
        .request(&Request::Write {
            path: path.clone(),
            text: large.clone(),
            expected: Some(read.revision),
        })
        .unwrap();
    assert_eq!(
        remote
            .request(&Request::Read { path: path.clone() })
            .unwrap()
            .text,
        large
    );
    assert!(
        remote
            .request(&Request::Write {
                path: path.clone(),
                text: "stale".into(),
                expected: Some(write.revision)
            })
            .unwrap_err()
            .to_string()
            .contains("conflict")
    );
    let list = remote
        .request(&Request::List {
            path: root.path().to_str().unwrap().into(),
        })
        .unwrap();
    assert!(list.entries.iter().any(|e| e.path == path));
    // Open two project folders through the same authenticated connection. A
    // non-root test sshd uses non-TTY channels; the local PTY remains real.
    let second = root.path().join("project 'two' 🦀");
    std::fs::create_dir(&second).unwrap();
    for (index, directory) in [root.path(), second.as_path()].iter().enumerate() {
        let response = remote
            .request(&Request::Prepare {
                path: directory.to_str().unwrap().into(),
                sandbox: Default::default(),
            })
            .unwrap();
        assert_eq!(response.path, directory.to_str().unwrap());
        let plan = response.project.unwrap();
        let mut command = portable_pty::CommandBuilder::new("ssh");
        command.args(remote.channel_args(false));
        command.arg(remote.shell_command(&plan));
        let shell = Terminal::spawn(
            &eframe::egui::Context::default(),
            command,
            "project shell".into(),
        )
        .unwrap();
        let marker = directory.join("terminal-marker.txt");
        shell
            .send(format!("printf 'project-{index}' > terminal-marker.txt\r").into_bytes())
            .unwrap();
        let start = Instant::now();
        while !marker.exists() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(marker).unwrap(),
            format!("project-{index}"),
            "{}",
            shell.contents()
        );
        drop(shell);
        assert!(
            remote.connected(),
            "Closing a project shell must not disconnect SSH"
        );
    }
    let response = remote
        .request(&Request::Prepare {
            path: second.to_str().unwrap().into(),
            sandbox: rode::sandbox::Options {
                enabled: true,
                ..Default::default()
            },
        })
        .unwrap();
    let plan = response.project.unwrap();
    let project_root = second.to_str().unwrap().to_owned();
    let image_path = second.join("image 'quoted' 🦀.png");
    let image_bytes = include_bytes!("../assets/carbon.png");
    std::fs::write(&image_path, image_bytes).unwrap();
    let response = remote
        .request_in(
            &Request::ReadImage {
                path: image_path.to_string_lossy().into(),
            },
            Some(&plan),
        )
        .unwrap();
    let decoded = rode::images::decode(
        response.image.as_deref().unwrap(),
        image_path.to_str().unwrap(),
        &std::sync::Arc::new(rode::images::Budget::default()),
        4096,
    )
    .unwrap();
    assert_eq!(decoded.format, "PNG");
    assert_eq!(response.revision, rode::files::digest(image_bytes));
    if plan.active {
        assert!(
            remote
                .request_in(&Request::ReadImage { path: path.clone() }, Some(&plan))
                .is_err(),
            "Image transfers must obey the same sandbox grants as text reads"
        );
    }
    let git = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::Git {
                    root: project_root.clone(),
                    args: vec!["init".into()],
                    input: None,
                },
            },
            Some(&plan),
        )
        .unwrap()
        .tool
        .unwrap();
    assert!(matches!(git,rode::project::Response::Command(output) if output.success()));
    let run_git = |args: &[&str]| {
        let response = remote
            .request_in(
                &Request::Tool {
                    request: rode::project::Request::Git {
                        root: project_root.clone(),
                        args: args.iter().map(|s| (*s).into()).collect(),
                        input: None,
                    },
                },
                Some(&plan),
            )
            .unwrap()
            .tool
            .unwrap();
        let rode::project::Response::Command(output) = response else {
            panic!("Missing Git reply")
        };
        output.checked().unwrap()
    };
    let stash_file = second.join("stash-test.txt");
    std::fs::write(&stash_file, "baseline\n").unwrap();
    run_git(&["add", "--", "stash-test.txt"]);
    run_git(&[
        "-c",
        "user.name=RODE Test",
        "-c",
        "user.email=rode@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-m",
        "Stash baseline",
    ]);
    let stash = |value: &str| {
        std::fs::write(&stash_file, value).unwrap();
        run_git(&[
            "-c",
            "user.name=RODE Test",
            "-c",
            "user.email=rode@example.invalid",
            "stash",
            "push",
            "-m",
            value.trim(),
            "--",
            "stash-test.txt",
        ]);
        run_git(&["rev-parse", "refs/stash"]).trim().to_owned()
    };
    let selected = stash("selected A\n");
    let newer = stash("newer B\n");
    let drop_stash = |reference: &str| {
        remote.request_in(
            &Request::Tool {
                request: rode::project::Request::GitStash {
                    root: project_root.clone(),
                    action: rode::project::StashAction::Drop,
                    reference: reference.into(),
                    oid: selected.clone(),
                },
            },
            Some(&plan),
        )
    };
    assert!(
        drop_stash("stash@{0}")
            .unwrap_err()
            .to_string()
            .contains("Stash list changed")
    );
    let reply = drop_stash("stash@{1}").unwrap().tool.unwrap();
    assert!(matches!(reply, rode::project::Response::Command(output) if output.success()));
    assert_eq!(run_git(&["stash", "list", "--format=%H"]).trim(), newer);
    let environment = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::Environments {
                    root: project_root.clone(),
                    selected: String::new(),
                },
            },
            Some(&plan),
        )
        .unwrap()
        .tool
        .unwrap();
    assert!(
        matches!(environment,rode::project::Response::Environments(env) if !env.version.is_empty())
    );
    let monitor = remote
        .request(&Request::Tool {
            request: rode::project::Request::Monitor {
                root: project_root.clone(),
            },
        })
        .unwrap()
        .tool
        .unwrap();
    assert!(matches!(monitor,rode::project::Response::Monitor(sample) if !sample.cpus.is_empty()));
    if let Ok(ruff) = std::env::var("RODE_TEST_RUFF") {
        let copied = second.join("ruff-test");
        std::fs::copy(ruff, &copied).unwrap();
        let analysis = remote
            .request_in(
                &Request::Tool {
                    request: rode::project::Request::Analyze {
                        root: project_root.clone(),
                        path: second.join("main.py").to_str().unwrap().into(),
                        text: "import os\nvalue= [1,2]\n".into(),
                        python: String::new(),
                        ruff: copied.to_str().unwrap().into(),
                        format: true,
                        check: true,
                        fix: false,
                    },
                },
                Some(&plan),
            )
            .unwrap()
            .tool
            .unwrap();
        assert!(
            matches!(analysis,rode::project::Response::Analysis(result) if result.text.contains("value = [1, 2]") && result.diagnostics.iter().any(|d|d.code == "F401"))
        );
    }
    // New language tool routing uses the same authenticated host/sandbox.
    let tools = second.join("node_modules/.bin");
    std::fs::create_dir_all(&tools).unwrap();
    let formatter = tools.join("prettier");
    std::fs::write(&formatter, "#!/usr/bin/python3\nimport sys\nsource=sys.stdin.read()\nassert '--stdin-filepath' in sys.argv\nif 'broken' in source:\n print('[error] main.ts: SyntaxError (2:3)',file=sys.stderr);sys.exit(2)\nprint(source.replace('x=1','x = 1'),end='')\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&formatter, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for source in ["const x=1;\n", "// comment\nbroken\n"] {
        let response = remote
            .request_in(
                &Request::Tool {
                    request: rode::project::Request::Analyze {
                        root: project_root.clone(),
                        path: second.join("main.ts").to_string_lossy().into(),
                        text: source.into(),
                        python: String::new(),
                        ruff: String::new(),
                        format: true,
                        check: true,
                        fix: false,
                    },
                },
                Some(&plan),
            )
            .unwrap();
        let Some(rode::project::Response::Analysis(result)) = response.tool else {
            panic!()
        };
        if source.contains("broken") {
            assert_eq!(result.text, source);
            assert_eq!(result.diagnostics[0].row, 2);
        } else {
            assert_eq!(result.text, "const x = 1;\n");
        }
        assert!(!second.join("main.ts").exists());
    }
    let pdf_bytes = include_bytes!("fixtures/pages.pdf");
    let pdf_path = second.join("quoted ' page 🦀.PDF");
    std::fs::write(&pdf_path, pdf_bytes).unwrap();
    let private_pdf = root.path().join("outside.pdf");
    std::fs::write(&private_pdf, pdf_bytes).unwrap();
    let escape_pdf = second.join("escape.pdf");
    std::os::unix::fs::symlink(&private_pdf, &escape_pdf).unwrap();
    if plan.active {
        for path in [&private_pdf, &escape_pdf] {
            assert!(
                remote
                    .request_in(
                        &Request::ReadBinary {
                            path: path.to_string_lossy().into()
                        },
                        Some(&plan)
                    )
                    .is_err()
            );
        }
    }
    let binary = remote
        .request_in(
            &Request::ReadBinary {
                path: pdf_path.to_string_lossy().into(),
            },
            Some(&plan),
        )
        .unwrap();
    {
        use base64::{Engine, engine::general_purpose::STANDARD};
        assert_eq!(STANDARD.decode(binary.binary.unwrap()).unwrap(), pdf_bytes);
    }
    let inside = second.join("sandbox.txt").to_str().unwrap().to_owned();
    remote
        .request_in(
            &Request::Write {
                path: inside.clone(),
                text: "remote sandbox".into(),
                expected: None,
            },
            Some(&plan),
        )
        .unwrap();
    assert_eq!(
        remote
            .request_in(&Request::Read { path: inside }, Some(&plan))
            .unwrap()
            .text,
        "remote sandbox"
    );
    if plan.active {
        assert!(
            remote
                .request_in(&Request::Read { path: path.clone() }, Some(&plan))
                .is_err(),
            "Parent project files must not be exposed to the sandbox of its child project"
        );
    } else {
        assert!(plan.requested && !plan.reason.is_empty());
        eprintln!("Remote sandbox unavailable: {}", plan.reason);
    }
    debugger_roundtrip(&remote, &plan);
    native_debugger_roundtrip(&remote, &plan.root);
    bundled_runtime_roundtrip(&remote, root.path());
    notebook_roundtrip(&remote, &plan.root, &path);
    marp_roundtrip(&remote, &plan);
    latex_roundtrip(&remote, &plan, &path);
    encrypted_roundtrip(&remote, root.path());
    binary_extension_roundtrip(&remote, &plan, &path);
    agent_roundtrip(&remote, &plan);
    native_upgrade_roundtrip(&remote, &plan);
    interactive_allocation_roundtrip(&remote, &plan);
    let (native_info, native_reply, native_runtime) = native_terminal_start(&remote, &plan);
    drop(terminal);
    let start = Instant::now();
    while remote.connected() && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        remote.request(&Request::Read { path }).is_err(),
        "File operations must fail after disconnect instead of opening another SSH login"
    );
    remote.cleanup();
    // Reconnect with a new editor-owned SSH master: the native PTY owner must
    // survive destruction of the first connection and all of its channels.
    let next = Remote::new(remote.profile.clone()).unwrap();
    let login = Terminal::remote(&eframe::egui::Context::default(), &next).unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    while !next.connected() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(next.connected());
    let native = native_attach(&next, &native_reply, &native_info);
    let until = Instant::now() + Duration::from_secs(8);
    while !native.contents().contains("BACKGROUND_DONE") && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        native.contents().contains("BACKGROUND_DONE"),
        "{}",
        native.contents()
    );
    let changed = next
        .request(&Request::Tool {
            request: rode::project::Request::Sessions {
                root: plan.root.clone(),
                operation: rode::sessions::Operation::SetPersistence {
                    id: native_info.id.clone(),
                    policy: native_info.policy.clone(),
                    persistent: false,
                },
                helper: Some(native_test_helper(false)),
                directory: Some(native_runtime.clone()),
            },
        })
        .unwrap();
    assert!(matches!(
        changed.tool,
        Some(rode::project::Response::Sessions(_))
    ));
    let until = Instant::now() + Duration::from_secs(5);
    while native.persistent() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !native.persistent(),
        "SSH persistence update did not reach the attached view"
    );
    drop(native);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let response = next
            .request(&Request::Tool {
                request: rode::project::Request::Sessions {
                    root: plan.root.clone(),
                    operation: rode::sessions::Operation::List,
                    helper: Some(native_test_helper(false)),
                    directory: Some(native_runtime.clone()),
                },
            })
            .unwrap();
        if let Some(rode::project::Response::Sessions(reply)) = response.tool
            && let rode_session::Response::Sessions(infos) = reply.response
            && !infos.iter().any(|info| info.id == native_info.id)
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "SSH nonpersistent terminal survived its last view"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(login);
    next.cleanup();
    let until = Instant::now() + Duration::from_secs(3);
    while std::path::Path::new(&native_runtime)
        .join("socket")
        .exists()
        && Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn native_test_helper(data: bool) -> rode::sessions::Helper {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let path = std::env::var_os("RODE_TEST_SESSION_HELPER")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/release/rode-sessiond-static")
        });
    let bytes = std::fs::read(path).expect("Run scripts/build-sessiond.sh before the SSH test");
    rode::sessions::Helper {
        digest: rode::files::digest(&bytes),
        architecture: std::env::consts::ARCH.into(),
        data: data.then(|| STANDARD.encode(bytes)),
    }
}

fn bundled_runtime_roundtrip(remote: &Remote, root: &std::path::Path) {
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
    let call = |spec: &Value, bytes: Vec<u8>| {
        let mut command = Command::new("ssh");
        command.args(remote.channel_args(false)).arg(format!(
            "RODE_CODEX_CACHE={} python3 -c {} {}",
            rode::ssh::quote(cache.to_str().unwrap()),
            rode::ssh::quote(include_str!("../src/remote_codex.py")),
            rode::ssh::quote(&spec.to_string())
        ));
        let text = rode::project::run(command, bytes, Duration::from_secs(10))
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

fn actual_bundled_codex_roundtrip(remote: &Remote, root: &std::path::Path) {
    use rode::agent::{Config, Event, Launch, Provider, Session};
    use serde_json::{Value, json};
    use std::sync::Arc;
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

fn marp_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    use rode::marp::{Destination, Format, Invocation, Settings};
    use std::{
        os::unix::fs::PermissionsExt,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let root = std::path::Path::new(&plan.root);
    let program = root.join("marp.py");
    std::fs::write(&program, include_bytes!("fixtures/marp.py")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.join("pages.pdf"), include_bytes!("fixtures/pages.pdf")).unwrap();
    let source = root.join("deck.marp.md");
    std::fs::write(&source, "saved source").unwrap();
    let mut request = Invocation {
        root: plan.root.clone(),
        path: source.to_string_lossy().into(),
        text: "---\nmarp: true\n---\n# SSH unsaved slide\n".into(),
        settings: Settings {
            program: program.to_string_lossy().into(),
            ..Default::default()
        },
        format: Format::Pdf,
        destination: None,
    };
    let reply = remote
        .marp_request(&request, Some(plan), &AtomicBool::new(false))
        .unwrap();
    let loaded = rode::pdf::load(
        reply.data.as_deref().unwrap(),
        1,
        &Arc::new(rode::images::Budget::default()),
        128,
    )
    .unwrap();
    assert_eq!(loaded.source.pages, 2);
    request.format = Format::Html;
    request.destination = Some(Destination {
        path: root.join("slides.html").to_string_lossy().into(),
        overwrite: false,
    });
    remote
        .marp_request(&request, Some(plan), &AtomicBool::new(false))
        .unwrap();
    assert!(
        std::fs::read_to_string(root.join("slides.html"))
            .unwrap()
            .contains("SSH unsaved slide")
    );
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "saved source");
    assert!(
        remote
            .marp_request(&request, Some(plan), &AtomicBool::new(false))
            .is_err()
    );
    request.destination = None;
    request.text = "RODE_TEST_SLEEP".into();
    let _ = std::fs::remove_file(root.join("marp-pids.json"));
    let cancel = Arc::new(AtomicBool::new(false));
    let control = cancel.clone();
    let connection = remote.clone();
    let policy = plan.clone();
    let worker =
        std::thread::spawn(move || connection.marp_request(&request, Some(&policy), &cancel));
    let deadline = Instant::now() + Duration::from_secs(8);
    while !root.join("marp-pids.json").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    control.store(true, Ordering::Release);
    assert!(worker.join().unwrap().is_err());
    assert!(
        remote.connected(),
        "Cancelling Marp must preserve the authenticated SSH connection"
    );
}
fn latex_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan, outside: &str) {
    use rode::latex::{self, Settings};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let root = std::path::Path::new(&plan.root);
    let tool = root.join("latex-fixture");
    fs::write(&tool, include_bytes!("fixtures/latex.py")).unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(root.join("pages.pdf"), include_bytes!("fixtures/pages.pdf")).unwrap();
    let source = root.join("latex-main.tex");
    let text = if plan.active {
        format!("ASSERT_HIDDEN={outside}\n")
    } else {
        "normal mode\n".into()
    };
    fs::write(&source, &text).unwrap();
    let settings = Settings {
        program: tool.to_string_lossy().into(),
        formatter: tool.to_string_lossy().into(),
        checker: tool.to_string_lossy().into(),
        arguments: vec!["{file}".into(), "{outdir}".into()],
        output_directory: "latex-output".into(),
        ..Default::default()
    };
    let invocation = latex::Invocation {
        root: plan.root.clone(),
        path: source.to_string_lossy().into(),
        settings: settings.clone(),
    };
    let result = remote
        .latex_request(&invocation, plan, &AtomicBool::new(false))
        .unwrap();
    assert!(result.output.success(), "{}", result.output.stderr);
    let response = remote
        .request_in(
            &Request::ReadBinary {
                path: result.pdf.unwrap(),
            },
            Some(plan),
        )
        .unwrap();
    {
        use base64::{Engine, engine::general_purpose::STANDARD};
        assert_eq!(
            STANDARD.decode(response.binary.unwrap()).unwrap(),
            include_bytes!("fixtures/pages.pdf")
        );
    }
    let analysis = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::LatexAnalyze {
                    root: plan.root.clone(),
                    path: source.to_string_lossy().into(),
                    text: "UNFORMATTED\n🦀 ISSUE\n".into(),
                    settings,
                    format: true,
                    check: true,
                },
            },
            Some(plan),
        )
        .unwrap();
    let Some(rode::project::Response::Analysis(analysis)) = analysis.tool else {
        panic!()
    };
    assert_eq!(analysis.text, "formatted\n🦀 ISSUE\n");
    assert_eq!(analysis.diagnostics[0].column, 3);
    assert_eq!(fs::read_to_string(&source).unwrap(), text);
    fs::write(&source, "ERROR").unwrap();
    let result = remote
        .latex_request(&invocation, plan, &AtomicBool::new(false))
        .unwrap();
    assert!(!result.output.success() && result.pdf.is_none());
    assert_eq!(result.diagnostics[0].diagnostic.row, 2);
    fs::write(&source, "SLOW").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let connection = remote.clone();
    let policy = plan.clone();
    let worker = std::thread::spawn(move || connection.latex_request(&invocation, &policy, &flag));
    let until = Instant::now() + Duration::from_secs(8);
    while !root.join("latex-output/pids.json").exists() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    cancel.store(true, Ordering::Release);
    assert!(worker.join().unwrap().is_err());
    assert!(
        remote.connected(),
        "Cancelling LaTeX must preserve the authenticated SSH connection"
    );
    assert!(
        remote
            .request_in(
                &Request::Read {
                    path: source.to_string_lossy().into()
                },
                Some(plan)
            )
            .is_ok()
    );
}

fn native_attach(
    remote: &Remote,
    reply: &rode::sessions::Reply,
    info: &rode_session::Info,
) -> Terminal {
    let mut command = portable_pty::CommandBuilder::new("ssh");
    command.args(remote.channel_args(false));
    let mut argv = vec![reply.executable.clone()];
    argv.extend(reply.prefix.clone());
    argv.extend([
        "attach".into(),
        "--framed".into(),
        info.id.clone(),
        info.policy.clone(),
    ]);
    command.arg(
        argv.iter()
            .map(|arg| rode::ssh::quote(arg))
            .collect::<Vec<_>>()
            .join(" "),
    );
    Terminal::spawn_native(
        &eframe::egui::Context::default(),
        command,
        "Native SSH test".into(),
    )
    .unwrap()
}
fn interactive_allocation_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};
    let root = Path::new(&plan.root);
    let tools = root.join("slurm-tools");
    fs::create_dir_all(&tools).unwrap();
    for name in ["salloc", "srun", "squeue", "scancel"] {
        let path = tools.join(name);
        fs::write(&path, include_str!("fixtures/slurm.py")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let state = root.join("slurm-state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("grant"), "").unwrap();
    let id = rode_session::new_id().unwrap();
    let settings = rode::slurm::Allocation {
        allocator: tools.join("salloc").to_string_lossy().into(),
        runner: tools.join("srun").to_string_lossy().into(),
        ..Default::default()
    };
    let runtime = root.parent().unwrap().join("slurm-runtime");
    let response = remote
        .request(&Request::Tool {
            request: rode::project::Request::Sessions {
                root: plan.root.clone(),
                operation: rode::sessions::Operation::Create {
                    environment: Default::default(),
                    id: id.clone(),
                    name: "SSH allocation".into(),
                    plan: Box::new(plan.clone()),
                    policy: rode::sessions::fingerprint(plan),
                    python: "off".into(),
                    persistent: false,
                    allocation: Some(settings.launch(plan, &id).unwrap()),
                },
                helper: Some(native_test_helper(true)),
                directory: Some(runtime.to_string_lossy().into()),
            },
        })
        .unwrap();
    let Some(rode::project::Response::Sessions(reply)) = response.tool else {
        panic!()
    };
    let rode_session::Response::Session(info) = &reply.response else {
        panic!()
    };
    assert_eq!(info.allocation, Some(format!("rode-{id}")));
    let view = native_attach(remote, &reply, info);
    view.send(b"printf 'SSH_SLURM_%s_%s_%s\\n' \"$SLURM_JOB_ID\" \"$SLURM_STEP_ID\" \"$CUDA_VISIBLE_DEVICES\"\n".to_vec()).unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    while !view.contents().contains("SSH_SLURM_4242_0_0,1") {
        assert!(
            view.is_alive() && Instant::now() < end,
            "{}",
            view.contents()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    view.send(b"exit\n".to_vec()).unwrap();
    while view.is_alive() {
        assert!(Instant::now() < end, "SSH allocation did not exit");
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(state.join("ended").exists());
    assert!(remote.connected());
}

fn native_upgrade_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    fn wait_output(terminal: &Terminal, token: &str, runtime: &std::path::Path) {
        let until = Instant::now() + Duration::from_secs(8);
        while !terminal.contents().contains(token) {
            assert!(
                terminal.is_alive() && Instant::now() < until,
                "Missing {token}: {}
Inputs: {:?}",
                terminal.contents(),
                std::fs::read_to_string(runtime.join("input"))
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    let runtime = std::path::Path::new(&plan.root)
        .parent()
        .unwrap()
        .join("legacy-runtime");
    let id = rode_session::new_id().unwrap();
    let policy = rode::sessions::fingerprint(plan);
    let mut legacy = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/session_v1.py"
        ))
        .arg(&runtime)
        .arg(&plan.root)
        .arg(&id)
        .arg(&policy)
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
    let wait = |label: &str, check: &dyn Fn() -> bool| {
        let until = Instant::now() + Duration::from_secs(8);
        while !check() {
            assert!(
                Instant::now() < until,
                "SSH terminal upgrade timed out: {label}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    wait("legacy startup", &|| runtime.join("socket").exists());
    let request = |operation| {
        remote.request(&Request::Tool {
            request: rode::project::Request::Sessions {
                root: plan.root.clone(),
                operation,
                helper: Some(native_test_helper(true)),
                directory: Some(runtime.to_string_lossy().into()),
            },
        })
    };
    {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let stale = b"#!/bin/sh\nexit 2\n";
        let error = remote
            .request(&Request::Tool {
                request: rode::project::Request::Sessions {
                    root: plan.root.clone(),
                    operation: rode::sessions::Operation::List,
                    helper: Some(rode::sessions::Helper {
                        digest: rode::files::digest(stale),
                        architecture: std::env::consts::ARCH.into(),
                        data: Some(STANDARD.encode(stale)),
                    }),
                    directory: Some(runtime.to_string_lossy().into()),
                },
            })
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("older protocol") && error.contains("build-sessiond.sh"),
            "{error}"
        );
    }
    let response = request(rode::sessions::Operation::List).unwrap();
    let Some(rode::project::Response::Sessions(reply)) = response.tool else {
        panic!()
    };
    let rode_session::Response::Sessions(infos) = &reply.response else {
        panic!()
    };
    let old = infos[0].clone();
    assert!(old.persistent && !old.persistence_supported);
    let old_view = native_attach(remote, &reply, &old);
    old_view
        .send(b"TOKEN=ssh_kept; printf 'LEGACY_%s\\n' READY\n".to_vec())
        .unwrap();
    wait_output(&old_view, "LEGACY_READY", &runtime);
    drop(old_view);
    let response = request(rode::sessions::Operation::Create {
        environment: Default::default(),
        allocation: None,
        id: rode_session::new_id().unwrap(),
        name: "Updated terminal".into(),
        plan: Box::new(plan.clone()),
        policy: policy.clone(),
        python: "off".into(),
        persistent: false,
    })
    .unwrap();
    let Some(rode::project::Response::Sessions(reply)) = response.tool else {
        panic!()
    };
    let rode_session::Response::Session(info) = &reply.response else {
        panic!()
    };
    assert!(info.persistence_supported && !info.persistent);
    let current = native_attach(remote, &reply, info);
    current
        .send(b"printf 'UPDATED_%s\\n' READY\n".to_vec())
        .unwrap();
    wait_output(&current, "UPDATED_READY", &runtime);
    let error = request(rode::sessions::Operation::SetPersistence {
        id: id.clone(),
        policy: policy.clone(),
        persistent: false,
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("Start a new terminal"), "{error}");
    assert!(!error.contains("unknown variant"), "{error}");
    let old_view = native_attach(remote, &reply, &old);
    old_view
        .send(b"printf 'TOKEN_%s\\n' \"$TOKEN\"\n".to_vec())
        .unwrap();
    wait_output(&old_view, "TOKEN_ssh_kept", &runtime);
    assert!(
        !std::fs::read_to_string(runtime.join("input"))
            .unwrap()
            .contains("\\x04"),
        "Detaching a native view must not forward portable-pty's EOF sentinel to the shell"
    );
    let response = request(rode::sessions::Operation::List).unwrap();
    assert!(
        matches!(response.tool, Some(rode::project::Response::Sessions(rode::sessions::Reply { response: rode_session::Response::Sessions(infos), .. })) if infos.len() == 2)
    );
    drop(old_view);
    request(rode::sessions::Operation::End { id, policy }).unwrap();
    wait("legacy cleanup", &|| !runtime.join("socket").exists());
    current
        .send(b"printf 'UPDATED_%s\\n' SURVIVED\n".to_vec())
        .unwrap();
    wait_output(&current, "UPDATED_SURVIVED", &runtime);
    request(rode::sessions::Operation::SetPersistence {
        id: info.id.clone(),
        policy: info.policy.clone(),
        persistent: true,
    })
    .unwrap();
    wait("persistence update", &|| current.persistent());
    current.send(b"exit\n".to_vec()).unwrap();
    wait("natural shell exit", &|| !current.is_alive());
    // The exited session must disappear before the service's idle timeout,
    // while this already-open view retains its final output.
    let response = request(rode::sessions::Operation::List).unwrap();
    assert!(
        matches!(response.tool, Some(rode::project::Response::Sessions(rode::sessions::Reply { response: rode_session::Response::Sessions(infos), .. })) if infos.is_empty())
    );
    assert!(current.contents().contains("UPDATED_SURVIVED"));
    wait("new cleanup", &|| {
        !runtime
            .join(format!("protocol-{}/socket", rode_session::PROTOCOL))
            .exists()
    });
    let response = request(rode::sessions::Operation::List).unwrap();
    assert!(
        matches!(response.tool, Some(rode::project::Response::Sessions(rode::sessions::Reply { response: rode_session::Response::Sessions(infos), .. })) if infos.is_empty())
    );
}

fn native_terminal_start(
    remote: &Remote,
    plan: &rode::sandbox::Plan,
) -> (rode_session::Info, rode::sessions::Reply, String) {
    let env_response = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::CreateEnvironment {
                    root: plan.root.clone(),
                    path: ".venv".into(),
                    python: "python3".into(),
                    with_pip: false,
                },
            },
            Some(plan),
        )
        .unwrap();
    let Some(rode::project::Response::Command(output)) = env_response.tool else {
        panic!()
    };
    assert!(output.success(), "{}", output.stderr);
    let runtime = std::path::Path::new(&plan.root)
        .parent()
        .unwrap()
        .join("native-runtime")
        .to_string_lossy()
        .into_owned();
    let response = remote
        .request(&Request::Tool {
            request: rode::project::Request::Sessions {
                root: plan.root.clone(),
                operation: rode::sessions::Operation::Create {
                    environment: Default::default(),
                    allocation: None,
                    persistent: true,
                    id: rode_session::new_id().unwrap(),
                    name: "Persistent SSH test".into(),
                    plan: Box::new(plan.clone()),
                    policy: rode::sessions::fingerprint(plan),
                    python: "auto".into(),
                },
                helper: Some(native_test_helper(true)),
                directory: Some(runtime.clone()),
            },
        })
        .unwrap();
    let Some(rode::project::Response::Sessions(reply)) = response.tool else {
        panic!()
    };
    let rode_session::Response::Session(info) = reply.response.clone() else {
        panic!()
    };
    let native = native_attach(remote, &reply, &info);
    native
        .send(
            b"python -c 'import sys;open(\"native-venv-prefix\",\"w\").write(sys.prefix)'\n"
                .to_vec(),
        )
        .unwrap();
    let prefix_file = std::path::Path::new(&plan.root).join("native-venv-prefix");
    let expected = format!("{}/.venv", plan.root);
    let deadline = Instant::now() + Duration::from_secs(8);
    while std::fs::read_to_string(&prefix_file).ok().as_deref() != Some(&expected)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        std::fs::read_to_string(prefix_file).unwrap(),
        expected,
        "{}",
        native.contents()
    );
    native
        .send(
            b"printf 'BACKGROUND_%s\\n' STARTED; sleep 2; printf 'BACKGROUND_%s\\n' DONE\n"
                .to_vec(),
        )
        .unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    while !native.contents().contains("BACKGROUND_STARTED") && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        native.contents().contains("BACKGROUND_STARTED"),
        "{}",
        native.contents()
    );
    drop(native);
    (info, reply, runtime)
}

fn agent_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    use rode::agent::{Access, Approval, Config, Launch, Provider, protocol::Chat};
    use std::{os::unix::fs::PermissionsExt, sync::Arc};
    let root = std::path::Path::new(&plan.root);
    let program = root.join("agent 'quoted'.py");
    std::fs::write(&program, include_str!("fixtures/agent.py")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.parent().unwrap().join("private.txt"), "hidden").unwrap();
    for provider in [Provider::Codex, Provider::Acp] {
        let deleted = root.join("history-deleted.json");
        if deleted.exists() {
            std::fs::remove_file(&deleted).unwrap();
        }
        let mut launch = Launch {
            config: Config {
                provider,
                codex: program.to_string_lossy().into(),
                acp_program: program.to_string_lossy().into(),
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
        let field = if provider == Provider::Acp {
            "prompt"
        } else {
            "input"
        };
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
        if provider == Provider::Acp {
            launch
                .config
                .acp_options
                .insert("model".into(), serde_json::json!("fixture-deep"));
            launch
                .config
                .acp_options
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
        if provider == Provider::Acp {
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
        let mut browser =
            rode::agent::history::Browser::open_service(launch.clone(), Arc::new(|| {})).unwrap();
        let wait_history = |browser: &mut rode::agent::history::Browser| {
            let until = Instant::now() + Duration::from_secs(20);
            while browser.busy {
                browser.poll();
                assert!(Instant::now() < until, "{}", browser.status);
                std::thread::sleep(Duration::from_millis(10));
            }
        };
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
        let pid: u32 = std::fs::read_to_string(root.join("agent-child.pid"))
            .unwrap()
            .parse()
            .unwrap();
        drop(chat);
        let until = Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
            !s.rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
        }) {
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

fn wait_agent(
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

fn binary_extension_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan, outside: &str) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let root = std::path::Path::new(&plan.root);
    let path = root
        .join("binary 'quoted' 🦀.bin")
        .to_string_lossy()
        .into_owned();
    let bytes = [0, 0xff, 0x80, 0x90, 0xc3];
    std::fs::write(&path, bytes).unwrap();
    let response = remote
        .request_in(&Request::ReadHex { path: path.clone() }, Some(plan))
        .unwrap();
    assert_eq!(STANDARD.decode(response.binary.unwrap()).unwrap(), bytes);
    remote
        .request_in(
            &Request::WriteBinary {
                path: path.clone(),
                data: STANDARD.encode([0, 0x90, 0x90, 0x90, 0xc3]),
                expected: Some(response.revision),
            },
            Some(plan),
        )
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), [0, 0x90, 0x90, 0x90, 0xc3]);
    if plan.active {
        assert!(
            remote
                .request_in(
                    &Request::ReadHex {
                        path: outside.into()
                    },
                    Some(plan)
                )
                .is_err()
        );
    }
    let adapter = root.join("extension.py");
    std::fs::write(&adapter, include_str!("fixtures/extension.py")).unwrap();
    let invocation = rode::extensions::Invocation {
        api: 1,
        extension: "test.adapter".into(),
        program: "python3".into(),
        args: vec![adapter.to_string_lossy().into()],
        operation: "format".into(),
        path: Some(path),
        text: "unsaved".into(),
        settings: Default::default(),
    };
    let reply = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::Extension {
                    root: plan.root.clone(),
                    invocation: invocation.clone(),
                },
            },
            Some(plan),
        )
        .unwrap()
        .tool
        .unwrap();
    let rode::project::Response::Extension(reply) = reply else {
        panic!()
    };
    reply.validate("format").unwrap();
    assert_eq!(reply.text.as_deref(), Some("UNSAVED"));
    let mut invocation = invocation;
    invocation.operation = "test.inspect".into();
    invocation
        .settings
        .insert("outside".into(), serde_json::json!(outside));
    let reply = remote
        .request_in(
            &Request::Tool {
                request: rode::project::Request::Extension {
                    root: plan.root.clone(),
                    invocation,
                },
            },
            Some(plan),
        )
        .unwrap()
        .tool
        .unwrap();
    let rode::project::Response::Extension(reply) = reply else {
        panic!()
    };
    assert_eq!(reply.output, if plan.active { "False" } else { "True" });
}

fn notebook_roundtrip(remote: &Remote, root: &str, outside: &str) {
    use rode::notebook::kernel::{Event, Kernel, Run};
    let Ok(python) = std::env::var("RODE_TEST_JUPYTER") else {
        return;
    };
    let env = std::path::Path::new(&python)
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let plan = remote
        .request(&Request::Prepare {
            path: root.into(),
            sandbox: rode::sandbox::Options {
                enabled: true,
                share_network: false,
                read_only: vec![env.to_string_lossy().into()],
                ..Default::default()
            },
        })
        .unwrap()
        .project
        .unwrap();
    let file = std::path::Path::new(root)
        .join("notebook 'quoted' 🦀.ipynb")
        .to_string_lossy()
        .into_owned();
    let wrote = remote
        .request_in(
            &Request::Write {
                path: file.clone(),
                text: include_str!("fixtures/notebook.ipynb").into(),
                expected: None,
            },
            Some(&plan),
        )
        .unwrap();
    let read = remote
        .request_in(&Request::Read { path: file.clone() }, Some(&plan))
        .unwrap();
    assert_eq!(read.revision, wrote.revision);
    let book = rode::notebook::Notebook::parse(&read.text).unwrap();
    assert_eq!(book.cells.len(), 4);
    let mut kernel = Kernel::start(
        plan.clone(),
        Some(remote.clone()),
        python,
        "python3".into(),
        std::sync::Arc::new(|| {}),
    )
    .unwrap();
    kernel.run(vec![Run{cell: String::new(),
uid:1,version:0,code:"value=40".into()},Run{cell: String::new(),
uid:2,version:0,code:format!("import os\nprint(value+2)\nprint(os.getcwd())\nprint(os.path.exists({outside:?}))")}]).unwrap();
    let mut done = 0;
    let mut text = String::new();
    let start = Instant::now();
    while done < 2 {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{}",
            kernel.status
        );
        for event in kernel.poll() {
            match event {
                Event::Done(_, status) => {
                    assert_eq!(status, "ok");
                    done += 1;
                }
                Event::Output { output, .. } => {
                    if let Some(value) = output["text"].as_str() {
                        text.push_str(value);
                    }
                }
                Event::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(text.contains("42\n"));
    assert!(text.contains(root));
    assert!(text.contains(if plan.active { "False\n" } else { "True\n" }));
    kernel.stop();
    assert!(
        remote.connected(),
        "Notebook shutdown ended the shared SSH connection"
    );
}

fn encrypted_roundtrip(remote: &Remote, root: &std::path::Path) {
    let cipher = root.join("encrypted quoted 'folder'");
    std::fs::create_dir(&cipher).unwrap();
    std::fs::write(cipher.join("gocryptfs.conf"), "fixture").unwrap();
    let request = rode::vault::Open {
        settings: rode::vault::Settings {
            cipherdir: cipher.to_string_lossy().into(),
            program: "/usr/bin/true".into(),
            ..Default::default()
        },
        password: "ssh-test-password".into(),
        create: false,
        lock_only: false,
    };
    let client =
        rode::vault::Client::start(request, Some(remote.clone()), std::sync::Arc::new(|| {}))
            .unwrap();
    let until = Instant::now() + Duration::from_secs(15);
    let mut failed = false;
    while Instant::now() < until {
        if let Some(rode::process::Event::Message(value)) = client.session.poll()
            && let Ok(rode::vault::Reply::Error { message }) = serde_json::from_value(value)
        {
            assert!(message.contains("verified encrypted mount"), "{message}");
            assert!(!message.contains("ssh-test-password"));
            failed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        failed,
        "SSH encrypted mount verification did not fail closed"
    );
    drop(client);
    assert!(
        remote.connected(),
        "Encrypted-project cleanup closed the SSH master"
    );
}

fn debugger_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rode::debugger::{
        Backend, Breakpoint, Event, Settings,
        protocol::{Client, Phase},
    };
    use std::{
        fs,
        sync::{Arc, atomic::Ordering},
    };
    let root = std::path::Path::new(&plan.root);
    let adapter = root.join("debug-adapter.py");
    fs::write(&adapter, include_str!("fixtures/debug_adapter.py")).unwrap();
    let source = root.join("main.py");
    fs::write(&source,"answer = 40\nanswer += 2\nprint(answer, flush=True)\nname=input('Name: ')\nprint('HELLO '+name, flush=True)\n").unwrap();
    let wait = |client: &mut Client, check: &dyn Fn(&Client) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            client.poll();
            if check(client) {
                break;
            }
            assert!(
                client.error.is_empty(),
                "{}\n{}",
                client.error,
                client.output
            );
            assert!(
                Instant::now() < deadline,
                "{}\n{}",
                client.status,
                client.output
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let mut settings = Settings::default();
    settings.config.backend = Backend::Custom;
    settings.config.adapter = "python3".into();
    settings.config.adapter_arguments = vec![adapter.to_string_lossy().into()];
    settings.breakpoints.insert(
        source.to_string_lossy().into(),
        vec![Breakpoint {
            line: 3,
            ..Default::default()
        }],
    );
    settings.watches = vec!["answer".into()];
    let mut client = Client::start(
        plan.clone(),
        Some(remote.clone()),
        source.to_string_lossy().into(),
        "auto".into(),
        settings,
        Arc::new(|| {}),
    )
    .unwrap();
    wait(&mut client, &|c| {
        c.phase == Phase::Paused && c.watch_values[0] == "42 🦀"
    });
    let mut data = String::new();
    let mut terminal = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !data.contains("Name: ") {
        client.poll();
        while let Some(event) = client.terminal_events.pop_front() {
            match event {
                Event::Terminal { id, .. } => terminal = Some(id),
                Event::Data { data: bytes, .. } => {
                    data.push_str(&String::from_utf8_lossy(&STANDARD.decode(bytes).unwrap()))
                }
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "{data}");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(data.contains("TTY=True"));
    client.writer().unwrap().send(&serde_json::json!({"op":"input","id":terminal.unwrap(),"data":STANDARD.encode("SSH\n")})).unwrap();
    wait(&mut client, &|_| root.join("input-result").exists());
    assert_eq!(
        fs::read_to_string(root.join("input-result")).unwrap(),
        "SSH"
    );
    client.control("next").unwrap();
    wait(&mut client, &|c| {
        c.phase == Phase::Paused && c.frames.first().is_some_and(|f| f["line"] == 4)
    });
    client.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    while client.cleanup().load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        remote.connected(),
        "Debug cleanup closed authenticated master"
    );
    if let Ok(python) =
        std::env::var("RODE_TEST_DEBUGPY").or_else(|_| std::env::var("RODE_TEST_JUPYTER"))
    {
        let env = std::path::Path::new(&python)
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let policy = remote
            .request(&Request::Prepare {
                path: plan.root.clone(),
                sandbox: rode::sandbox::Options {
                    enabled: true,
                    read_only: vec![env.to_string_lossy().into()],
                    ..Default::default()
                },
            })
            .unwrap()
            .project
            .unwrap();
        let mut settings = Settings::default();
        settings.breakpoints.insert(
            source.to_string_lossy().into(),
            vec![Breakpoint {
                line: 2,
                ..Default::default()
            }],
        );
        settings.watches = vec!["answer".into()];
        let mut client = Client::start(
            policy,
            Some(remote.clone()),
            source.to_string_lossy().into(),
            python,
            settings,
            Arc::new(|| {}),
        )
        .unwrap();
        wait(&mut client, &|c| {
            c.phase == Phase::Paused && c.watch_values[0] == "40"
        });
        client.control("next").unwrap();
        wait(&mut client, &|c| {
            c.phase == Phase::Paused && c.watch_values[0] == "42"
        });
        client.stop();
        let deadline = Instant::now() + Duration::from_secs(8);
        while client.cleanup().load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(remote.connected());
    }
}

fn native_debugger_roundtrip(remote: &Remote, root: &str) {
    use rode::debugger::{
        Backend, Breakpoint, Settings,
        protocol::{Client, Phase},
    };
    use std::{
        fs,
        sync::{Arc, atomic::Ordering},
    };
    let Ok(adapter) = std::env::var("RODE_TEST_LLDB") else {
        return;
    };
    let source = std::path::Path::new(root).join("native-debug.c");
    let binary = std::path::Path::new(root).join("native-debug");
    fs::write(&source,"#include <stdio.h>\nint main(void){\n int answer=40;\n answer+=2;\n printf(\"%d\\n\",answer);\n return 0;\n}\n").unwrap();
    let status = Command::new("cc")
        .arg("-g")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .status()
        .unwrap();
    assert!(status.success());
    let plan = remote
        .request(&Request::Prepare {
            path: root.into(),
            sandbox: rode::sandbox::Options {
                enabled: true,
                read_only: vec![
                    std::path::Path::new(&adapter)
                        .parent()
                        .unwrap()
                        .to_string_lossy()
                        .into(),
                ],
                ..Default::default()
            },
        })
        .unwrap()
        .project
        .unwrap();
    let mut settings = Settings::default();
    settings.config.backend = Backend::Lldb;
    settings.config.adapter = adapter;
    settings.config.program = binary.to_string_lossy().into();
    if let Ok(libraries) = std::env::var("LD_LIBRARY_PATH") {
        settings.config.extra = serde_json::json!({"env":{"LD_LIBRARY_PATH":libraries}});
    }
    settings.breakpoints.insert(
        source.to_string_lossy().into(),
        vec![Breakpoint {
            line: 4,
            ..Default::default()
        }],
    );
    settings.watches = vec!["answer".into()];
    let mut client = Client::start(
        plan,
        Some(remote.clone()),
        source.to_string_lossy().into(),
        "auto".into(),
        settings,
        Arc::new(|| {}),
    )
    .unwrap();
    let wait = |client: &mut Client, value: &str| {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            client.poll();
            if client.phase == Phase::Paused && client.watch_values[0].contains(value) {
                break;
            }
            if !client.error.is_empty() {
                for event in &client.terminal_events {
                    if let rode::debugger::Event::Data { data, .. } = event {
                        use base64::{Engine, engine::general_purpose::STANDARD};
                        eprintln!(
                            "Debug terminal: {}",
                            String::from_utf8_lossy(&STANDARD.decode(data).unwrap_or_default())
                        );
                    }
                }
            }
            assert!(
                client.error.is_empty(),
                "{}\n{}",
                client.error,
                client.output
            );
            assert!(
                Instant::now() < deadline,
                "{}\n{}",
                client.status,
                client.output
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    wait(&mut client, "40");
    client.control("next").unwrap();
    wait(&mut client, "42");
    client.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    while client.cleanup().load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(remote.connected());
}

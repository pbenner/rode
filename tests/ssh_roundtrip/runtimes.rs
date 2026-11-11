//! Slurm shells, notebooks and debugger sessions on the project host.
use super::*;

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
        capabilities::skip(
            "jupyter",
            "Set RODE_TEST_JUPYTER to exercise Slurm notebooks over SSH",
        );
        return;
    };
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    for jump in ["", "rode-jump"] {
        let root = tempfile::tempdir().unwrap();
        let tools = root.path().join("tools");
        fs::create_dir(&tools).unwrap();
        for name in ["srun", "squeue"] {
            let path = tools.join(name);
            fs::write(&path, include_str!("../fixtures/slurm.py")).unwrap();
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
        capabilities::skip(
            "jupyter",
            "Set RODE_TEST_JUPYTER for persistent Slurm runtime coverage",
        );
        return;
    };
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    struct Cleanup(rode::sandbox::Plan, Vec<Info>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for info in &self.1 {
                let _ = runtime::request(
                    &self.0,
                    None,
                    rode_session::Request::EndRuntime {
                        id: info.id.clone(),
                        policy: info.policy.clone(),
                    },
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
        let reply = runtime::request(plan, Some(remote), rode_session::Request::Runtimes).unwrap();
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
            fs::write(&path, include_str!("../fixtures/slurm.py")).unwrap();
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
        // Publish readiness only after closing the written file. The parent
        // observes existence; open(..., 'w') exposes an empty file first.
        let runs = vec![
            Run {
                uid: 1,
                cell: "first".into(),
                version: 0,
                code: format!(
                    "import os,time\nfrom pathlib import Path\ncounter=41\nmarker=Path({:?})\nmarker.write_text(os.environ['SLURM_JOB_ID'])\nos.replace(marker,{:?})\ntime.sleep(1)",
                    first.with_extension("tmp").to_string_lossy(),
                    first.to_string_lossy()
                ),
            },
            Run {
                uid: 2,
                cell: "second".into(),
                version: 0,
                code: format!(
                    "counter+=1\nprint(counter)\nmarker=Path({:?})\nmarker.write_text(str(counter))\nos.replace(marker,{:?})",
                    done.with_extension("tmp").to_string_lossy(),
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
        fs::write(&adapter, include_str!("../fixtures/debug_adapter.py")).unwrap();
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
        runtime::request(
            &plan,
            Some(&remote),
            rode_session::Request::EndRuntime {
                id: kernel_info.id,
                policy: kernel_info.policy,
            },
        )
        .unwrap();
        drop(client);
        drop(login);
        remote.cleanup();
    }
}

pub(super) fn native_attach(
    remote: &Remote,
    reply: &rode::sessions::Reply,
    info: &rode_session::Info,
) -> Terminal {
    let mut command = portable_pty::CommandBuilder::new(remote.client_program());
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

pub(super) fn interactive_allocation_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};
    let root = Path::new(&plan.root);
    let tools = root.join("slurm-tools");
    fs::create_dir_all(&tools).unwrap();
    for name in ["salloc", "srun", "squeue", "scancel"] {
        let path = tools.join(name);
        fs::write(&path, include_str!("../fixtures/slurm.py")).unwrap();
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
    terminal_ui::wait_for_prompt(&view);
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

pub(super) fn native_terminal_start(
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

pub(super) fn interpreter_grants(python: &str) -> Vec<String> {
    // The isolated sshd has its own HOME. A uv-managed venv may link into the
    // desktop user's .local tree, so grant that interpreter's installation too.
    let configured = std::path::Path::new(python);
    let real = configured.canonicalize().unwrap();
    let mut paths = [configured, real.as_path()]
        .into_iter()
        .map(|path| {
            path.parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

pub(super) fn debugger_roundtrip(remote: &Remote, plan: &rode::sandbox::Plan) {
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
    fs::write(&adapter, include_str!("../fixtures/debug_adapter.py")).unwrap();
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
        let policy = remote
            .request(&Request::Prepare {
                path: plan.root.clone(),
                sandbox: rode::sandbox::Options {
                    enabled: true,
                    read_only: interpreter_grants(&python),
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

pub(super) fn native_debugger_roundtrip(remote: &Remote, root: &str) {
    use rode::debugger::{
        Backend, Breakpoint, Settings,
        protocol::{Client, Phase},
    };
    use std::{
        fs,
        sync::{Arc, atomic::Ordering},
    };
    let Ok(adapter) = std::env::var("RODE_TEST_LLDB") else {
        capabilities::skip(
            "lldb",
            "Set RODE_TEST_LLDB for remote native debugger coverage",
        );
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

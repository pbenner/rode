//! Real native service/PTYS, shared attachment, explicit persistence, upgrade compatibility and
//! idle/child cleanup in private runtimes.

use portable_pty::CommandBuilder;
use rode::{sandbox, sessions, terminal::Terminal};
use rode_session::{Request, Response};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn wait(check: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < end, "Timed out waiting for terminal work");
        std::thread::sleep(Duration::from_millis(40));
    }
}
struct Fixture {
    root: tempfile::TempDir,
    runtime: PathBuf,
    ids: Vec<(String, String)>,
    standalone: bool,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        Self {
            root,
            runtime,
            ids: vec![],
            standalone: false,
        }
    }
    fn rpc(&self, request: &Request) -> anyhow::Result<Response> {
        let mut command = if self.standalone {
            Command::new(env!("CARGO_BIN_EXE_rode-sessiond"))
        } else {
            let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
            command.arg("--session-service");
            command
        };
        let mut child = command
            .arg("request")
            .env("RODE_SESSION_DIR", &self.runtime)
            .env("RODE_TEST_PARENT_ONLY", "daemon-only")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(request)?)?;
        let out = child.wait_with_output()?;
        anyhow::ensure!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Response>(&out.stdout)?.checked()
    }
    fn create(&mut self, plan: &sandbox::Plan) -> rode_session::Info {
        self.create_with_persistence(plan, true)
    }
    fn create_with_persistence(
        &mut self,
        plan: &sandbox::Plan,
        persistent: bool,
    ) -> rode_session::Info {
        let id = rode_session::new_id().unwrap();
        let policy = sessions::fingerprint(plan);
        let mut argv = if plan.active {
            plan.prefix.clone()
        } else {
            vec![]
        };
        argv.push("/bin/sh".into());
        let response = self
            .rpc(&Request::Create {
                allocation: None,
                persistent,
                id: id.clone(),
                name: "Native test".into(),
                root: plan.root.clone(),
                policy: policy.clone(),
                mode: plan.label().into(),
                argv,
                initial_input: String::new(),
                env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
            })
            .unwrap();
        self.ids.push((id, policy));
        let Response::Session(info) = response else {
            panic!()
        };
        info
    }
    fn attach(&self, info: &rode_session::Info) -> Terminal {
        let mut cmd = if self.standalone {
            CommandBuilder::new(env!("CARGO_BIN_EXE_rode-sessiond"))
        } else {
            let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_rode"));
            command.arg("--session-service");
            command
        };
        cmd.args(["attach", "--framed", &info.id, &info.policy]);
        cmd.env("RODE_SESSION_DIR", &self.runtime);
        Terminal::spawn_native(&eframe::egui::Context::default(), cmd, "Native test".into())
            .unwrap()
    }
    fn direct_list(&self) -> Vec<rode_session::Info> {
        // Inspect the daemon itself, bypassing client-side filtering for
        // older helpers, so these checks prove its records are released.
        let mut stream =
            std::os::unix::net::UnixStream::connect(self.runtime.join("socket")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request = br#"{"action":"list"}"#;
        stream
            .write_all(&((request.len() + 1) as u32).to_be_bytes())
            .unwrap();
        stream.write_all(b"J").unwrap();
        stream.write_all(request).unwrap();
        let (kind, bytes) = rode_session::read_stream(&mut stream).unwrap();
        assert_eq!(kind, b'J');
        let Response::Sessions(infos) = serde_json::from_slice::<Response>(&bytes)
            .unwrap()
            .checked()
            .unwrap()
        else {
            panic!()
        };
        infos
    }
    fn plan(&self) -> sandbox::Plan {
        let mut plan = sandbox::Plan::default();
        plan.root = self.root.path().to_string_lossy().into();
        plan
    }
}
#[test]
fn cargo_helper_is_headless_and_shares_persistent_sessions_with_the_editor() {
    let protocol = Command::new(env!("CARGO_BIN_EXE_rode-sessiond"))
        .arg("protocol")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap();
    assert!(protocol.status.success());
    assert_eq!(
        String::from_utf8(protocol.stdout).unwrap().trim(),
        rode_session::PROTOCOL.to_string()
    );
    let mut fixture = Fixture::new();
    fixture.standalone = true;
    let info = fixture.create(&fixture.plan());
    let terminal = fixture.attach(&info);
    terminal
        .send(b"TOKEN=cargo-installed; printf 'READY_%s\\n' HELPER\n".to_vec())
        .unwrap();
    wait(|| terminal.contents().contains("READY_HELPER"));
    drop(terminal);
    // The helper's short-lived creator/attachment clients have exited; an
    // ordinary editor process must still discover and use the same owned shell.
    fixture.standalone = false;
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    assert!(
        infos
            .iter()
            .any(|session| session.id == info.id && session.persistent)
    );
    let terminal = fixture.attach(&info);
    terminal
        .send(b"printf 'TOKEN_%s\\n' \"$TOKEN\"\n".to_vec())
        .unwrap();
    wait(|| terminal.contents().contains("TOKEN_cargo-installed"));
}

#[test]
fn an_unlinked_running_image_can_start_an_independent_native_service() {
    let mut fixture = Fixture::new();
    let executable = fixture.root.path().join("rode-running");
    fs::copy(env!("CARGO_BIN_EXE_rode"), &executable).unwrap();
    let mut command = Command::new(&executable);
    command
        .args(["--session-service", "request"])
        .env("RODE_SESSION_DIR", &fixture.runtime)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Parallel PTY forks can briefly inherit this fixture's copy writer before
    // their exec closes CLOEXEC descriptors. Retry only that initial setup.
    let until = Instant::now() + Duration::from_secs(2);
    let mut caller = loop {
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(10))
            }
            result => break result.unwrap(),
        }
    };
    // spawn() has completed exec, while the request still waits for stdin EOF.
    let replacement = fixture.root.path().join("rode-updated");
    fs::copy(env!("CARGO_BIN_EXE_rode"), &replacement).unwrap();
    fs::rename(replacement, executable).unwrap();
    let id = rode_session::new_id().unwrap();
    let policy = "a".repeat(64);
    fixture.ids.push((id.clone(), policy.clone()));
    let request = Request::Create {
        allocation: None,
        id,
        policy,
        persistent: true,
        name: "Before update".into(),
        root: fixture.root.path().to_string_lossy().into(),
        mode: "NORMAL MODE".into(),
        argv: vec!["/bin/sh".into()],
        initial_input: String::new(),
        env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
    };
    caller
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = caller.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let Response::Session(info) = serde_json::from_slice::<Response>(&output.stdout)
        .unwrap()
        .checked()
        .unwrap()
    else {
        panic!("No native session")
    };
    // The short-lived creator is gone. The service owns its image and PTY.
    let terminal = fixture.attach(&info);
    terminal
        .send(b"printf 'UPDATE_STILL_WORKS\\n'\n".to_vec())
        .unwrap();
    wait(|| terminal.contents().contains("UPDATE_STILL_WORKS"));
}

#[test]
fn native_ssh_attach_never_runs_a_synchronous_connection_probe() {
    if Command::new("ssh").arg("-V").output().is_err() {
        eprintln!("OpenSSH is needed for the asynchronous attach check");
        return;
    }
    let remote = rode::ssh::Remote::new(rode::ssh::Profile {
        host: "rode-unreachable-fixture".into(),
        ..Default::default()
    })
    .unwrap();
    // A socket can vanish between a worker reply and attaching its PTY. The
    // channel fails asynchronously, and ProxyCommand=false prevents a new login.
    fs::write(&remote.socket, "stale fixture").unwrap();
    let reply = sessions::Reply {
        response: Response::Ok,
        executable: "/unused-helper".into(),
        prefix: vec![],
    };
    let info: rode_session::Info = serde_json::from_value(serde_json::json!({
        "id":"s-0123456789abcdef0123456789abcdef", "name":"Fixture", "root":"/", "policy":"a".repeat(64),
        "mode":"NORMAL MODE", "alive":true,"clients":0,"created":0,"exit_code":null
    })).unwrap();
    let result = Terminal::native_session(
        &eframe::egui::Context::default(),
        Some(&remote),
        &reply,
        &info,
    );
    remote.cleanup();
    let terminal = result.unwrap();
    wait(|| !terminal.is_alive());
}

#[test]
fn persistence_changes_in_place_and_nonpersistent_sessions_end_only_after_the_last_view() {
    let mut fixture = Fixture::new();
    let keeper = fixture.create(&fixture.plan());
    let info = fixture.create_with_persistence(&fixture.plan(), false);
    let first = fixture.attach(&info);
    let second = fixture.attach(&info);
    wait(|| !first.persistent() && !second.persistent());
    first
        .send(b"TOKEN=original; printf 'STATE_%s\\n' READY\n".to_vec())
        .unwrap();
    wait(|| second.contents().contains("STATE_READY"));
    assert!(
        fixture
            .rpc(&Request::SetPersistence {
                id: info.id.clone(),
                policy: "wrong".into(),
                persistent: true,
            })
            .is_err()
    );
    fixture
        .rpc(&Request::SetPersistence {
            id: info.id.clone(),
            policy: info.policy.clone(),
            persistent: true,
        })
        .unwrap();
    wait(|| first.persistent() && second.persistent());
    drop(first);
    drop(second);
    let first = fixture.attach(&info);
    let second = fixture.attach(&info);
    first
        .send(b"printf 'TOKEN_%s\\n' \"$TOKEN\"\n".to_vec())
        .unwrap();
    wait(|| second.contents().contains("TOKEN_original"));
    fixture
        .rpc(&Request::SetPersistence {
            id: info.id.clone(),
            policy: info.policy.clone(),
            persistent: false,
        })
        .unwrap();
    wait(|| !first.persistent() && !second.persistent());
    drop(first);
    second
        .send(b"printf 'STILL_%s\\n' RUNNING\n".to_vec())
        .unwrap();
    wait(|| second.contents().contains("STILL_RUNNING"));
    drop(second);
    wait(|| match fixture.rpc(&Request::List).unwrap() {
        Response::Sessions(infos) => !infos.iter().any(|i| i.id == info.id),
        _ => false,
    });
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    assert!(
        infos
            .iter()
            .any(|i| i.id == keeper.id && i.alive && i.persistent)
    );
}

#[test]
fn older_session_metadata_retains_persistence_instead_of_ending_existing_jobs() {
    let info: rode_session::Info = serde_json::from_value(serde_json::json!({
        "id":"legacy", "name":"old", "root":"/", "policy":"old", "mode":"NORMAL MODE",
        "alive":true, "clients":1, "created":0, "exit_code":null,
    }))
    .unwrap();
    assert!(info.persistent);
    assert!(!info.persistence_supported);
    assert!(info.allocation.is_none());
}

#[test]
fn interactive_allocation_waits_then_supplies_the_scheduler_environment_and_survives_reattachment()
{
    use rode::slurm;
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let tools = fixture.root.path().join("tools");
    fs::create_dir(&tools).unwrap();
    for name in ["salloc", "srun", "squeue", "scancel"] {
        let path = tools.join(name);
        fs::write(&path, include_str!("fixtures/slurm.py")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let shell = tools.join("shell");
    fs::write(&shell, "#!/bin/sh\nexec /bin/bash --noprofile --norc\n").unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
    let settings = slurm::Allocation {
        allocator: tools.join("salloc").to_string_lossy().into(),
        runner: tools.join("srun").to_string_lossy().into(),
        gres: "gpu:2".into(),
        cpus: 4,
        ..Default::default()
    };
    let id = rode_session::new_id().unwrap();
    let launch = settings.launch(&plan, &id).unwrap();
    let policy = sessions::fingerprint(&plan);
    let Response::Session(info) = fixture
        .rpc(&Request::Create {
            allocation: Some(launch.job_name.clone()),
            id: id.clone(),
            name: "Allocation test".into(),
            root: plan.root.clone(),
            policy: policy.clone(),
            mode: plan.label().into(),
            argv: launch.argv,
            initial_input: String::new(),
            persistent: true,
            env: BTreeMap::from([
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("SHELL".into(), shell.to_string_lossy().into()),
                ("SLURM_JOB_ID".into(), "stale-job".into()),
            ]),
        })
        .unwrap()
    else {
        panic!()
    };
    fixture.ids.push((id.clone(), policy.clone()));
    assert_eq!(info.allocation.as_deref(), Some(launch.job_name.as_str()));
    let view = fixture.attach(&info);
    let state = fixture.root.path().join("slurm-state");
    wait(|| state.join("job.json").exists());
    let queue = tools.join("squeue").to_string_lossy().into_owned();
    let snapshot = slurm::snapshot(fixture.root.path(), &queue).unwrap();
    assert_eq!(snapshot.jobs[0].state, "PENDING");
    assert!(!snapshot.jobs[0].allocated());
    fs::write(state.join("grant"), "").unwrap();
    wait(|| {
        fs::read_to_string(state.join("job.json"))
            .unwrap_or_default()
            .contains("RUNNING")
    });
    view.send(b"printf 'ENV_%s_%s_%s_%s\\n' \"$SLURM_JOB_ID\" \"$SLURM_JOB_GPUS\" \"$SLURM_STEP_ID\" \"$CUDA_VISIBLE_DEVICES\"\n".to_vec()).unwrap();
    wait(|| view.contents().contains("ENV_4242_0,1_0_0,1"));
    fixture
        .rpc(&Request::Rename {
            id: id.clone(),
            policy: policy.clone(),
            name: "Renamed training".into(),
        })
        .unwrap();
    drop(view);
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    let restored = infos.iter().find(|i| i.id == id).unwrap();
    assert_eq!(restored.allocation, info.allocation);
    let first = fixture.attach(restored);
    let second = fixture.attach(restored);
    second
        .send(b"printf 'RESTORED_%s\\n' \"$SLURM_JOB_ID\"\n".to_vec())
        .unwrap();
    wait(|| first.contents().contains("RESTORED_4242"));
    slurm::cancel(
        fixture.root.path(),
        &queue,
        &tools.join("scancel").to_string_lossy(),
        "4242",
    )
    .unwrap();
    wait(|| !second.is_alive());
    wait(|| state.join("ended").exists());
    assert!(state.join("cancelled").exists());
}

#[test]
fn notebook_reservations_survive_view_and_kernel_closure_until_explicit_release() {
    use rode::{
        notebook::kernel::{Event, Kernel, Run},
        slurm,
    };
    use std::{os::unix::fs::PermissionsExt, sync::Arc};
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let tools = fixture.root.path().join("tools");
    fs::create_dir(&tools).unwrap();
    for name in ["salloc", "srun", "squeue", "scancel"] {
        let path = tools.join(name);
        fs::write(&path, include_str!("fixtures/slurm.py")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let settings = slurm::Allocation {
        allocator: tools.join("salloc").to_string_lossy().into(),
        runner: tools.join("srun").to_string_lossy().into(),
        ..Default::default()
    };
    let id = rode_session::new_id().unwrap();
    let policy = sessions::fingerprint(&plan);
    let launch = settings.reserve(&plan, &id).unwrap();
    let Response::Session(info) = fixture
        .rpc(&Request::Create {
            id: id.clone(),
            name: "Notebook allocation".into(),
            root: plan.root.clone(),
            policy: policy.clone(),
            mode: plan.label().into(),
            argv: launch.argv,
            initial_input: String::new(),
            persistent: true,
            env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
            allocation: Some(launch.job_name.clone()),
        })
        .unwrap()
    else {
        panic!()
    };
    fixture.ids.push((id.clone(), policy));
    let view = fixture.attach(&info);
    let state = fixture.root.path().join("slurm-state");
    wait(|| state.join("job.json").exists());
    fs::write(state.join("grant"), "").unwrap();
    wait(|| view.contents().contains("is ready"));
    drop(view);
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    assert!(
        infos
            .iter()
            .any(|session| session.id == id && session.alive && session.persistent)
    );
    assert!(
        !state.join("step.json").exists(),
        "Reservation must not occupy a compute step"
    );
    let queue = tools.join("squeue").to_string_lossy().into_owned();
    let target = slurm::Target {
        id: "4242".into(),
        name: launch.job_name,
        queue: queue.clone(),
        runner: settings.runner,
    };
    let mut running = None;
    if let Ok(python) = std::env::var("RODE_TEST_JUPYTER") {
        let start_kernel = || {
            Kernel::start_on(
                plan.clone(),
                None,
                python.clone(),
                "python3".into(),
                Some(target.clone()),
                Arc::new(|| {}),
            )
            .unwrap()
        };
        let mut kernel = start_kernel();
        kernel.run(vec![Run { cell: String::new(),
uid: 1, version: 0, code: "import os\nprint('ALLOCATED', os.environ['SLURM_JOB_ID'], os.environ['CUDA_VISIBLE_DEVICES'])".into() }]).unwrap();
        let mut output = String::new();
        let deadline = Instant::now() + Duration::from_secs(35);
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
            assert!(Instant::now() < deadline, "{}", kernel.status);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(output.contains("ALLOCATED 4242 0,1"), "{output}");
        assert!(kernel.execution.contains("Slurm job 4242"));
        let marker = fixture.root.path().join("kernel-pid");
        kernel
            .run(vec![Run {
                cell: String::new(),
                uid: 2,
                version: 0,
                code: format!(
                    "import time\nopen({:?}, 'w').write(str(os.getpid()))\ntime.sleep(60)",
                    marker.to_string_lossy()
                ),
            }])
            .unwrap();
        wait(|| marker.exists());
        let pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
        kernel.interrupt().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while kernel.busy() {
            for event in kernel.poll() {
                if let Event::Error(error) = event {
                    panic!("{error}");
                }
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(kernel);
        wait(|| unsafe { libc::kill(pid, 0) } != 0);
        assert!(slurm::snapshot(fixture.root.path(), &queue).unwrap().jobs[0].allocated());
        let mut kernel = start_kernel();
        kernel
            .run(vec![Run {
                cell: String::new(),
                uid: 3,
                version: 0,
                code: "print('ready')".into(),
            }])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(35);
        while kernel.busy() {
            for event in kernel.poll() {
                if let Event::Error(error) = event {
                    panic!("{error}");
                }
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        running = Some(kernel);
    } else {
        eprintln!("Set RODE_TEST_JUPYTER for real Slurm notebook kernel/interrupt/expiry coverage");
    }
    slurm::cancel(
        fixture.root.path(),
        &queue,
        &tools.join("scancel").to_string_lossy(),
        "4242",
    )
    .unwrap();
    wait(|| state.join("ended").exists());
    if let Some(mut kernel) = running {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if kernel
                .poll()
                .iter()
                .any(|event| matches!(event, Event::Error(_)))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Kernel survived allocation release"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert!(
        slurm::snapshot(fixture.root.path(), &queue)
            .unwrap()
            .jobs
            .is_empty()
    );
}

#[test]
fn stale_service_sockets_do_not_hide_sessions_or_prevent_safe_startup() {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    for stale_compatible in [false, true] {
        let mut fixture = Fixture::new();
        fs::create_dir(&fixture.runtime).unwrap();
        fs::set_permissions(&fixture.runtime, fs::Permissions::from_mode(0o700)).unwrap();
        drop(UnixListener::bind(fixture.runtime.join("socket")).unwrap());
        if stale_compatible {
            let path = fixture.runtime.join("protocol-2");
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            drop(UnixListener::bind(path.join("socket")).unwrap());
        }
        let info = fixture.create_with_persistence(&fixture.plan(), false);
        assert!(info.persistence_supported && !info.persistent);
        let terminal = fixture.attach(&info);
        terminal
            .send(b"printf 'RECOVERED_%s\\n' READY\n".to_vec())
            .unwrap();
        wait(|| terminal.contents().contains("RECOVERED_READY"));
        let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
            panic!()
        };
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id, info.id);
    }
}

#[test]
fn older_service_keeps_its_jobs_while_new_terminals_get_persistence_controls() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let policy = sessions::fingerprint(&plan);
    let legacy_id = rode_session::new_id().unwrap();
    let mut legacy = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/session_v1.py"
        ))
        .arg(&fixture.runtime)
        .arg(&plan.root)
        .arg(&legacy_id)
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
    wait(|| fixture.runtime.join("socket").exists());
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    let old = infos.into_iter().find(|info| info.id == legacy_id).unwrap();
    assert!(old.persistent);
    assert!(!old.persistence_supported);
    let first = fixture.attach(&old);
    first
        .send(b"TOKEN=kept; printf 'LEGACY_%s\\n' READY\n".to_vec())
        .unwrap();
    wait(|| first.contents().contains("LEGACY_READY"));
    drop(first);

    let modern = fixture.create_with_persistence(&plan, false);
    assert!(modern.persistence_supported);
    assert!(!modern.persistent);
    assert!(
        fixture
            .runtime
            .join(format!("protocol-{}/socket", rode_session::PROTOCOL))
            .exists()
    );
    let new_view = fixture.attach(&modern);
    new_view
        .send(b"printf 'MODERN_%s\\n' READY\n".to_vec())
        .unwrap();
    wait(|| new_view.contents().contains("MODERN_READY"));
    fixture
        .rpc(&Request::SetPersistence {
            id: modern.id.clone(),
            policy: modern.policy.clone(),
            persistent: true,
        })
        .unwrap();
    wait(|| new_view.persistent());
    let error = fixture
        .rpc(&Request::SetPersistence {
            id: old.id.clone(),
            policy: old.policy.clone(),
            persistent: false,
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("Start a new terminal"), "{error}");
    assert!(!error.contains("unknown variant"), "{error}");

    let reattached = fixture.attach(&old);
    reattached
        .send(b"printf 'TOKEN_%s\\n' \"$TOKEN\"\n".to_vec())
        .unwrap();
    wait(|| reattached.contents().contains("TOKEN_kept"));
    fixture
        .rpc(&Request::Rename {
            id: old.id.clone(),
            policy: old.policy.clone(),
            name: "Retained job".into(),
        })
        .unwrap();
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    assert_eq!(infos.len(), 2);
    assert!(
        infos
            .iter()
            .any(|info| info.id == old.id && info.name == "Retained job")
    );
    assert!(infos.iter().all(|info| info.alive));
    assert!(fixture.rpc(&Request::Shutdown).is_err());
    let actions = fs::read_to_string(fixture.runtime.join("requests")).unwrap();
    assert!(
        !actions
            .lines()
            .any(|action| matches!(action, "create" | "set_persistence" | "shutdown"))
    );
    drop(reattached);
    fixture
        .rpc(&Request::End {
            id: old.id,
            policy: old.policy,
        })
        .unwrap();
    wait(|| !fixture.runtime.join("socket").exists());
    // Creation/retry/attachment still find the compatibility service after
    // the older daemon has gone away. No shell is restarted or duplicated.
    let replay = fixture
        .rpc(&Request::Create {
            allocation: None,
            id: modern.id.clone(),
            name: "Retry".into(),
            root: modern.root.clone(),
            policy: modern.policy.clone(),
            mode: modern.mode.clone(),
            argv: vec!["/bin/sh".into()],
            initial_input: String::new(),
            env: BTreeMap::new(),
            persistent: false,
        })
        .unwrap();
    assert!(
        matches!(replay, Response::Session(info) if info.persistent && info.name == modern.name)
    );
    let next = fixture.create_with_persistence(&plan, false);
    assert!(next.persistence_supported && !next.persistent);
    new_view
        .send(b"printf 'MODERN_%s\\n' SURVIVED\n".to_vec())
        .unwrap();
    wait(|| new_view.contents().contains("MODERN_SURVIVED"));
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for (id, policy) in &self.ids {
            let _ = self.rpc(&Request::End {
                id: id.clone(),
                policy: policy.clone(),
            });
        }
        if self.runtime.exists() {
            let _ = self.rpc(&Request::Shutdown);
            let end = Instant::now() + Duration::from_secs(3);
            while self.runtime.join("socket").exists() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}
#[test]
fn native_jobs_survive_disconnect_and_multiple_views_share_the_same_shell() {
    let mut fixture = Fixture::new();
    let info = fixture.create(&fixture.plan());
    let first = fixture.attach(&info);
    first
        .send(b"printf 'FRESH_%s\\n' \"${RODE_TEST_PARENT_ONLY-unset}\"\n".to_vec())
        .unwrap();
    wait(|| first.contents().contains("FRESH_unset"));
    first.send(b"echo FIRST_CONNECTED\n".to_vec()).unwrap();
    wait(|| first.contents().contains("FIRST_CONNECTED"));
    let output = fixture.root.path().join("finished");
    let code = format!(
        "import time;from pathlib import Path;print('TRAINING_STARTED',flush=True);time.sleep(1);Path({:?}).write_text('done');print('TRAINING_DONE',flush=True)",
        output.to_str().unwrap()
    );
    first
        .send(format!("python3 -u -c {}\n", rode::ssh::quote(&code)).into_bytes())
        .unwrap();
    wait(|| first.contents().contains("TRAINING_STARTED"));
    drop(first);
    wait(|| output.exists());
    let second = fixture.attach(&info);
    wait(|| second.contents().contains("TRAINING_DONE"));
    let third = fixture.attach(&info);
    second.send(b"echo SHARED_OUTPUT\n".to_vec()).unwrap();
    wait(|| third.contents().contains("SHARED_OUTPUT"));
    third.send(b"echo OTHER_VIEW_INPUT\n".to_vec()).unwrap();
    wait(|| second.contents().contains("OTHER_VIEW_INPUT"));
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    assert_eq!(infos[0].clients, 2);
    assert!(infos[0].alive);
    let wrong = fixture.rpc(&Request::End {
        id: info.id.clone(),
        policy: "a".repeat(64),
    });
    assert!(wrong.is_err());
    fixture
        .rpc(&Request::Rename {
            id: info.id.clone(),
            policy: info.policy.clone(),
            name: "Training run".into(),
        })
        .unwrap();
    drop(second);
    drop(third);
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    assert_eq!(infos[0].name, "Training run");
}
#[test]
fn detached_native_sandbox_keeps_running_with_its_original_access_policy() {
    let mut fixture = Fixture::new();
    let project = fixture.root.path().join("project");
    fs::create_dir(&project).unwrap();
    let hidden = fixture.root.path().join("secret");
    fs::write(&hidden, "private").unwrap();
    let plan = sandbox::prepare(
        project.to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let info = fixture.create(&plan);
    let terminal = fixture.attach(&info);
    terminal
        .send(
            format!(
                "if test -e {}; then printf 'ACCESS_%s\\n' VISIBLE; else printf 'ACCESS_%s\\n' HIDDEN; fi\n",
                rode::ssh::quote(hidden.to_str().unwrap())
            )
            .into_bytes(),
        )
        .unwrap();
    wait(|| {
        terminal.contents().contains(if plan.active {
            "ACCESS_HIDDEN"
        } else {
            "ACCESS_VISIBLE"
        })
    });
    terminal
        .send(b"printf 'DETACH_%s\\n' READY; sleep 1; echo detached > survived\n".to_vec())
        .unwrap();
    wait(|| terminal.contents().contains("DETACH_READY"));
    drop(terminal);
    wait(|| project.join("survived").exists());
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    assert!(infos[0].alive);
    assert_eq!(infos[0].policy, sessions::fingerprint(&plan));
}
#[test]
fn ending_one_native_terminal_does_not_end_another() {
    let mut fixture = Fixture::new();
    let first = fixture.create(&fixture.plan());
    let second = fixture.create(&fixture.plan());
    fixture
        .rpc(&Request::End {
            id: first.id,
            policy: first.policy,
        })
        .unwrap();
    let terminal = fixture.attach(&second);
    terminal.send(b"echo SECOND_ALIVE\n".to_vec()).unwrap();
    wait(|| terminal.contents().contains("SECOND_ALIVE"));
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    assert_eq!(infos.len(), 1);
    assert!(infos[0].alive);
}

#[test]
fn natural_exit_drains_tail_output_and_removes_the_session_automatically() {
    let mut fixture = Fixture::new();
    let info = fixture.create(&fixture.plan());
    let terminal = fixture.attach(&info);
    terminal.send(b"python3 -c 'import sys;sys.stdout.write(\"line\\n\"*5000)'; printf 'FINAL_%s\\n' TAIL; exit\n".to_vec()).unwrap();
    wait(|| !terminal.is_alive());
    assert!(
        terminal.contents().contains("FINAL_TAIL"),
        "Final output was lost at shell exit"
    );
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    assert!(infos.is_empty());
    assert!(fixture.direct_list().is_empty());
    assert!(
        fixture
            .rpc(&Request::Attach {
                id: info.id,
                policy: info.policy
            })
            .is_err()
    );
    // Keep the finished view open: it retains the tail without keeping any
    // record or PTY in the service, and cannot prevent idle shutdown.
    wait(|| {
        let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
            panic!()
        };
        assert!(infos.iter().all(|info| !info.alive));
        !fixture.runtime.join("socket").exists()
    });
    assert!(
        matches!(fixture.rpc(&Request::List).unwrap(), Response::Sessions(infos) if infos.is_empty())
    );
    assert!(terminal.contents().contains("FINAL_TAIL"));
    let next = fixture.create_with_persistence(&fixture.plan(), false);
    let terminal = fixture.attach(&next);
    terminal
        .send(b"printf 'RESTART_%s\\n' READY\n".to_vec())
        .unwrap();
    wait(|| terminal.contents().contains("RESTART_READY"));
}

#[test]
fn an_unused_service_exits_after_startup_and_removes_its_socket() {
    let fixture = Fixture::new();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_rode"))
        .args(["--session-service", "serve"])
        .env("RODE_SESSION_DIR", &fixture.runtime)
        .spawn()
        .unwrap();
    wait(|| fixture.runtime.join("socket").exists());
    wait(|| !fixture.runtime.join("socket").exists());
    assert!(daemon.wait().unwrap().success());
    assert!(
        fixture.runtime.join("lock").exists(),
        "Keep the stable lock for future clients"
    );
}

#[test]
fn exited_sessions_are_removed_while_other_detached_shells_keep_running() {
    let mut fixture = Fixture::new();
    let keeper = fixture.create(&fixture.plan());
    let info = fixture.create(&fixture.plan());
    let terminal = fixture.attach(&info);
    terminal
        .send(b"printf 'FINISHED_%s\\n' READY; exit\n".to_vec())
        .unwrap();
    wait(|| !terminal.is_alive());
    assert!(terminal.contents().contains("FINISHED_READY"));
    assert!(
        fixture
            .direct_list()
            .iter()
            .all(|record| record.id != info.id)
    );
    // Exceed the idle timeout while repeatedly exercising read-only discovery.
    // List/Attach must not be what keeps a service with no children alive.
    let until = Instant::now() + Duration::from_secs(7);
    while Instant::now() < until {
        let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
            panic!()
        };
        assert!(
            infos
                .iter()
                .any(|info| info.id == keeper.id && info.alive && info.clients == 0)
        );
        assert_eq!(infos.len(), 1);
        assert!(infos.iter().all(|record| record.id != info.id));
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(terminal.contents().contains("FINISHED_READY"));
    assert_eq!(fixture.direct_list().len(), 1);
    let keeper_view = fixture.attach(&keeper);
    keeper_view
        .send(b"printf 'KEEPER_%s\\n' READY\n".to_vec())
        .unwrap();
    wait(|| keeper_view.contents().contains("KEEPER_READY"));
    keeper_view.send(b"exit\n".to_vec()).unwrap();
    wait(|| !keeper_view.is_alive());
    wait(|| !fixture.runtime.join("socket").exists());
}

#[test]
fn project_mount_namespace_cannot_control_an_exposed_host_service_socket() {
    let mut fixture = Fixture::new();
    fixture.create(&fixture.plan());
    let mut child = match Command::new("bwrap")
        .args(["--bind", "/", "/", "--"])
        .arg(env!("CARGO_BIN_EXE_rode"))
        .args(["--session-service", "request"])
        .env("RODE_SESSION_DIR", &fixture.runtime)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("{error}"),
    };
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"action":"list"}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    if error.starts_with("bwrap:") {
        eprintln!("Mount namespace test unavailable: {error}");
        return;
    }
    assert!(error.contains("outside the project sandbox"), "{error}");
}

#[test]
fn concurrent_rode_clients_start_one_service_and_keep_independent_sessions() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let policy = sessions::fingerprint(&plan);
    let ids: Vec<_> = (0..8).map(|_| rode_session::new_id().unwrap()).collect();
    std::thread::scope(|scope| {
        for id in &ids {
            let fixture = &fixture;
            let plan = &plan;
            let policy = &policy;
            scope.spawn(move || {
                let response = fixture
                    .rpc(&Request::Create {
                        allocation: None,
                        persistent: true,
                        id: id.clone(),
                        name: "Terminal 1".into(),
                        root: plan.root.clone(),
                        policy: policy.clone(),
                        mode: "NORMAL MODE".into(),
                        argv: vec!["/bin/sh".into()],
                        initial_input: String::new(),
                        env: BTreeMap::new(),
                    })
                    .unwrap();
                assert!(matches!(response, Response::Session(_)));
            });
        }
    });
    fixture.ids = ids.into_iter().map(|id| (id, policy.clone())).collect();
    let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
        panic!()
    };
    assert_eq!(infos.len(), 8);
    assert_eq!(
        infos
            .iter()
            .map(|info| &info.name)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        8
    );
    assert!(infos.iter().all(|info| info.alive));
}

#[test]
fn new_sessions_activate_the_environment_after_shell_startup_but_reattach_does_not() {
    let mut fixture = Fixture::new();
    let venv = fixture.root.path().join(".venv");
    assert!(
        Command::new("python3")
            .args(["-m", "venv", "--without-pip"])
            .arg(&venv)
            .status()
            .unwrap()
            .success()
    );
    let plan = fixture.plan();
    let id = rode_session::new_id().unwrap();
    let policy = sessions::fingerprint(&plan);
    let response = fixture
        .rpc(&Request::Create {
            allocation: None,
            persistent: true,
            id: id.clone(),
            name: "Environment test".into(),
            root: plan.root,
            policy: policy.clone(),
            mode: "NORMAL MODE".into(),
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "PATH=/usr/bin:/bin; export PATH; exec /bin/sh".into(),
            ],
            initial_input: sessions::initial_input(&venv.join("bin/python")),
            env: BTreeMap::from([(
                "PATH".into(),
                format!("{}:/usr/bin:/bin", venv.join("bin").display()),
            )]),
        })
        .unwrap();
    fixture.ids.push((id, policy));
    let Response::Session(info) = response else {
        panic!()
    };
    let terminal = fixture.attach(&info);
    terminal
        .send(b"python -c 'import sys;open(\"prefix\",\"w\").write(sys.prefix)'\n".to_vec())
        .unwrap();
    let result = fixture.root.path().join("prefix");
    wait(|| result.exists());
    assert_eq!(fs::read_to_string(result).unwrap(), venv.to_str().unwrap());
    terminal
        .send(b"PATH=/usr/bin:/bin; export PATH; printf 'RESET_%s\\n' READY\n".to_vec())
        .unwrap();
    wait(|| terminal.contents().contains("RESET_READY"));
    drop(terminal);
    let reattached = fixture.attach(&info);
    reattached
        .send(b"printf '%s' \"$PATH\" > reattached-path\n".to_vec())
        .unwrap();
    let result = fixture.root.path().join("reattached-path");
    wait(|| result.exists());
    assert_eq!(fs::read_to_string(result).unwrap(), "/usr/bin:/bin");
}

#[test]
fn immediate_restart_after_ending_the_last_terminal_is_safe_and_create_is_idempotent() {
    let mut fixture = Fixture::new();
    for _ in 0..4 {
        let info = fixture.create(&fixture.plan());
        let replay = fixture
            .rpc(&Request::Create {
                allocation: None,
                persistent: true,
                id: info.id.clone(),
                name: "Must not replace the original".into(),
                root: info.root.clone(),
                policy: info.policy.clone(),
                mode: info.mode.clone(),
                argv: vec!["/bin/sh".into()],
                initial_input: "echo must-not-run > repeated-startup\n".into(),
                env: BTreeMap::new(),
            })
            .unwrap();
        let Response::Session(replay) = replay else {
            panic!()
        };
        assert_eq!(replay.name, info.name);
        assert!(!fixture.root.path().join("repeated-startup").exists());
        let Response::Sessions(infos) = fixture.rpc(&Request::List).unwrap() else {
            panic!()
        };
        assert_eq!(infos.len(), 1);
        fixture
            .rpc(&Request::End {
                id: info.id,
                policy: info.policy,
            })
            .unwrap();
    }
}

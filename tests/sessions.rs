//! Real native service/PTYS, shared attachment, explicit persistence and
//! idle/child cleanup in private runtimes.

#[path = "support/native.rs"]
mod native_peer;

#[path = "support/terminal.rs"]
mod terminal_ui;

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
        // Inspect the daemon itself, bypassing client-side filtering, so these
        // checks prove its records are released.
        let mut stream = rode_session::connect_control(&self.runtime).unwrap();
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
        sandbox::Plan {
            root: self.root.path().to_string_lossy().into(),
            ..Default::default()
        }
    }
}
#[test]
fn native_terminal_resize_reaches_the_shell_over_tty_and_pipe_attachments() {
    for piped in [false, true] {
        let mut fixture = Fixture::new();
        let info = fixture.create(&fixture.plan());
        let mut command = if piped {
            // SSH executes the attachment with pipes, without a remote TTY or
            // SIGWINCH. Relay bytes through the same kind of stdin here.
            let mut command = CommandBuilder::new("/bin/sh");
            command.args(["-c", "cat | \"$@\"", "pipe-attachment"]);
            command.arg(env!("CARGO_BIN_EXE_rode-sessiond"));
            command
        } else {
            CommandBuilder::new(env!("CARGO_BIN_EXE_rode-sessiond"))
        };
        command.args(["attach", "--framed", &info.id, &info.policy]);
        command.env("RODE_SESSION_DIR", &fixture.runtime);
        let mut terminal = Terminal::spawn_native(
            &eframe::egui::Context::default(),
            command,
            "Resize test".into(),
        )
        .unwrap();
        terminal_ui::check_sizes(&mut terminal, fixture.root.path());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn unauthenticated_connections_are_bounded_without_ending_retained_work() {
    use std::{
        io::Read,
        os::{
            linux::net::SocketAddrExt,
            unix::{
                fs::MetadataExt,
                net::{SocketAddr, UnixStream},
            },
        },
    };
    let mut fixture = Fixture::new();
    let info = fixture.create(&fixture.plan());
    let terminal = fixture.attach(&info);
    wait(|| {
        fixture
            .direct_list()
            .iter()
            .any(|item| item.id == info.id && item.clients == 1)
    });
    assert_eq!(
        fs::metadata(fixture.runtime.join("socket")).unwrap().mode() & 0o777,
        0o600
    );
    terminal
        .send(b"umask > inherited.umask\n".to_vec())
        .unwrap();
    let mask_path = fixture.root.path().join("inherited.umask");
    wait(|| fs::read_to_string(&mask_path).is_ok_and(|text| !text.is_empty()));
    assert_eq!(
        u32::from_str_radix(fs::read_to_string(&mask_path).unwrap().trim(), 8).unwrap(),
        0o077
    );
    let metadata = fs::metadata(&fixture.runtime).unwrap();
    let address = SocketAddr::from_abstract_name(format!(
        "rode-session-auth-{}-{}-{}",
        metadata.uid(),
        metadata.dev(),
        metadata.ino()
    ))
    .unwrap();
    let mut pending = Vec::new();
    for _ in 0..32 {
        pending.push(UnixStream::connect_addr(&address).unwrap());
    }
    let mut overflow = UnixStream::connect_addr(&address).unwrap();
    overflow
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    assert_eq!(
        overflow
            .read(&mut [0])
            .expect("An unauthenticated peer retained another worker"),
        0,
    );
    drop(pending.pop());
    wait(|| {
        matches!(fixture.rpc(&Request::List), Ok(Response::Sessions(items))
        if items.iter().any(|item| item.id == info.id && item.alive))
    });
    drop(pending);
    assert!(
        fixture
            .direct_list()
            .iter()
            .any(|item| item.id == info.id && item.alive)
    );
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
fn abandoned_remote_terminal_views_expire_and_apply_last_view_persistence() {
    std::thread::scope(|scope| {
        for persistent in [false, true] {
            scope.spawn(move || {
                let mut fixture = Fixture::new();
                let info = fixture.create_with_persistence(&fixture.plan(), persistent);
                let mut bridge = Command::new(env!("CARGO_BIN_EXE_rode-sessiond"))
                    .args(["attach", "--framed", "--heartbeat", &info.id, &info.policy])
                    .env("RODE_SESSION_DIR", &fixture.runtime)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                let input = bridge.stdin.take().unwrap();
                wait(|| {
                    fixture
                        .direct_list()
                        .iter()
                        .any(|i| i.id == info.id && i.clients == 1)
                });
                let until = Instant::now()
                    + rode_host_protocol::framing::CONNECTION_TIMEOUT
                    + Duration::from_secs(5);
                while bridge.try_wait().unwrap().is_none() {
                    if Instant::now() >= until {
                        let _ = bridge.kill();
                        let _ = bridge.wait();
                        panic!("Abandoned terminal bridge did not expire");
                    }
                    std::thread::sleep(Duration::from_millis(30));
                }
                drop(input);
                if persistent {
                    wait(|| {
                        fixture
                            .direct_list()
                            .iter()
                            .any(|i| i.id == info.id && i.alive && i.clients == 0)
                    });
                    let terminal = fixture.attach(&info);
                    terminal
                        .send(b"printf 'REATTACH_%s\\n' READY\n".to_vec())
                        .unwrap();
                    wait(|| terminal.contents().contains("REATTACH_READY"));
                } else {
                    wait(|| !fixture.runtime.join("socket").exists());
                }
            });
        }
    });
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
fn native_attachments_survive_startup_warnings_and_continue_framed_output() {
    let mut fixture = Fixture::new();
    let plan = sandbox::Plan {
        root: fixture.root.path().to_string_lossy().into(),
        ..Default::default()
    };
    let info = fixture.create(&plan);
    let mut command = CommandBuilder::new("/bin/sh");
    command.args([
        "-c",
        "printf 'shell startup warning\\n' >&2; exec \"$@\"",
        "attach-with-warning",
    ]);
    command.args([
        env!("CARGO_BIN_EXE_rode"),
        "--session-service",
        "attach",
        "--framed",
        &info.id,
        &info.policy,
    ]);
    command.env("RODE_SESSION_DIR", &fixture.runtime);
    let view = Terminal::spawn_native(
        &eframe::egui::Context::default(),
        command,
        "Startup warning".into(),
    )
    .unwrap();
    view.send(b"printf 'ATTACHED_%s\\n' after-warning\n".to_vec())
        .unwrap();
    wait(|| view.contents().contains("ATTACHED_after-warning"));
    assert!(view.is_alive());
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
        "mode":"NORMAL MODE", "alive":true,"clients":0,"created":0,"exit_code":null,"persistent":false,"allocation":null
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
        native_peer::capabilities::skip(
            "jupyter",
            "Set RODE_TEST_JUPYTER for real Slurm notebook kernel/interrupt/expiry coverage",
        );
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
    let mut fixture = Fixture::new();
    fs::create_dir(&fixture.runtime).unwrap();
    fs::set_permissions(&fixture.runtime, fs::Permissions::from_mode(0o700)).unwrap();
    drop(UnixListener::bind(fixture.runtime.join("socket")).unwrap());
    let info = fixture.create_with_persistence(&fixture.plan(), false);
    assert!(!info.persistent);
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

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(Response::Tunnels(tunnels)) = self.rpc(&Request::Tunnels) {
            for tunnel in tunnels {
                let _ = self.rpc(&Request::StopTunnel { id: tunnel.id });
            }
        }

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
    detached_sandbox(sandbox::Backend::Bwrap);
}
#[test]
fn detached_native_landlock_keeps_running_with_its_original_access_policy() {
    detached_sandbox(sandbox::Backend::Landlock);
}
fn detached_sandbox(backend: sandbox::Backend) {
    let mut fixture = Fixture::new();
    let project = fixture.root.path().join("project");
    fs::create_dir(&project).unwrap();
    let hidden = fixture.root.path().join("secret");
    fs::write(&hidden, "private").unwrap();
    let plan = native_peer::prepare(
        project.to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            backend,
            ..Default::default()
        },
    )
    .unwrap();
    let info = fixture.create(&plan);
    let terminal = fixture.attach(&info);
    terminal
        .send(
            format!(
                "if cat {} >/dev/null 2>&1; then printf 'ACCESS_%s\\n' VISIBLE; else printf 'ACCESS_%s\\n' HIDDEN; fi\n",
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
    let info = fixture.create(&fixture.plan());
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
    if let Err(error) = child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"action":"list"}"#)
    {
        // The sandbox/client can reject before consuming stdin. Its explicit
        // failure and diagnostic below remain mandatory; EPIPE alone is not proof.
        assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            ),
            "{error}"
        );
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    if error.starts_with("bwrap:") {
        eprintln!("Mount namespace test unavailable: {error}");
        return;
    }
    assert!(error.contains("outside the project sandbox"), "{error}");
    assert!(
        output.stdout.is_empty(),
        "Sandboxed control exposed session data"
    );
    assert!(
        fixture
            .direct_list()
            .iter()
            .any(|entry| entry.id == info.id && entry.alive),
        "Denied control request interrupted the existing host session"
    );
}

#[test]
fn landlock_cannot_use_the_host_service_as_an_unsandboxed_process_broker() {
    let mut fixture = Fixture::new();
    fixture.create(&fixture.plan());
    let project = fixture.root.path().join("landlock-project");
    fs::create_dir(&project).unwrap();
    for expose_runtime in [false, true] {
        let plan = native_peer::prepare(
            project.to_str().unwrap(),
            &sandbox::Options {
                enabled: true,
                backend: sandbox::Backend::Landlock,
                read_write: if expose_runtime {
                    vec![fixture.runtime.to_string_lossy().into()]
                } else {
                    vec![]
                },
                ..Default::default()
            },
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
        let mut command = Command::new(&plan.prefix[0]);
        command
            .args(&plan.prefix[1..])
            .args([
                "/usr/bin/python3",
                "-I",
                "-c",
                include_str!("fixtures/control_rejection.py"),
            ])
            .arg(fixture.runtime.join("socket"))
            .arg(if expose_runtime { "true" } else { "false" });
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fixture.direct_list().len(), 1);
    }
}

#[test]
fn control_rejection_probe_requires_a_complete_denial_even_after_an_early_close() {
    let result = Command::new("python3")
        .args([
            "-I",
            "-c",
            r#"
import errno, json, socket, struct, sys, threading
namespace = {'__name__': 'control_rejection_fixture'}
exec(sys.argv[1], namespace)
probe = namespace['assert_rejected']
read_frame = namespace['read_frame']
expected = 'outside the project sandbox'
request = b'J{"action":"list"}'
def frame(data, kind=b'J'):
    body = kind + json.dumps(data).encode()
    return struct.pack('>I', len(body)) + body
denial = frame({'kind':'error', 'data':expected})

class WriteFailure:
    def __init__(self, client, failure):
        self.client, self.failure = client, failure
    def sendall(self, _data):
        raise self.failure
    def recv(self, size):
        return self.client.recv(size)

# Force the peer to send its denial and close before the client writes.
# No scheduler timing, retries or sleeps are needed to reproduce EPIPE.
for failure in [None, ConnectionResetError(errno.ECONNRESET, 'fixture reset')]:
    client, peer = socket.socketpair()
    with client, peer:
        client.settimeout(5)
        peer.sendall(denial)
        peer.close()
        probe(client if failure is None else WriteFailure(client, failure), request, expected)

# Also require the ordinary send-before-rejection path to keep working.
client, peer = socket.socketpair()
def reject_after_request():
    with peer:
        peer.settimeout(5)
        assert read_frame(peer) == request
        peer.sendall(denial)
thread = threading.Thread(target=reject_after_request)
thread.start()
with client:
    client.settimeout(5)
    probe(client, request, expected)
thread.join(timeout=5)
assert not thread.is_alive()

# A disconnect, partial frame, success, wrong message or unrelated I/O error
# must still fail, even if an early write-side close accompanies it.
for payload, failure in [
    (b'', None), (denial[:3], None), (denial[:-1], None),
    (struct.pack('>I', 0), None),
    (struct.pack('>I', 4 * 1024 * 1024 + 1), None),
    (frame({'kind':'sessions','data':[{'name':expected}]}), None),
    (frame({'kind':'error','data':'unrelated error'}), None),
    (frame({'kind':'error','data':expected}, b'O'), None),
    (denial, OSError(errno.EIO, 'fixture I/O error')),
]:
    client, peer = socket.socketpair()
    with client, peer:
        client.settimeout(5)
        peer.sendall(payload)
        peer.close()
        try:
            probe(client if failure is None else WriteFailure(client, failure), request, expected)
        except (AssertionError, OSError, ValueError):
            pass
        else:
            raise AssertionError('Probe accepted missing or invalid denial')
"#,
        ])
        .arg(include_str!("fixtures/control_rejection.py"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
#[cfg(target_os = "linux")]
fn host_control_rejects_missing_and_incorrect_authentication_before_requests() {
    use std::os::{
        linux::net::SocketAddrExt,
        unix::{
            fs::MetadataExt,
            net::{SocketAddr, UnixStream},
        },
    };
    let mut fixture = Fixture::new();
    fixture.create(&fixture.plan());
    let metadata = fs::metadata(&fixture.runtime).unwrap();
    let address = SocketAddr::from_abstract_name(format!(
        "rode-session-auth-{}-{}-{}",
        metadata.uid(),
        metadata.dev(),
        metadata.ino()
    ))
    .unwrap();
    for bytes in [
        b"J{\"action\":\"list\"}".to_vec(),
        [vec![b'A'], vec![0; 32]].concat(),
    ] {
        let mut stream = UnixStream::connect_addr(&address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        if let Err(error) = stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .and_then(|()| stream.write_all(&bytes))
        {
            // A bad length can be rejected before the body write. Still
            // require the typed authentication error from this same socket.
            assert!(
                matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                ),
                "{error}"
            );
        }
        let (kind, bytes) = rode_session::read_stream(&mut stream).unwrap();
        assert_eq!(kind, b'J');
        let response = serde_json::from_slice::<Response>(&bytes)
            .unwrap()
            .checked();
        assert!(response.unwrap_err().to_string().contains("authentication"));
    }
    assert_eq!(fixture.direct_list().len(), 1);
}

#[test]
fn concurrent_rode_clients_start_one_service_and_keep_independent_sessions() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let policy = sessions::fingerprint(&plan);
    let ids: Vec<_> = (0..8).map(|_| rode_session::new_id().unwrap()).collect();
    // Register cleanup before starting workers, including when one panics after
    // other clients have already created persistent shells.
    fixture.ids = ids.iter().map(|id| (id.clone(), policy.clone())).collect();
    let start = std::sync::Barrier::new(ids.len());
    std::thread::scope(|scope| {
        for id in &ids {
            let fixture = &fixture;
            let plan = &plan;
            let policy = &policy;
            let start = &start;
            scope.spawn(move || {
                start.wait();
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

#[test]
fn dec_graphics_survive_native_session_reattachment_and_continued_output() {
    for standalone in [false, true] {
        let mut fixture = Fixture::new();
        fixture.standalone = standalone;
        let info = fixture.create(&fixture.plan());
        let terminal = fixture.attach(&info);
        terminal
            .send(b"printf '\\033)0\\016lqqk'\n".to_vec())
            .unwrap();
        wait(|| terminal.contents().contains("┌──┐"));
        drop(terminal);
        wait(|| {
            fixture
                .direct_list()
                .iter()
                .all(|session| session.clients == 0)
        });
        let terminal = fixture.attach(&info);
        wait(|| terminal.contents().contains("┌──┐"));
        terminal
            .send(b"printf '\\r\\033[2Kqq\\017plain-q\\n'\n".to_vec())
            .unwrap();
        wait(|| terminal.contents().contains("──plain-q"));
        drop(terminal);
    }
}

#[test]
fn local_tunnels_need_neither_a_project_nor_an_ssh_client() {
    const CHILD: &str = "RODE_TEST_LOCAL_TUNNEL_CONTROL";
    if std::env::var_os(CHILD).is_some() {
        let runtime = PathBuf::from(std::env::var_os("RODE_SESSION_DIR").unwrap());
        if std::env::var(CHILD).as_deref() == Ok("missing-helper") {
            let error = sessions::local(&sessions::Operation::Tunnels {
                request: Box::new(Request::Tunnels),
            })
            .unwrap_err();
            let message = format!("{error:#}");
            let helper = std::env::var("RODE_SESSION_HELPER").unwrap();
            assert!(message.contains(&helper), "{message}");
            assert!(message.contains("RODE_SESSION_HELPER"), "{message}");
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::NotFound
            );
            assert!(!runtime.join("socket").exists());
            return;
        }
        let control = |request| {
            sessions::local(&sessions::Operation::Tunnels {
                request: Box::new(request),
            })
            .unwrap()
            .response
            .checked()
            .unwrap()
        };
        assert!(matches!(control(Request::Tunnels), Response::Tunnels(list) if list.is_empty()));
        assert!(
            !runtime.join("socket").exists(),
            "Discovery must not start sessiond"
        );
        // Reserve a loopback port without accepting SSH. This keeps the service's
        // native worker in a connection attempt, independently of host credentials.
        let relay = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let id = rode_session::new_id().unwrap();
        let spec = rode_session::tunnels::Spec {
            relay_host: "127.0.0.1".into(),
            relay_port: relay.local_addr().unwrap().port(),
            relay_user: "fixture".into(),
            identity: runtime.join("unused-relay-key").to_string_lossy().into(),
            ..Default::default()
        };
        assert!(
            matches!(control(Request::StartTunnel { id: id.clone(), spec }), Response::Tunnel(info) if info.id == id && !info.hostname.is_empty())
        );
        assert!(runtime.join("socket").exists());
        assert!(
            matches!(control(Request::Tunnels), Response::Tunnels(list) if list.len() == 1 && list[0].id == id)
        );
        control(Request::StopTunnel { id });
        assert!(matches!(control(Request::Tunnels), Response::Tunnels(list) if list.is_empty()));
        return;
    }
    for mode in ["service", "missing-helper"] {
        let fixture = Fixture::new();
        let helper = if mode == "missing-helper" {
            fixture.root.path().join("missing-session-helper")
        } else {
            PathBuf::from(env!("CARGO_BIN_EXE_rode-sessiond"))
        };
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_tunnels_need_neither_a_project_nor_an_ssh_client",
                "--nocapture",
            ])
            .env(CHILD, mode)
            .env("RODE_SESSION_DIR", &fixture.runtime)
            .env("RODE_SESSION_HELPER", helper)
            .env("PATH", "")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .current_dir(fixture.root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn retained_tunnels_keep_sessiond_alive_after_last_terminal_and_stop_explicitly() {
    let mut fixture = Fixture::new();
    fixture.standalone = true;
    let vacant = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = vacant.local_addr().unwrap().port();
    drop(vacant);
    let id = rode_session::new_id().unwrap();
    let spec = rode_session::tunnels::Spec {
        relay_host: "127.0.0.1".into(),
        relay_port: port,
        relay_user: "fixture".into(),
        identity: fixture
            .root
            .path()
            .join("unused-relay-key")
            .to_string_lossy()
            .into(),
        ..Default::default()
    };
    assert!(
        matches!(fixture.rpc(&Request::Tunnels).unwrap(), Response::Tunnels(list) if list.is_empty())
    );
    assert!(
        !fixture.runtime.join("socket").exists(),
        "Listing must not launch a service"
    );
    fixture
        .rpc(&Request::StartTunnel {
            id: id.clone(),
            spec,
        })
        .unwrap();
    // Both explicit End and last-view cleanup must retain a tunnel-only daemon.
    let info = fixture.create(&fixture.plan());
    fixture
        .rpc(&Request::End {
            id: info.id.clone(),
            policy: info.policy.clone(),
        })
        .unwrap();
    let info = fixture.create_with_persistence(&fixture.plan(), false);
    let terminal = fixture.attach(&info);
    wait(|| {
        fixture
            .direct_list()
            .iter()
            .any(|session| session.id == info.id && session.clients == 1)
    });
    drop(terminal);
    wait(|| fixture.direct_list().is_empty());
    assert!(
        fixture
            .rpc(&Request::Shutdown)
            .unwrap_err()
            .to_string()
            .contains("tunnels")
    );
    std::thread::sleep(Duration::from_secs(6));
    assert!(
        matches!(fixture.rpc(&Request::Tunnels).unwrap(), Response::Tunnels(list) if list.len() == 1 && list[0].id == id)
    );
    let service = std::fs::read_to_string(fixture.runtime.join("service.log")).unwrap_or_default();
    assert!(!service.contains("panicked"));
    fixture.rpc(&Request::StopTunnel { id }).unwrap();
    fixture.rpc(&Request::Shutdown).unwrap();
}

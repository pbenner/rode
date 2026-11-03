//! Deterministic scheduler parsing, exact-ID ownership checks and cancellation argument
//! isolation; no real allocation is cancelled.

#[path = "support/native.rs"]
mod native_peer;

use rode::{
    files,
    project::{Request, Response},
    slurm,
};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};
fn execute(request: &Request, peer: bool) -> anyhow::Result<Response> {
    let mut command = if peer {
        native_peer::command()
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
        command.arg("--file-helper");
        command
    };
    let mut child = command
        .env("SCANCEL_SIGNAL", "KILL")
        .env("SQUEUE_STATES", "PENDING")
        .env("SLURM_CLUSTERS", "must-not-leak")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&files::Request::Tool {
            request: request.clone(),
        })?)?;
    let out = child.wait_with_output()?;
    anyhow::ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(serde_json::from_slice::<files::Response>(&out.stdout)?
        .tool
        .unwrap())
}
#[test]
fn interactive_compute_uses_the_headless_helper_after_the_desktop_image_is_released() {
    let root = tempfile::tempdir().unwrap();
    let tools = root.path().join("tools");
    fs::create_dir(&tools).unwrap();
    for name in ["salloc", "srun"] {
        let path = tools.join(name);
        fs::write(&path, include_str!("fixtures/slurm.py")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let state = root.path().join("slurm-state");
    fs::create_dir(&state).unwrap();
    fs::write(state.join("grant"), "").unwrap();
    let mise = tools.join("mise");
    fs::write(&mise, "#!/bin/sh\nprintf '%s\\n' '{}'\n").unwrap();
    fs::set_permissions(&mise, fs::Permissions::from_mode(0o700)).unwrap();
    let helper = rode::sandbox::HelperBinary::new().unwrap();
    let desktop = helper.path.clone();
    let plan = rode::sandbox::Plan {
        root: root.path().to_string_lossy().into(),
        native_helper: Some(desktop.to_string_lossy().into()),
        helper: Some(helper),
        environment: rode::environment::Settings {
            mise: true,
            runtimes: std::collections::BTreeMap::from([(
                "mise".into(),
                mise.to_string_lossy().into(),
            )]),
            ..Default::default()
        },
        ..Default::default()
    };
    let allocation = slurm::Allocation {
        allocator: tools.join("salloc").to_string_lossy().into(),
        runner: tools.join("srun").to_string_lossy().into(),
        ..Default::default()
    };
    let launch = allocation
        .launch(&plan, &rode_session::new_id().unwrap())
        .unwrap();
    drop(plan);
    assert!(!desktop.exists());
    let mut command = Command::new(&launch.argv[0]);
    command
        .args(&launch.argv[1..])
        .current_dir(root.path())
        .env("SHELL", "/bin/true");
    rode::project::run(command, vec![], std::time::Duration::from_secs(10))
        .unwrap()
        .checked()
        .unwrap();
    assert!(state.join("ended").exists());
}

#[test]
fn slurm_tres_counts_do_not_double_count_typed_gpu_resources() {
    for (tres, expected) in [
        ("cpu=16,gres/gpu=4,gres/gpu:a100=4", Some(4)),
        ("gres/gpu:a100=2,gres/gpu:h100=3", Some(5)),
        ("cpu=16,mem=64G", Some(0)),
        ("", None),
        ("N/A", None),
        ("gres/gpu=bad", None),
    ] {
        assert_eq!(slurm::gpu_count(tres), expected);
    }
    for id in ["12", "12_3", "12+1", "12_3+1"] {
        assert!(slurm::valid_id(id));
    }
    for id in [
        "",
        "--all",
        "12.0",
        "12,13",
        "12_[1-3]",
        "12;echo bad",
        "12_",
        "12++3",
    ] {
        assert!(!slurm::valid_id(id), "{id}");
    }
}
#[test]
fn native_and_remote_slurm_queries_and_releases_use_exact_owned_allocations() {
    let root = tempfile::tempdir().unwrap();
    let queue = root.path().join("queue");
    let cancel = root.path().join("cancel");
    let log = root.path().join("cancelled");
    fs::write(&queue,"#!/usr/bin/python3\nimport os,sys\nassert 'SLURM_CLUSTERS' not in os.environ\nassert 'SQUEUE_STATES' not in os.environ\nassert sys.argv[sys.argv.index('--user')+1]==str(os.geteuid())\nassert '--local' in sys.argv\nprint('12|RUNNING|2:30|1:00:00|gpu01|cpu=16,gres/gpu=4,gres/gpu:a100=4|training')\nprint('13|PENDING|0:00|2:00:00|(Resources)|gres/gpu=8|waiting')\n").unwrap();
    fs::write(&cancel,format!("#!/usr/bin/python3\nimport os,sys,json\nassert 'SCANCEL_SIGNAL' not in os.environ\nassert 'SLURM_CLUSTERS' not in os.environ\nassert sys.argv[1:4]==['--user',str(os.geteuid()),'--']\nassert sys.argv[4] in ('12','13')\nopen({:?},'w').write(json.dumps(sys.argv[1:]))\n",log.to_str().unwrap())).unwrap();
    for path in [&queue, &cancel] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let request = Request::Slurm {
        root: root.path().to_string_lossy().into(),
        queue: queue.to_string_lossy().into(),
        cancel_program: cancel.to_string_lossy().into(),
        cancel: None,
    };
    let mut previous = None;
    for peer in [false, true] {
        let Response::Slurm(snapshot) = execute(&request, peer).unwrap() else {
            panic!()
        };
        assert_eq!(snapshot.jobs[0].gpus, Some(4));
        assert!(snapshot.jobs[0].allocated());
        assert!(!snapshot.jobs[1].allocated());
        if let Some(previous) = previous {
            assert_eq!(serde_json::to_value(&snapshot).unwrap(), previous);
        }
        previous = Some(serde_json::to_value(snapshot).unwrap());
        for id in ["99", "12.0", "--all"] {
            let mut bad = request.clone();
            if let Request::Slurm { cancel, .. } = &mut bad {
                *cancel = Some(id.into());
            }
            assert!(execute(&bad, peer).is_err());
            assert!(!log.exists());
        }
        for id in ["12", "13"] {
            let mut release = request.clone();
            if let Request::Slurm { cancel, .. } = &mut release {
                *cancel = Some(id.into());
            }
            let Response::Command(result) = execute(&release, peer).unwrap() else {
                panic!()
            };
            assert!(result.success());
            let args: Vec<String> = serde_json::from_slice(&fs::read(&log).unwrap()).unwrap();
            assert_eq!(args.last().unwrap(), id);
            fs::remove_file(&log).unwrap();
        }
    }
}

#[test]
fn allocation_fields_are_literal_and_compute_sandbox_failure_never_falls_back() {
    let root = tempfile::tempdir().unwrap();
    let allocator = root.path().join("allocator");
    let runner = root.path().join("runner");
    let options = root.path().join("options.json");
    fs::write(&allocator, format!("#!/usr/bin/python3\nimport json,os,sys\nargs=sys.argv[1:]\ni=next(i for i,v in enumerate(args) if not v.startswith('--'))\nopen({:?},'w').write(json.dumps(args[:i]))\nos.environ['SLURM_JOB_ID']='123'\nos.execv(args[i],args[i:])\n", options.to_str().unwrap())).unwrap();
    fs::write(
        &runner,
        "#!/usr/bin/python3\nimport os,sys\nos.execv(sys.argv[5],sys.argv[5:])\n",
    )
    .unwrap();
    for path in [&allocator, &runner] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let settings = slurm::Allocation {
        allocator: allocator.to_string_lossy().into(),
        runner: runner.to_string_lossy().into(),
        account: "science ' $(touch injected) ; echo wrong".into(),
        constraint: "gpu&fast".into(),
        time: "2:30".into(),
        ..Default::default()
    };
    let plan = rode::sandbox::Plan {
        active: true,
        prefix: vec!["/bin/false".into()],
        root: root.path().to_string_lossy().into(),
        ..Default::default()
    };
    let id = rode_session::new_id().unwrap();
    let launch = settings.launch(&plan, &id).unwrap();
    let output = Command::new(&launch.argv[0])
        .args(&launch.argv[1..])
        .current_dir(root.path())
        .env("SHELL", "/bin/sh")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!root.path().join("injected").exists());
    let args: Vec<String> = serde_json::from_slice(&fs::read(options).unwrap()).unwrap();
    assert!(args.contains(&format!("--account={}", settings.account)));
    assert!(args.contains(&"--constraint=gpu&fast".into()));
    assert!(args.contains(&"--time=2:30:00".into()));
    assert!(args.contains(&"--mem=80G".into()));
    assert!(args.contains(&"--gpus=1".into()));
    for invalid in [
        slurm::Allocation {
            nodes: 0,
            ..Default::default()
        },
        slurm::Allocation {
            allocator: "--no-shell".into(),
            ..Default::default()
        },
        slurm::Allocation {
            account: "line\nbreak".into(),
            ..Default::default()
        },
        slurm::Allocation {
            time: "2:99".into(),
            ..Default::default()
        },
        slurm::Allocation {
            time: "0".into(),
            ..Default::default()
        },
        slurm::Allocation {
            memory: "0".into(),
            ..Default::default()
        },
    ] {
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn notebook_steps_revalidate_identity_preserve_stdio_and_enforce_compute_policy() {
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
    let job =
        serde_json::json!({"id":"4242", "state":"RUNNING", "name":"a ' $(touch injected) | job"});
    fs::write(&record, job.to_string()).unwrap();
    let target = slurm::Target {
        id: "4242".into(),
        name: job["name"].as_str().unwrap().into(),
        queue: "tools/squeue".into(),
        runner: "tools/srun".into(),
    };
    let source =
        std::env::var("RODE_TEST_SESSION_HELPER").unwrap_or_else(|_| native_peer::binary().into());
    let helper = root.path().join("compute-helper");
    fs::copy(source, &helper).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let plan = rode::sandbox::Plan {
        root: root.path().to_string_lossy().into(),
        native_helper: Some(helper.to_string_lossy().into()),
        ..Default::default()
    };
    let runtime = root.path().join("fixture-runtime");
    fs::write(&runtime, "#!/usr/bin/python3\nimport json,os,sys\nrequest=json.loads(sys.stdin.readline())\nprint(json.dumps(dict(request=request,job=os.environ['SLURM_JOB_ID'],gpu=os.environ['CUDA_VISIBLE_DEVICES'],cwd=os.getcwd())))\n").unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
    let argv = target
        .notebook_argv(&plan, runtime.to_str().unwrap())
        .unwrap();
    let mut payload: serde_json::Value = serde_json::from_str(argv.last().unwrap()).unwrap();
    let execute = |argv: &[String], peer: bool| {
        let mut command = if peer {
            let mut cmd = Command::new(native_peer::binary());
            cmd.arg("--process");
            cmd
        } else {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..]);
            cmd
        };
        let mut child = command
            .current_dir(root.path())
            .env("SLURM_JOB_ID", "stale")
            .env("CUDA_VISIBLE_DEVICES", "wrong")
            .env("SALLOC_NO_SHELL", "1")
            .env("SRUN_EXPORT_ENV", "NONE")
            .env("SLURM_CLUSTERS", "wrong-cluster")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        if peer {
            writeln!(input, "{}", serde_json::json!({"root":plan.root,"prefix":[],"argv":argv,"python":"off","python_program":false,"environment":{}})).unwrap();
        }
        writeln!(input, "{{\"code\":\"hello\"}}").unwrap();
        // Keep the peer's request channel open until its owned child has replied.
        let output = child.wait_with_output().unwrap();
        drop(input);
        output
    };
    for peer in [false, true] {
        let output = execute(&argv, peer);
        assert!(
            output.status.success(),
            "peer={peer}, status={}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["job"], "4242");
        assert_eq!(response["gpu"], "0,1");
        assert_eq!(response["request"]["code"], "hello");
        assert_eq!(response["cwd"], root.path().to_string_lossy().as_ref());
        assert!(!root.path().join("injected").exists());

        for changed in [
            serde_json::json!({"id":"4242", "state":"PENDING", "name":job["name"]}),
            serde_json::json!({"id":"4242", "state":"RUNNING", "name":"reused id"}),
        ] {
            fs::remove_file(state.join("step.json")).unwrap();
            fs::write(&record, changed.to_string()).unwrap();
            let failed = execute(&argv, peer);
            assert!(!failed.status.success());
            assert!(!state.join("step.json").exists());
            assert!(String::from_utf8_lossy(&failed.stderr).contains("no longer your running job"));
            fs::write(&record, job.to_string()).unwrap();
            // Restore the marker for the next refusal case.
            fs::write(state.join("step.json"), "").unwrap();
        }
        fs::remove_file(&record).unwrap();
        assert!(!execute(&argv, peer).status.success());
        fs::write(&record, job.to_string()).unwrap();
        let mut sandboxed = argv.clone();
        payload["prefix"] = serde_json::json!(["/bin/false"]);
        *sandboxed.last_mut().unwrap() = payload.to_string();
        let failed = execute(&sandboxed, peer);
        assert!(!failed.status.success());
        assert!(failed.stdout.is_empty());
    }
    for id in ["12_3", "12+1", "--all", "12.0"] {
        let mut invalid = target.clone();
        invalid.id = id.into();
        assert!(invalid.notebook_argv(&plan, "off").is_err());
    }
}

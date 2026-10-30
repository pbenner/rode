//! Deterministic scheduler parsing, exact-ID ownership checks and cancellation argument
//! isolation; no real allocation is cancelled.

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
        let mut command = Command::new("python3");
        command.args(["-c", files::REMOTE_HELPER]);
        command
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
    fs::write(&cancel,format!("#!/usr/bin/python3\nimport os,sys,json\nassert 'SCANCEL_SIGNAL' not in os.environ\nassert 'SLURM_CLUSTERS' not in os.environ\nassert sys.argv[1:]==['--user',str(os.geteuid()),'--','12']\nopen({:?},'w').write(json.dumps(sys.argv[1:]))\n",log.to_str().unwrap())).unwrap();
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
        let mut release = request.clone();
        if let Request::Slurm { cancel, .. } = &mut release {
            *cancel = Some("12".into());
        }
        let Response::Command(result) = execute(&release, peer).unwrap() else {
            panic!()
        };
        assert!(result.success());
        assert!(log.exists());
        fs::remove_file(&log).unwrap();
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
    let mut plan = rode::sandbox::Plan::default();
    plan.active = true;
    plan.prefix = vec!["/bin/false".into()];
    plan.root = root.path().to_string_lossy().into();
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

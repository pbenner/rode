//! Headless native protocol and policy regressions. CI supplies its release/static helper.
use rode::{files, project, sandbox};
use rode_host_protocol::{self as wire, framing};
use rode_session::host::{FileRequest, Operation, Response};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

fn executable(path: &Path, source: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn installed_helper_discovery_checks_paths_before_execution() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let bin = root.path().join("installed ' helpers");
    fs::create_dir(&bin).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    let helper = bin.join("rode-sessiond");
    let marker = root.path().join("executed");
    executable(
        &helper,
        &format!(
            "#!/bin/sh\ntest \"$1\" = --host-info || exit 2\nprintf '{{\"probe\":true}}'\ntouch {}\n",
            rode::ssh::quote(marker.to_str().unwrap())
        ),
    );
    let probe = |path: &str| {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                include_str!("../src/host/installed.sh"),
                "installed",
                path,
            ])
            .env("HOME", root.path())
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        project::run(command, vec![], Duration::from_secs(5))
            .unwrap()
            .checked()
    };
    let candidates = probe("--list").unwrap();
    assert!(candidates.starts_with("RODE-HELPERS\0"));
    assert!(
        candidates
            .split('\0')
            .any(|s| s == helper.to_str().unwrap())
    );
    assert!(!marker.exists(), "Listing helpers must not execute them");
    assert_eq!(probe(helper.to_str().unwrap()).unwrap(), "{\"probe\":true}");
    assert!(marker.exists());
    fs::remove_file(&marker).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        probe(helper.to_str().unwrap())
            .unwrap_err()
            .to_string()
            .contains("writable")
    );
    assert!(
        !marker.exists(),
        "Unsafe helper was executed before its permissions were checked"
    );
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o4700)).unwrap();
    assert!(
        probe(helper.to_str().unwrap())
            .unwrap_err()
            .to_string()
            .contains("setuid")
    );
    assert!(!marker.exists());
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(probe(helper.to_str().unwrap()).is_err());
    assert!(!marker.exists());
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&helper, &alias).unwrap();
    assert!(probe("~/alias").is_ok());
    assert!(probe("relative-helper").is_err());
}

#[test]
fn installed_helper_snapshot_preserves_packages_and_pins_live_generations() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let helper = root.path().join("installed helper ' source");
    let original = b"original executable fixture bytes";
    fs::write(&helper, original).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let digest = files::digest(original);
    let publish = |digest: &str, size: usize| {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                include_str!("../src/host/bootstrap.sh"),
                "bootstrap",
                digest,
                &size.to_string(),
                helper.to_str().unwrap(),
            ])
            .env("HOME", root.path());
        // No executable bytes arrive from the desktop in installed mode.
        project::run(command, vec![], Duration::from_secs(5))
            .unwrap()
            .checked()
    };
    assert!(publish(&"0".repeat(64), original.len()).is_err());
    assert!(publish(&digest, original.len() + 1).is_err());
    let published = publish(&digest, original.len()).unwrap();
    let cached = PathBuf::from(published.trim());
    assert_eq!(fs::read(&helper).unwrap(), original);
    assert_eq!(fs::read(&cached).unwrap(), original);
    assert_eq!(
        fs::metadata(&helper).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&cached).unwrap().permissions().mode() & 0o777,
        0o500
    );
    fs::write(&helper, b"upgraded package").unwrap();
    assert_eq!(publish(&digest, original.len()).unwrap(), published);
    assert_eq!(fs::read(&cached).unwrap(), original);
    assert_eq!(fs::read(&helper).unwrap(), b"upgraded package");
    fs::remove_file(&cached).unwrap();
    assert!(
        publish(&digest, original.len()).is_err(),
        "Changed installation was silently accepted"
    );
}

#[test]
fn installed_helper_reports_the_same_contract_as_its_rpc_handshake() {
    let Some(helper) = helper() else { return };
    let output = Command::new(&helper).arg("--host-info").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let info: wire::helper::Installed = serde_json::from_slice(&output.stdout).unwrap();
    info.validate("linux", std::env::consts::ARCH, rode_session::PROTOCOL)
        .unwrap();
    assert_eq!(info.digest, files::digest(&fs::read(&helper).unwrap()));
    assert_eq!(
        PathBuf::from(info.executable),
        helper.canonicalize().unwrap()
    );
    let _gateway = Gateway::start(&helper);
}

#[test]
fn native_handshake_rejects_wrong_version_identity_and_oversized_frames() {
    let Some(helper) = helper() else { return };
    let digest = files::digest(&fs::read(&helper).unwrap());
    for case in ["version", "digest", "architecture", "frame"] {
        let mut input = framing::MAGIC.to_vec();
        if case == "frame" {
            input.extend_from_slice(&u32::MAX.to_be_bytes());
        } else {
            framing::write(
                &mut input,
                &wire::Hello {
                    version: if case == "version" {
                        999
                    } else {
                        framing::VERSION
                    },
                    digest: if case == "digest" {
                        "0".repeat(64)
                    } else {
                        digest.clone()
                    },
                    architecture: if case == "architecture" {
                        "wrong"
                    } else {
                        std::env::consts::ARCH
                    }
                    .into(),
                    capabilities: vec![],
                    info: None,
                },
            )
            .unwrap();
        }
        let mut command = Command::new(&helper);
        command.arg("host-rpc");
        let result = project::run(command, input, Duration::from_secs(5)).unwrap();
        assert!(!result.success(), "Accepted invalid {case}");
    }
}

#[test]
fn native_runtime_cache_verifies_bytes_and_preserves_existing_artifacts() {
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    let Some(helper) = helper() else { return };
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let cache = root.path().join("runtime cache ' 🦀");
    let bytes = fs::read("/usr/bin/true").unwrap();
    let mut spec = json!({
        "action":"install", "version":"0-native-test",
        "targets": {std::env::consts::ARCH: {
            "size": bytes.len(), "sha256": files::digest(&bytes)
        }},
        "notices": [{"file":"LICENSE", "sha256":files::digest(b"fixture\n")}],
        "notice_data": {"LICENSE":"fixture\n"}
    });
    let call = |spec: &serde_json::Value, payload: &[u8]| {
        let mut command = Command::new(&helper);
        command
            .arg("--runtime-cache")
            .env("RODE_CODEX_CACHE", &cache);
        let mut input = serde_json::to_vec(spec).unwrap();
        input.push(b'\n');
        input.extend_from_slice(payload);
        project::run(command, input, Duration::from_secs(10))
            .unwrap()
            .checked()
    };
    assert!(call(&spec, b"incomplete").is_err());
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| call(&spec, &bytes));
        let b = scope.spawn(|| call(&spec, &bytes));
        [a.join().unwrap().unwrap(), b.join().unwrap().unwrap()]
    });
    let reply: serde_json::Value = serde_json::from_str(&results[0]).unwrap();
    let installed = PathBuf::from(reply["executable"].as_str().unwrap());
    assert_eq!(fs::read(&installed).unwrap(), bytes);
    assert!(Command::new(&installed).status().unwrap().success());
    spec["action"] = "probe".into();
    let reply: serde_json::Value = serde_json::from_str(&call(&spec, &[]).unwrap()).unwrap();
    assert_eq!(reply["cached"], true);
    fs::write(&installed, b"tampered").unwrap();
    assert!(call(&spec, &[]).is_err());
    spec["action"] = "install".into();
    assert!(call(&spec, &bytes).is_err());
    assert_eq!(fs::read(&installed).unwrap(), b"tampered");
    fs::remove_file(&installed).unwrap();
    std::os::unix::fs::symlink("/usr/bin/true", &installed).unwrap();
    assert!(call(&spec, &bytes).is_err());
    assert!(installed.is_symlink());
    let parent = installed.parent().unwrap().parent().unwrap();
    assert!(!fs::read_dir(parent).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".incoming-")
    }));
}

#[test]
fn native_runtime_upload_cancellation_removes_only_its_staging_directory() {
    use std::os::unix::fs::PermissionsExt;
    let Some(helper) = helper() else { return };
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let cache = root.path().join("cache");
    let folder = cache.join("0-cancel").join(std::env::consts::ARCH);
    let spec = serde_json::json!({"action":"install", "version":"0-cancel", "targets": {
        std::env::consts::ARCH: {"size": 65536, "sha256": "0".repeat(64)}
    }});
    let mut child = Command::new(helper)
        .arg("--runtime-cache")
        .env("RODE_CODEX_CACHE", &cache)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{spec}").unwrap();
    input.write_all(b"partial").unwrap();
    let staged = || {
        fs::read_dir(&folder).is_ok_and(|mut entries| {
            entries.any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".incoming-")
            })
        })
    };
    let until = Instant::now() + Duration::from_secs(5);
    while !staged() {
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Upload did not enter staging");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    fs::write(folder.join("unrelated"), "preserve").unwrap();
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let until = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Stalled upload ignored cancellation");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(input);
    assert!(!staged());
    assert_eq!(
        fs::read_to_string(folder.join("unrelated")).unwrap(),
        "preserve"
    );
}

#[test]
fn native_bootstrap_rejects_writable_home_ancestors_before_creating_its_cache() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let shared = root.path().join("shared");
    let home = shared.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    let payload = b"verified helper fixture, never executed";
    let digest = files::digest(payload);
    let call = || {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                include_str!("../src/host/bootstrap.sh"),
                "bootstrap",
                &digest,
                &payload.len().to_string(),
            ])
            .env("HOME", &home);
        project::run(command, payload.to_vec(), Duration::from_secs(5))
            .unwrap()
            .checked()
    };
    for mode in [0o777, 0o770] {
        fs::set_permissions(&shared, fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            call()
                .unwrap_err()
                .to_string()
                .contains("ancestor is writable")
        );
        assert!(!home.join(".rode-host").exists());
    }
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
    let published = PathBuf::from(call().unwrap().trim());
    assert_eq!(fs::read(&published).unwrap(), payload);
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(PathBuf::from(call().unwrap().trim()), published);
}

#[test]
fn native_bootstrap_verifies_before_publication_and_preserves_suspect_cache() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let payload = b"bootstrap fixture, never executed";
    let digest = files::digest(payload);
    let call = |hash: &str, bytes: &[u8]| {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                include_str!("../src/host/bootstrap.sh"),
                "bootstrap",
                hash,
                &payload.len().to_string(),
            ])
            .env("HOME", root.path());
        project::run(command, bytes.to_vec(), Duration::from_secs(5))
            .unwrap()
            .checked()
    };
    assert!(call(&"0".repeat(64), payload).is_err());
    assert!(call(&digest, b"short").is_err());
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| call(&digest, payload));
        let b = scope.spawn(|| call(&digest, payload));
        [a.join().unwrap().unwrap(), b.join().unwrap().unwrap()]
    });
    assert_eq!(results[0], results[1]);
    let cached = PathBuf::from(results[0].trim());
    assert_eq!(fs::read(&cached).unwrap(), payload);
    assert_eq!(
        fs::metadata(&cached).unwrap().permissions().mode() & 0o777,
        0o500
    );
    fs::set_permissions(&cached, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&cached, "suspect").unwrap();
    assert!(call(&digest, payload).is_err());
    assert_eq!(fs::read_to_string(&cached).unwrap(), "suspect");
    assert!(
        !fs::read_dir(root.path().join(".rode-host"))
            .unwrap()
            .any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".upload."))
    );
}

#[test]
fn native_slurm_revalidates_compute_identity_helper_and_captured_policy() {
    use std::os::unix::fs::PermissionsExt;
    let Some(helper) = helper() else { return };
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("project ' 🦀");
    fs::create_dir(&root).unwrap();
    let copy = base.path().join("shared-helper");
    fs::copy(&helper, &copy).unwrap();
    fs::set_permissions(&copy, fs::Permissions::from_mode(0o500)).unwrap();
    let secret = base.path().join("secret");
    fs::write(&secret, "outside").unwrap();
    std::os::unix::fs::symlink(secret, root.join("escape")).unwrap();
    let queue = root.join("squeue");
    let runner = root.join("srun");
    let python = root.join("runtime");
    executable(
        &queue,
        "#!/bin/sh\n[ -z \"${CUDA_VISIBLE_DEVICES:-}${SLURM_JOB_ID:-}${SQUEUE_STATES:-}\" ] || exit 91\nprintf '42|RUNNING|native-fixture\\n'\n",
    );
    executable(
        &runner,
        "#!/bin/sh\n[ \"$1\" = --jobid=42 ] || exit 92\n[ -z \"${CUDA_VISIBLE_DEVICES:-}${SLURM_JOB_ID:-}\" ] || exit 93\nshift 5\nexport SLURM_JOB_ID=${FIXTURE_JOB:-42} CUDA_VISIBLE_DEVICES=5\nexec \"$@\"\n",
    );
    // This is an interpreter stand-in, so the scheduler/policy contract needs no Jupyter install.
    executable(
        &python,
        "#!/bin/sh\n[ \"$SLURM_JOB_ID\" = 42 ] && [ \"$CUDA_VISIBLE_DEVICES\" = 5 ] || exit 94\nif [ \"${FIXTURE_DENY:-}\" = 1 ] && cat escape >/dev/null 2>&1; then exit 95; fi\nprintf 'compute-runtime-ready\\n'\n",
    );
    let target = rode::slurm::Target {
        id: "42".into(),
        name: "native-fixture".into(),
        queue: queue.to_string_lossy().into(),
        runner: runner.to_string_lossy().into(),
    };
    let mut gateway = Gateway::start(&helper);
    for isolated in [false, true] {
        let mut plan = gateway.prepare(
            &root,
            sandbox::Options {
                enabled: isolated,
                backend: sandbox::Backend::Landlock,
                read_only: vec![copy.to_string_lossy().into()],
                ..Default::default()
            },
        );
        if isolated && !plan.active {
            assert_ne!(
                std::env::var("RODE_TEST_REQUIRE_LANDLOCK").as_deref(),
                Ok("1"),
                "{}",
                plan.reason
            );
            eprintln!("Compute Landlock unavailable: {}", plan.reason);
            continue;
        }
        plan.native_helper = Some(copy.to_string_lossy().into());
        let run = |plan: &sandbox::Plan, job: &str| {
            let argv = target
                .notebook_argv(plan, python.to_str().unwrap())
                .unwrap();
            let mut command = Command::new(&argv[0]);
            command
                .args(&argv[1..])
                .env("SLURM_JOB_ID", "stale")
                .env("CUDA_VISIBLE_DEVICES", "wrong")
                .env("SQUEUE_STATES", "wrong")
                .env("FIXTURE_JOB", job)
                .env("FIXTURE_DENY", if isolated { "1" } else { "0" });
            project::run(command, vec![], Duration::from_secs(10))
                .unwrap()
                .checked()
        };
        assert_eq!(run(&plan, "42").unwrap().trim(), "compute-runtime-ready");
        assert!(
            run(&plan, "43")
                .unwrap_err()
                .to_string()
                .contains("identity mismatch")
        );
        let mut denied = plan;
        denied.active = true;
        denied.prefix = vec!["/bin/false".into()];
        assert!(run(&denied, "42").is_err());
        denied.prefix.clear();
        assert!(
            target
                .notebook_argv(&denied, python.to_str().unwrap())
                .is_err()
        );
    }
    // A helper changed between the scheduler check and compute execution cannot run.
    executable(
        &runner,
        &format!(
            "#!/bin/sh\nshift 5\nchmod 700 {0}\nprintf 'changed' > {0}\nexport SLURM_JOB_ID=42\nexec \"$@\"\n",
            rode::ssh::quote(copy.to_str().unwrap())
        ),
    );
    let mut plan = gateway.prepare(&root, Default::default());
    plan.native_helper = Some(copy.to_string_lossy().into());
    let argv = target
        .notebook_argv(&plan, python.to_str().unwrap())
        .unwrap();
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    let error = project::run(command, vec![], Duration::from_secs(10))
        .unwrap()
        .checked()
        .unwrap_err();
    assert!(error.to_string().contains("failed verification"), "{error}");
    gateway.close();
}

fn helper() -> Option<PathBuf> {
    let path = std::env::var_os("RODE_HOST_HELPER")
        .or_else(|| std::env::var_os("RODE_TEST_SESSION_HELPER"))
        .map(PathBuf::from);
    if path.is_none() {
        eprintln!(
            "Set RODE_HOST_HELPER to the release/static helper for native host integration tests"
        );
    }
    path
}
struct Gateway {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<wire::Reply<Response>>,
    id: u64,
    _home: tempfile::TempDir,
}
impl Gateway {
    fn start(helper: &Path) -> Self {
        let home = tempfile::tempdir().unwrap();
        let mut child = Command::new(helper)
            .arg("host-rpc")
            .env("HOME", home.path())
            .env("RODE_SESSION_DIR", home.path().join("sessions"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let (ready, handshake) = mpsc::sync_channel(1);
        let (send, replies) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            let mut magic = [0; 12];
            output.read_exact(&mut magic).unwrap();
            assert_eq!(&magic, framing::MAGIC);
            let hello: wire::Hello = framing::read(&mut output).unwrap().unwrap();
            ready.send(hello).unwrap();
            while let Ok(Some(reply)) = framing::read_document(&mut output) {
                if send.send(reply).is_err() {
                    break;
                }
            }
        });
        input.write_all(framing::MAGIC).unwrap();
        let digest = files::digest(&fs::read(helper).unwrap());
        framing::write(
            &mut input,
            &wire::Hello {
                version: framing::VERSION,
                architecture: std::env::consts::ARCH.into(),
                digest: digest.clone(),
                capabilities: vec![],
                info: None,
            },
        )
        .unwrap();
        let hello = handshake.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(hello.version, framing::VERSION);
        assert_eq!(hello.digest, digest);
        hello
            .info
            .unwrap()
            .validate("linux", std::env::consts::ARCH, rode_session::PROTOCOL)
            .unwrap();
        Self {
            child,
            input: Some(input),
            replies,
            id: 0,
            _home: home,
        }
    }
    fn send(&mut self, operation: Operation) -> u64 {
        self.id += 1;
        framing::write_document(
            self.input.as_mut().unwrap(),
            &wire::Request {
                id: self.id,
                operation,
            },
        )
        .unwrap();
        self.id
    }
    fn call(&mut self, operation: Operation) -> Result<Response, String> {
        self.call_with_timeout(operation, Duration::from_secs(20))
    }
    fn call_with_timeout(
        &mut self,
        operation: Operation,
        timeout: Duration,
    ) -> Result<Response, String> {
        let id = self.send(operation);
        let reply = self.replies.recv_timeout(timeout).unwrap();
        assert_eq!(reply.id, id);
        reply.result
    }
    fn prepare(&mut self, root: &Path, options: sandbox::Options) -> sandbox::Plan {
        let Response::Prepared { response } = self
            .call(Operation::Prepare {
                path: root.to_string_lossy().into(),
                options,
            })
            .unwrap()
        else {
            panic!("Expected preparation result")
        };
        response.project.unwrap()
    }
    fn run(
        &mut self,
        plan: &sandbox::Plan,
        request: FileRequest,
    ) -> Result<rode_session::host::FileResponse, String> {
        let Response::Completed { response } = self.call(Operation::Run {
            context: plan.host_context.clone(),
            request,
        })?
        else {
            panic!("Expected operation result")
        };
        Ok(response)
    }
    fn close(&mut self) {
        self.input.take();
        let until = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < until,
                "Native gateway did not stop after EOF"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Gateway {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn native_files_keep_revision_guards_large_payloads_and_connection_identity() {
    let Some(helper) = helper() else { return };
    let mut gateway = Gateway::start(&helper);
    let root = tempfile::tempdir().unwrap();
    let plan = gateway.prepare(root.path(), Default::default());
    let path = root
        .path()
        .join("source ' 🦀.rs")
        .to_string_lossy()
        .into_owned();
    let text = "α native host\n".repeat(250_000);
    let saved = gateway
        .run(
            &plan,
            FileRequest::Write {
                path: path.clone(),
                text: text.clone(),
                expected: None,
            },
        )
        .unwrap();
    assert_eq!(saved.file.revision, files::digest(text.as_bytes()));
    assert_eq!(
        gateway
            .run(&plan, FileRequest::Read { path: path.clone() })
            .unwrap()
            .file
            .text,
        text
    );
    assert!(
        gateway
            .run(
                &plan,
                FileRequest::Write {
                    path: path.clone(),
                    text: "stale".into(),
                    expected: None
                }
            )
            .unwrap_err()
            .contains("conflict")
    );
    assert!(
        gateway
            .call(Operation::Run {
                context: None,
                request: FileRequest::Read { path: path.clone() }
            })
            .is_err()
    );
    let mut another = Gateway::start(&helper);
    assert!(
        another
            .run(
                &plan,
                FileRequest::Write {
                    path: path.clone(),
                    text: "wrong connection".into(),
                    expected: Some(saved.file.revision)
                }
            )
            .unwrap_err()
            .contains("context")
    );
    assert_eq!(fs::read_to_string(path).unwrap(), text);
    gateway.close();
    another.close();
}

#[test]
fn native_workers_overlap_without_losing_response_identity() {
    let Some(helper) = helper() else { return };
    let mut gateway = Gateway::start(&helper);
    let root = tempfile::tempdir().unwrap();
    let plan = gateway.prepare(root.path(), Default::default());
    for batch in 0..8 {
        let mut expected = std::collections::HashMap::new();
        for item in 0..4 {
            let path = root.path().join(format!("file-{batch}-{item}"));
            let text = format!("contents {batch}-{item}");
            let id = gateway.send(Operation::Run {
                context: plan.host_context.clone(),
                request: FileRequest::Write {
                    path: path.to_string_lossy().into(),
                    text: text.clone(),
                    expected: None,
                },
            });
            expected.insert(id, (path, files::digest(text.as_bytes())));
        }
        for _ in 0..4 {
            let reply = gateway
                .replies
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            let (path, digest) = expected.remove(&reply.id).unwrap();
            let Response::Completed { response } = reply.result.unwrap() else {
                panic!("Unexpected concurrent response")
            };
            assert_eq!(response.file.revision, digest);
            assert_eq!(response.file.path, path.to_string_lossy());
        }
        assert!(expected.is_empty());
    }
    gateway.close();
}

#[test]
fn native_policy_denies_symlink_escape_and_cannot_be_changed_by_the_client() {
    let Some(helper) = helper() else { return };
    for backend in [sandbox::Backend::Bwrap, sandbox::Backend::Landlock] {
        let mut gateway = Gateway::start(&helper);
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("project");
        fs::create_dir(&root).unwrap();
        let secret = base.path().join("secret");
        fs::write(&secret, "outside").unwrap();
        std::os::unix::fs::symlink(&secret, root.join("escape")).unwrap();
        let mut plan = gateway.prepare(
            &root,
            sandbox::Options {
                enabled: true,
                backend,
                ..Default::default()
            },
        );
        if !plan.active {
            let required = if backend == sandbox::Backend::Bwrap {
                "RODE_TEST_REQUIRE_SANDBOX"
            } else {
                "RODE_TEST_REQUIRE_LANDLOCK"
            };
            assert_ne!(
                std::env::var(required).as_deref(),
                Ok("1"),
                "{}",
                plan.reason
            );
            assert!(!plan.reason.is_empty());
            eprintln!("{} unavailable: {}", backend.label(), plan.reason);
            gateway.close();
            continue;
        }
        plan.active = false;
        plan.prefix.clear();
        for path in [secret, root.join("escape")] {
            assert!(
                gateway
                    .run(
                        &plan,
                        FileRequest::Read {
                            path: path.to_string_lossy().into()
                        }
                    )
                    .is_err()
            );
        }
        assert!(
            gateway
                .run(
                    &plan,
                    FileRequest::Tool {
                        request: project::Request::Monitor {
                            root: root.to_string_lossy().into()
                        }
                    }
                )
                .is_err()
        );
        gateway
            .run(
                &plan,
                FileRequest::Write {
                    path: root.join("allowed").to_string_lossy().into(),
                    text: "inside".into(),
                    expected: None,
                },
            )
            .unwrap();
        let read_only = gateway.prepare(
            &root,
            sandbox::Options {
                enabled: true,
                backend,
                project_read_only: true,
                ..Default::default()
            },
        );
        assert!(read_only.active, "{}", read_only.reason);
        assert!(
            gateway
                .run(
                    &read_only,
                    FileRequest::Write {
                        path: root.join("denied").to_string_lossy().into(),
                        text: "no".into(),
                        expected: None
                    }
                )
                .is_err()
        );
        gateway.close();
    }
}

#[test]
fn native_gateway_cancels_owned_commands_on_input_loss() {
    use std::os::unix::fs::PermissionsExt;
    let Some(helper) = helper() else { return };
    let mut gateway = Gateway::start(&helper);
    let root = tempfile::tempdir().unwrap();
    let adapter = root.path().join("adapter");
    let pidfile = root.path().join("pid");
    fs::write(&adapter, "#!/bin/sh\necho $$ > pid\nexec sleep 30\n").unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o700)).unwrap();
    let plan = gateway.prepare(root.path(), Default::default());
    gateway.send(Operation::Run {
        context: plan.host_context,
        request: FileRequest::Tool {
            request: project::Request::Extension {
                root: root.path().to_string_lossy().into(),
                invocation: rode::extensions::Invocation {
                    api: 1,
                    extension: "lifetime".into(),
                    program: adapter.to_string_lossy().into(),
                    args: vec![],
                    operation: "format".into(),
                    path: Some("source".into()),
                    text: String::new(),
                    settings: Default::default(),
                },
            },
        },
    });
    let until = Instant::now() + Duration::from_secs(10);
    while !pidfile.exists() {
        assert!(Instant::now() < until, "Adapter did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid: i32 = fs::read_to_string(pidfile).unwrap().trim().parse().unwrap();
    gateway.close();
    let until = Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(Instant::now() < until, "Owned adapter survived gateway EOF");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn document_transfers_exceed_the_control_message_limit_without_losing_revision_checks() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let Some(helper) = helper() else { return };
    let mut gateway = Gateway::start(&helper);
    let root = tempfile::tempdir().unwrap();
    let plan = gateway.prepare(root.path(), Default::default());
    let path = root.path().join("large.bin").to_string_lossy().into_owned();
    // Base64 exceeds 64 MiB, exercising both directions and the worker's input
    // line without a larger physical frame or an unbounded request queue.
    let bytes = vec![0x93; 49 * 1024 * 1024];
    let revision = files::digest(&bytes);
    let mut run = |request| match gateway.call_with_timeout(
        Operation::Run {
            context: plan.host_context.clone(),
            request,
        },
        Duration::from_secs(160),
    )? {
        Response::Completed { response } => Ok(response),
        _ => Err(String::from("Expected a completed file operation")),
    };
    let saved = run(FileRequest::WriteBinary {
        path: path.clone(),
        data: STANDARD.encode(&bytes),
        expected: None,
    })
    .unwrap();
    assert_eq!(saved.file.revision, revision);
    let read = run(FileRequest::ReadHex { path: path.clone() }).unwrap();
    assert_eq!(read.file.revision, revision);
    assert_eq!(STANDARD.decode(read.file.binary.unwrap()).unwrap(), bytes);
    assert!(
        run(FileRequest::WriteBinary {
            path: path.clone(),
            data: STANDARD.encode([0]),
            expected: Some("stale".into())
        })
        .unwrap_err()
        .contains("conflict")
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), bytes.len() as u64);
    gateway.close();
}

//! Actual desktop/headless bubblewrap grants, read-only rules, nested overrides and visible
//! unavailable-host fallback.

use rode::sandbox::PlanExt as _;
#[path = "support/native.rs"]
mod native_peer;

use rode::{
    files::{self, Request, Response},
    sandbox::{self, Options},
};
use std::{fs, io::Write, path::Path, process::Stdio};

const DEVICE_PROBE: &str = include_str!("fixtures/sandbox_device.py");

#[test]
fn device_probe_checks_allowed_access_and_rejects_false_positives() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let secret = directory.path().join("secret");
    fs::write(&secret, b"private").unwrap();
    for (expected, secret, success, diagnostic) in [
        ("allowed", &missing, true, ""),
        ("denied", &missing, false, "Unexpected device access"),
        (
            "allowed",
            &secret,
            false,
            "Device grant exposed unrelated files",
        ),
    ] {
        let output = std::process::Command::new("/usr/bin/python3")
            .args(["-I", "-c", DEVICE_PROBE, "/dev/zero", expected])
            .arg(secret)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.success(), success, "{stderr}");
        assert!(stderr.contains(diagnostic), "{stderr}");
    }
}

#[test]
fn optional_devices_preserve_filesystem_denials_on_both_sandbox_backends() {
    // Bubblewrap exposes /dev/full by default. A private alias exercises an
    // optional path without GPU hardware or permission to create device nodes.
    // An enclosing sandbox can deny this node before our child policy runs.
    if fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/full")
        .is_err()
    {
        native_peer::capabilities::skip(
            "devices",
            "/dev/full is unavailable with existing host permissions",
        );
        return;
    }
    use std::os::unix::fs::{PermissionsExt, symlink};
    let device_directory = match tempfile::Builder::new()
        .prefix("rode-device-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/dev/shm")
    {
        Ok(directory) => directory,
        Err(error) => {
            native_peer::capabilities::skip(
                "devices",
                &format!("Cannot create a private device alias in /dev/shm: {error}"),
            );
            return;
        }
    };
    let device = device_directory.path().join("full");
    symlink("/dev/full", &device).unwrap();
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), b"private").unwrap();
    for backend in [sandbox::Backend::Bwrap, sandbox::Backend::Landlock] {
        let mut options = Options {
            enabled: true,
            backend,
            ..Default::default()
        };
        let ordinary = native_peer::prepare(project.path().to_str().unwrap(), &options).unwrap();
        if !ordinary.active {
            native_peer::capabilities::skip(
                match backend {
                    sandbox::Backend::Bwrap => "bwrap",
                    sandbox::Backend::Landlock => "landlock",
                },
                &ordinary.reason,
            );
            continue;
        }
        let run = |plan: &sandbox::Plan, allowed: bool| {
            let mut command = rode_host_core::command::from_argv(&plan.prefix).unwrap();
            command
                .args(["/usr/bin/python3", "-I", "-c", DEVICE_PROBE])
                .arg(&device)
                .arg(if allowed { "allowed" } else { "denied" })
                .arg(outside.path().join("secret"));
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{} (device {}, expected {}): {}",
                backend.label(),
                device.display(),
                if allowed { "allowed" } else { "denied" },
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&ordinary, false);
        options.devices = vec![
            device.to_string_lossy().into_owned(),
            device_directory
                .path()
                .join("missing")
                .to_string_lossy()
                .into_owned(),
        ];
        let permitted = native_peer::prepare(project.path().to_str().unwrap(), &options).unwrap();
        assert!(permitted.active, "{}", permitted.reason);
        run(&permitted, true);
    }
}

fn headless(request: &Request, path: Option<&Path>) -> anyhow::Result<Response> {
    headless_using(request, path, native_peer::binary())
}

fn headless_using(
    request: &Request,
    path: Option<&Path>,
    binary: &str,
) -> anyhow::Result<Response> {
    let mut command = std::process::Command::new(binary);
    command
        .arg("--file-helper")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let mut child = command.spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(request)?)?;
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn normal_and_sandbox_project_preparation_match_on_both_backends() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();
    for enabled in [false, true] {
        let request = Request::Prepare {
            path: project.path().to_str().unwrap().into(),
            sandbox: Options {
                enabled,
                ..Default::default()
            },
        };
        let local = files::local(&request).unwrap();
        let remote = headless(&request, None).unwrap();
        assert_eq!(local.path, remote.path);
        assert_eq!(local.entries.len(), 1);
        let local = local.project.unwrap();
        let remote = remote.project.unwrap();
        if enabled && std::env::var("RODE_TEST_REQUIRE_SANDBOX").as_deref() == Ok("1") {
            assert!(
                local.active,
                "Required local sandbox unavailable: {}",
                local.reason
            );
            assert!(
                remote.active,
                "Required headless sandbox unavailable: {}",
                remote.reason
            );
        }
        assert_eq!(local.requested, enabled);
        assert_eq!(
            local.active, remote.active,
            "Local: {} / remote: {}",
            local.reason, remote.reason
        );
        assert_eq!(
            native_peer::policy_arguments(&local),
            native_peer::policy_arguments(&remote)
        );
        if enabled && !local.active {
            eprintln!("Sandbox unavailable in this environment: {}", local.reason);
        }
    }
}

#[test]
fn resolver_symlink_target_is_shared_on_both_backends_without_sharing_run_or_var() {
    let resolver = Path::new("/etc/resolv.conf");
    if !resolver.is_file() {
        return;
    }
    let project = tempfile::tempdir().unwrap();
    let home = files::expand_home("~");
    let resolved = resolver
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    for backend in [sandbox::Backend::Bwrap, sandbox::Backend::Landlock] {
        let args = sandbox::arguments(
            Path::new(native_peer::binary()),
            project.path(),
            &home,
            &Options {
                enabled: true,
                backend,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            args.windows(3)
                .any(|w| w == ["--ro-bind", &resolved, &resolved]),
            "{backend:?}: {args:?}"
        );
        assert!(
            !args
                .windows(3)
                .any(|w| matches!(w[0].as_str(), "--ro-bind" | "--bind")
                    && matches!(w[1].as_str(), "/run" | "/var"))
        );
    }
}

#[test]
fn missing_bwrap_uses_the_bundled_launcher_and_unusable_system_bwrap_is_not_retried() {
    let mut programs = vec![native_peer::binary().to_owned()];
    if let Ok(binary) = std::env::var("RODE_TEST_SESSION_HELPER")
        && !programs.contains(&binary)
    {
        programs.push(binary);
    }
    for program in programs {
        check_bwrap_fallback(&program);
    }
}

fn check_bwrap_fallback(native_binary: &str) {
    let project = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let endpoint = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let request = Request::Prepare {
        path: project.path().to_str().unwrap().into(),
        sandbox: Options {
            enabled: true,
            share_network: false,
            ..Default::default()
        },
    };
    let response = headless_using(&request, Some(tools.path()), native_binary).unwrap();
    let plan = response.project.unwrap();
    assert!(plan.requested);
    assert!(
        plan.reason.contains("bundled bubblewrap"),
        "{}",
        plan.reason
    );
    if std::env::var("RODE_TEST_REQUIRE_SANDBOX").as_deref() == Ok("1") {
        assert!(
            plan.active,
            "Required bundled sandbox unavailable: {}",
            plan.reason
        );
    }
    if plan.active {
        assert_eq!(plan.prefix[1], "--bwrap-launch");
        assert_eq!(Some(&plan.prefix[0]), plan.native_helper.as_ref());
        let helper = Path::new(native_binary);
        let path = project
            .path()
            .join("bundled.txt")
            .to_string_lossy()
            .into_owned();
        plan.local_request_with_helper(
            &Request::Write {
                path: path.clone(),
                text: "bundled project edit".into(),
                expected: None,
            },
            helper,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "bundled project edit");
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("private");
        fs::write(&secret, "private").unwrap();
        assert!(
            plan.local_request_with_helper(
                &Request::Read {
                    path: secret.to_string_lossy().into_owned(),
                },
                helper
            )
            .is_err()
        );
        let probe = std::process::Command::new(&plan.prefix[0])
            .args(&plan.prefix[1..])
            .args(["/usr/bin/python3", "-I", "-c", "import socket,sys\ns=socket.socket();s.settimeout(1)\ntry: s.connect(('127.0.0.1',int(sys.argv[1])))\nexcept OSError: print('OFFLINE')\nelse: raise SystemExit('Bundled command reached host network')"])
            .arg(endpoint.local_addr().unwrap().port().to_string())
            .env("PATH", tools.path())
            .output().unwrap();
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stderr)
        );
        assert_eq!(probe.stdout, b"OFFLINE\n");
    } else {
        assert!(plan.prefix.is_empty());
        assert!(plan.reason.contains("cannot start"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let binary = tools.path().join("bwrap");
        fs::write(
            &binary,
            "#!/bin/sh\nprintf 'user namespaces disabled' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let response = headless_using(&request, Some(tools.path()), native_binary).unwrap();
        let plan = response.project.unwrap();
        assert!(plan.requested && !plan.active);
        assert!(plan.reason.contains("user namespaces disabled"));
        assert!(!plan.reason.contains("bundled bubblewrap"));
        assert!(plan.prefix.is_empty());
    }
}

#[test]
fn bundled_bwrap_is_available_without_path_from_both_native_entry_points() {
    let tools = tempfile::tempdir().unwrap();
    for binary in [env!("CARGO_BIN_EXE_rode"), native_peer::binary()] {
        let output = std::process::Command::new(binary)
            .args(["--bwrap-launch", "--version"])
            .env("PATH", tools.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "bubblewrap 0.13.1 (bundled with RODE)"
        );
        // Argument-file parsing must also work through the bundled dispatch,
        // independently of whether this machine allows namespace creation.
        let argv = [
            binary,
            "--bwrap-launch",
            "--unshare-all",
            "--die-with-parent",
            "--version",
            "--",
        ]
        .map(str::to_owned);
        let output = rode_host_core::command::from_argv(&argv)
            .unwrap()
            .env("PATH", tools.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "bubblewrap 0.13.1 (bundled with RODE)"
        );
    }
}

#[test]
fn active_local_sandbox_allows_project_edits_and_hides_unrelated_files() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("private.txt");
    fs::write(&secret, "outside project").unwrap();
    let options = Options {
        enabled: true,
        ..Default::default()
    };
    let plan = native_peer::prepare(project.path().to_str().unwrap(), &options).unwrap();
    if !plan.active {
        eprintln!("Cannot exercise namespace isolation here: {}", plan.reason);
        return;
    }
    let helper = Path::new(env!("CARGO_BIN_EXE_rode"));
    let path = project
        .path()
        .join("a 'quoted' 🦀.rs")
        .to_str()
        .unwrap()
        .to_owned();
    let write = plan
        .local_request_with_helper(
            &Request::Write {
                path: path.clone(),
                text: "project text\n".into(),
                expected: None,
            },
            helper,
        )
        .unwrap();
    let read = plan
        .local_request_with_helper(&Request::Read { path: path.clone() }, helper)
        .unwrap();
    assert_eq!(read.text, "project text\n");
    assert_eq!(write.revision, read.revision);
    assert!(
        plan.local_request_with_helper(
            &Request::Read {
                path: secret.to_str().unwrap().into()
            },
            helper
        )
        .is_err()
    );
    let forbidden = outside.path().join("forbidden.txt");
    assert!(
        plan.local_request_with_helper(
            &Request::Write {
                path: forbidden.to_str().unwrap().into(),
                text: "bad".into(),
                expected: None
            },
            helper
        )
        .is_err()
    );
    assert!(!forbidden.exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&secret, project.path().join("escape.txt")).unwrap();
        assert!(
            plan.local_request_with_helper(
                &Request::Read {
                    path: project.path().join("escape.txt").to_str().unwrap().into()
                },
                helper
            )
            .is_err()
        );
    }
    assert_eq!(fs::read_to_string(&secret).unwrap(), "outside project");
    let mut command = portable_pty::CommandBuilder::new(&plan.prefix[0]);
    command.args(&plan.prefix[1..]);
    command.args([
        "/bin/sh",
        "-c",
        "printf 'SANDBOX_CWD=%s UV=%s\\n' \"$PWD\" \"$UV_LINK_MODE\"",
    ]);
    let terminal = rode::terminal::Terminal::spawn(
        &eframe::egui::Context::default(),
        command,
        "sandbox test".into(),
    )
    .unwrap();
    let start = std::time::Instant::now();
    while !terminal.contents().contains("UV=copy") && start.elapsed().as_secs() < 3 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        terminal
            .contents()
            .contains(&format!("SANDBOX_CWD={}", project.path().display()))
    );
    assert!(terminal.contents().contains("UV=copy"));
}

#[test]
fn sandbox_paths_are_specific_and_network_access_is_configurable() {
    let project = tempfile::tempdir().unwrap();
    let home = files::expand_home("~");
    let options = Options {
        enabled: true,
        share_network: false,
        ..Default::default()
    };
    let args =
        sandbox::arguments(Path::new("/usr/bin/bwrap"), project.path(), &home, &options).unwrap();
    assert!(args.contains(&"--unshare-all".into()));
    assert!(!args.contains(&"--share-net".into()));
    assert!(!args.windows(3).any(|a| a == ["--bind", "/run", "/run"]));
    assert!(sandbox::arguments(Path::new("/usr/bin/bwrap"), &home, &home, &options).is_err());
    for field in ["read_only", "read_write"] {
        let mut options = options.clone();
        if field == "read_only" {
            options.read_only = vec!["/".into()];
        } else {
            options.read_write = vec!["/".into()];
        }
        assert!(
            sandbox::arguments(Path::new("/usr/bin/bwrap"), project.path(), &home, &options)
                .is_err()
        );
    }
}

#[test]
fn configurable_default_grants_and_read_only_projects_are_enforced_by_both_hosts() {
    let project = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    fs::write(project.path().join("main.py"), "print('project')\n").unwrap();
    let readonly = shared.path().join("dataset.txt");
    fs::write(&readonly, "read only data").unwrap();
    let writable = shared.path().join("cache");
    fs::create_dir(&writable).unwrap();
    let options = Options {
        enabled: true,
        project_read_only: true,
        overrides: [("~/.cargo".into(), sandbox::Access::Hidden)].into(),
        read_only: vec![readonly.to_str().unwrap().into()],
        read_write: vec![writable.to_str().unwrap().into()],
        ..Default::default()
    };
    let request = Request::Prepare {
        path: project.path().to_str().unwrap().into(),
        sandbox: options,
    };
    let local = files::local(&request).unwrap().project.unwrap();
    let peer = headless(&request, None).unwrap().project.unwrap();
    assert_eq!(
        native_peer::policy_arguments(&local),
        native_peer::policy_arguments(&peer)
    );
    assert!(
        !local
            .prefix
            .iter()
            .any(|p| p == &files::expand_home("~/.cargo").to_string_lossy())
    );
    if !local.active {
        eprintln!(
            "Read-only sandbox enforcement unavailable: {}",
            local.reason
        );
        return;
    }
    let helper = Path::new(env!("CARGO_BIN_EXE_rode"));
    assert!(
        local
            .local_request_with_helper(
                &Request::Write {
                    path: project.path().join("blocked.py").to_str().unwrap().into(),
                    text: "blocked".into(),
                    expected: None
                },
                helper
            )
            .is_err()
    );
    let read = local
        .local_request_with_helper(
            &Request::Read {
                path: readonly.to_str().unwrap().into(),
            },
            helper,
        )
        .unwrap();
    assert_eq!(read.text, "read only data");
    assert!(
        local
            .local_request_with_helper(
                &Request::Write {
                    path: readonly.to_str().unwrap().into(),
                    text: "blocked".into(),
                    expected: Some(read.revision)
                },
                helper
            )
            .is_err()
    );
    local
        .local_request_with_helper(
            &Request::Write {
                path: writable.join("allowed.txt").to_str().unwrap().into(),
                text: "allowed".into(),
                expected: None,
            },
            helper,
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(writable.join("allowed.txt")).unwrap(),
        "allowed"
    );
    assert_eq!(fs::read_to_string(readonly).unwrap(), "read only data");
}

#[test]
fn nested_access_rules_override_parent_grants_without_being_masked() {
    let project = tempfile::tempdir().unwrap();
    let child = project.path().join("nested");
    fs::create_dir(&child).unwrap();
    for project_read_only in [false, true] {
        let mut options = Options {
            enabled: true,
            project_read_only,
            ..Default::default()
        };
        if project_read_only {
            options.read_write.push(child.to_str().unwrap().into());
        } else {
            options.read_only.push(child.to_str().unwrap().into());
        }
        let request = Request::Prepare {
            path: project.path().to_str().unwrap().into(),
            sandbox: options,
        };
        let plan = files::local(&request).unwrap().project.unwrap();
        let peer = headless(&request, None).unwrap().project.unwrap();
        assert_eq!(
            native_peer::policy_arguments(&plan),
            native_peer::policy_arguments(&peer)
        );
        if !plan.active {
            eprintln!("Nested mount test unavailable: {}", plan.reason);
            return;
        }
        let write = plan.local_request_with_helper(
            &Request::Write {
                path: child
                    .join(if project_read_only {
                        "writable.txt"
                    } else {
                        "blocked.txt"
                    })
                    .to_str()
                    .unwrap()
                    .into(),
                text: "nested rule".into(),
                expected: None,
            },
            Path::new(env!("CARGO_BIN_EXE_rode")),
        );
        assert_eq!(
            write.is_ok(),
            project_read_only,
            "more specific child rule must take effect: {write:?}"
        );
    }
}

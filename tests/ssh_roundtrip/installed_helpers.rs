//! Real SSH discovery, compatibility rejection and immutable installed-helper use.
use rode::{
    files::Request,
    ssh::{Profile, Remote},
    terminal::Terminal,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn connect(config: &str, helper: &str, jump: &str) -> (Remote, Terminal) {
    let remote = Remote::new(Profile {
        host: "rode-test".into(),
        config: config.into(),
        remote_helper: helper.into(),
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
        std::thread::sleep(Duration::from_millis(25));
    }
    (remote, login)
}

fn shell(remote: &Remote, command: &str) -> String {
    let mut child = Command::new(remote.client_program());
    child.args(remote.channel_args(false)).arg(command);
    rode::project::run(child, vec![], Duration::from_secs(10))
        .unwrap()
        .checked()
        .unwrap()
}

#[test]
#[ignore = "requires scripts/test-ssh.sh and a matching RODE_TEST_SESSION_HELPER"]
fn preinstalled_helpers_are_discovered_pinned_and_version_checked_over_ssh() {
    // Artifact overrides belong to other integration tests. Remove them only in
    // an isolated child so this test exercises automatic remote discovery.
    if std::env::var_os("RODE_INSTALLED_HELPER_CHILD").is_none() {
        // Give this client a private packaged helper so automatic deployment is
        // exercised without global overrides or modifying Cargo's shared output.
        let runner = std::env::current_exe().unwrap();
        let client = tempfile::tempdir_in(runner.parent().unwrap()).unwrap();
        fs::set_permissions(client.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let test_binary = client.path().join("ssh-roundtrip");
        fs::hard_link(&runner, &test_binary).unwrap();
        fs::copy(
            std::env::var_os("RODE_TEST_SESSION_HELPER").expect("Build a release helper"),
            client.path().join("rode-sessiond"),
        )
        .unwrap();
        let status = Command::new(test_binary)
            .env("RODE_TEST_SSH_CLIENT", env!("CARGO_BIN_EXE_rode"))
            .args(["installed_helpers::preinstalled_helpers_are_discovered_pinned_and_version_checked_over_ssh", "--exact", "--ignored", "--nocapture"])
            .env("RODE_INSTALLED_HELPER_CHILD", "1")
            .env_remove("RODE_HOST_HELPER").env_remove("RODE_SESSION_HELPER")
            .status().unwrap();
        assert!(status.success());
        return;
    }
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    let binary = PathBuf::from(
        std::env::var_os("RODE_TEST_SESSION_HELPER").expect("Build a release helper"),
    );
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let package = root.path().join("installed ' package");
    fs::create_dir(&package).unwrap();
    fs::set_permissions(&package, fs::Permissions::from_mode(0o700)).unwrap();
    let installed = package.join("rode-sessiond");
    fs::copy(&binary, &installed).unwrap();
    fs::set_permissions(&installed, fs::Permissions::from_mode(0o700)).unwrap();
    let expected = rode::files::digest(&fs::read(&installed).unwrap());
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("source.txt"), "remote source").unwrap();
    for jump in ["", "rode-jump"] {
        let (remote, login) = connect(&config, installed.to_str().unwrap(), jump);
        let plan = remote
            .request(&Request::Prepare {
                path: project.to_string_lossy().into(),
                sandbox: Default::default(),
            })
            .unwrap()
            .project
            .unwrap();
        let helper = plan.native_helper.as_ref().unwrap();
        assert!(helper.ends_with(&format!("/host-{expected}")));
        assert_ne!(Path::new(helper), installed);
        assert_eq!(
            shell(
                &remote,
                &format!("sha256sum < {}", rode::ssh::quote(helper))
            )
            .split_whitespace()
            .next()
            .unwrap(),
            expected
        );
        // Newly launched file workers must retain the cached image after a
        // package update replaces the installation's bytes.
        fs::write(&installed, b"replacement package fixture").unwrap();
        let source = remote
            .request_in(
                &Request::Read {
                    path: project.join("source.txt").to_string_lossy().into(),
                },
                Some(&plan),
            )
            .unwrap();
        assert_eq!(source.text, "remote source");
        let saved = project.join(format!("saved-{jump}.txt"));
        remote
            .request_in(
                &Request::Write {
                    path: saved.to_string_lossy().into(),
                    text: "saved via installed helper".into(),
                    expected: None,
                },
                Some(&plan),
            )
            .unwrap();
        assert_eq!(
            fs::read_to_string(saved).unwrap(),
            "saved via installed helper"
        );
        fs::copy(&binary, &installed).unwrap();
        fs::set_permissions(&installed, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(remote.connected());
        drop(login);
        remote.cleanup();
    }
    // Discovery uses the remote account's Cargo bin even when login PATH omits it.
    let (remote, login) = connect(&config, "", "");
    shell(
        &remote,
        &format!(
            "mkdir -p \"$HOME/.cargo/bin\"; chmod 700 \"$HOME/.cargo\" \"$HOME/.cargo/bin\"; cp -- {} \"$HOME/.cargo/bin/rode-sessiond\"; chmod 700 \"$HOME/.cargo/bin/rode-sessiond\"",
            rode::ssh::quote(installed.to_str().unwrap())
        ),
    );
    let plan = remote
        .request(&Request::Prepare {
            path: project.to_string_lossy().into(),
            sandbox: Default::default(),
        })
        .unwrap()
        .project
        .unwrap();
    assert!(
        plan.native_helper
            .as_ref()
            .unwrap()
            .ends_with(&format!("/host-{expected}"))
    );
    assert_eq!(
        shell(&remote, "sha256sum < \"$HOME/.cargo/bin/rode-sessiond\"")
            .split_whitespace()
            .next()
            .unwrap(),
        expected
    );
    drop(login);
    remote.cleanup();

    // An explicit incompatible installation must not silently use the desktop artifact.
    let output = Command::new(&installed)
        .arg("--host-info")
        .output()
        .unwrap();
    assert!(output.status.success());
    let original: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for field in [
        "format",
        "api",
        "host_protocol",
        "session_protocol",
        "architecture",
        "os",
        "capabilities",
        "version",
    ] {
        let mut value = original.clone();
        value["info"][field] = match field {
            "architecture" | "os" => "wrong".into(),
            "capabilities" => serde_json::json!([]),
            "version" => "9.9.9-probe-only".into(),
            _ => 999.into(),
        };
        let fake = package.join("incompatible-helper");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s' {}\n",
                rode::ssh::quote(&value.to_string())
            ),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let (remote, login) = connect(&config, fake.to_str().unwrap(), "");
        let error = remote
            .request(&Request::Prepare {
                path: project.to_string_lossy().into(),
                sandbox: Default::default(),
            })
            .unwrap_err();
        assert!(
            format!("{error:#}").contains(if field == "version" {
                "metadata changed"
            } else {
                "configured remote helper"
            }),
            "{error:#}"
        );
        assert!(
            remote.connected(),
            "Metadata rejection closed the authentication master"
        );
        drop(login);
        remote.cleanup();
    }

    let oversized = package.join("oversized-helper");
    fs::write(&oversized, "#!/bin/sh\nprintf '%16385s' ''\n").unwrap();
    fs::set_permissions(&oversized, fs::Permissions::from_mode(0o700)).unwrap();
    let (remote, login) = connect(&config, oversized.to_str().unwrap(), "");
    let error = remote
        .request(&Request::Prepare {
            path: project.to_string_lossy().into(),
            sandbox: Default::default(),
        })
        .unwrap_err();
    assert!(format!("{error:#}").contains("size limit"), "{error:#}");
    drop(login);
    remote.cleanup();

    // A broken automatic candidate must allow the packaged deployment fallback.
    // Explicitly selecting the same candidate must fail, even with that artifact.
    let mut incompatible = original.clone();
    incompatible["info"]["api"] = 999.into();
    let fake = package.join("incompatible-auto-helper");
    let probed = package.join("automatic-probe");
    fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf probed > {}\nprintf '%s' {}\n",
            rode::ssh::quote(probed.to_str().unwrap()),
            rode::ssh::quote(&incompatible.to_string())
        ),
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let (remote, login) = connect(&config, "", "");
    shell(
        &remote,
        &format!(
            "cp -- {} \"$HOME/.cargo/bin/rode-sessiond\"",
            rode::ssh::quote(fake.to_str().unwrap())
        ),
    );
    let plan = remote
        .request(&Request::Prepare {
            path: project.to_string_lossy().into(),
            sandbox: Default::default(),
        })
        .unwrap()
        .project
        .unwrap();
    assert!(
        probed.exists(),
        "The incompatible candidate was not examined"
    );
    assert!(
        plan.native_helper
            .as_ref()
            .unwrap()
            .ends_with(&format!("/host-{expected}"))
    );
    assert_eq!(
        shell(&remote, "\"$HOME/.cargo/bin/rode-sessiond\" --host-info"),
        incompatible.to_string(),
        "Deployment must not replace an incompatible installed package"
    );
    drop(login);
    remote.cleanup();
    let (remote, login) = connect(&config, "~/.cargo/bin/rode-sessiond", "");
    let error = remote
        .request(&Request::Prepare {
            path: project.to_string_lossy().into(),
            sandbox: Default::default(),
        })
        .unwrap_err();
    assert!(format!("{error:#}").contains("configured remote helper"));
    drop(login);
    remote.cleanup();
}

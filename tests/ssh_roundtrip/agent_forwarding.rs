//! Forwarding follows the authenticated master's policy, including native Git and shells.
use rode::{
    files::Request,
    ssh::{Profile, Remote},
};
use std::{
    fs,
    os::unix::net::UnixStream,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// Own private fixture processes even if a regression assertion panics.
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run(command: Command) -> rode::project::Output {
    rode::project::run(command, vec![], Duration::from_secs(15)).unwrap()
}

fn identities(socket: &Path) -> String {
    let mut command = Command::new("ssh-add");
    command.arg("-l").env("SSH_AUTH_SOCK", socket);
    run(command).checked().unwrap()
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn configured_agent_forwarding_reaches_channels_git_and_native_terminals() {
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    let fixture = tempfile::tempdir().unwrap();
    let key = fixture.path().join("key");
    let mut generate = Command::new("ssh-keygen");
    generate.args(["-q", "-t", "ed25519", "-N", "", "-f"]);
    generate.arg(&key);
    run(generate).checked().unwrap();
    let socket = fixture.path().join("agent");
    let mut agent = Process(
        Command::new("ssh-agent")
            .args(["-D", "-a"])
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(10);
    // bind() publishes the path before listen() makes the agent usable.
    while UnixStream::connect(&socket).is_err() {
        assert!(agent.0.try_wait().unwrap().is_none() && Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut add = Command::new("ssh-add");
    add.arg(&key).env("SSH_AUTH_SOCK", &socket);
    run(add).checked().unwrap();
    let expected = identities(&socket);

    // A destination-restricted key is visible to local clients, but must not
    // leak into forwarded listings on an unrelated host. This checks that the
    // forwarding socket is session-bound before any remote agent request.
    let restricted = fixture.path().join("restricted");
    let mut generate = Command::new("ssh-keygen");
    generate
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&restricted);
    run(generate).checked().unwrap();
    let hosts = fixture.path().join("restricted_hosts");
    fs::write(
        &hosts,
        format!(
            "only-other-host {}",
            fs::read_to_string(restricted.with_extension("pub")).unwrap()
        ),
    )
    .unwrap();
    let mut add = Command::new("ssh-add");
    add.arg("-H")
        .arg(&hosts)
        .args(["-h", "only-other-host"])
        .arg(&restricted)
        .env("SSH_AUTH_SOCK", &socket);
    run(add).checked().unwrap();
    assert_eq!(identities(&socket).lines().count(), 2);

    for jump in ["", "rode-jump"] {
        // An omitted setting must retain OpenSSH's default of no forwarding.
        for policy in ["yes", "no", ""] {
            let root = tempfile::tempdir().unwrap();
            let profile = root.path().join("ssh_config");
            let option = if policy.is_empty() {
                String::new()
            } else {
                format!("Host rode-test\n    ForwardAgent {policy}\n")
            };
            fs::write(
                &profile,
                format!("{option}Host *\n    Include \"{config}\"\n"),
            )
            .unwrap();
            let remote = Remote::new(Profile {
                host: "rode-test".into(),
                config: profile.to_string_lossy().into(),
                jump: jump.into(),
                ..Default::default()
            })
            .unwrap();
            // Only the master receives the source socket; channels must use the
            // master's configured agent rather than the caller's environment.
            let log = root.path().join("login.log");
            let mut login = Process(
                Command::new(remote.client_program())
                    .args(remote.master_args().unwrap())
                    .env("SSH_AUTH_SOCK", &socket)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(fs::File::create(&log).unwrap())
                    .spawn()
                    .unwrap(),
            );
            let until = Instant::now() + Duration::from_secs(15);
            while !remote.connected() {
                assert!(
                    login.0.try_wait().unwrap().is_none() && Instant::now() < until,
                    "ForwardAgent={policy:?}, jump={jump:?}: {}",
                    fs::read_to_string(&log).unwrap()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            for tty in [false, true] {
                if tty && std::env::var("RODE_TEST_SSH_PTY").as_deref() == Ok("0") {
                    continue;
                }
                let mut command = Command::new(remote.client_program());
                // Test signing as well as discovery through the forwarded agent.
                command.args(remote.channel_args(tty)).arg(format!(
                    "ssh-add -T {} && ssh-add -l",
                    rode::ssh::quote(key.with_extension("pub").to_str().unwrap())
                ));
                command.env_remove("SSH_AUTH_SOCK");
                let output = run(command);
                assert_eq!(
                    output.success(),
                    policy == "yes",
                    "ForwardAgent={policy:?}, jump={jump:?}, tty={tty}: {output:?}"
                );
                if policy == "yes" {
                    assert_eq!(output.stdout.trim(), expected.trim());
                }
            }

            let plan = remote
                .request(&Request::Prepare {
                    path: root.path().to_string_lossy().into(),
                    sandbox: Default::default(),
                })
                .unwrap()
                .project
                .unwrap();
            assert!(!plan.active);
            // Exercise the Git tab's native helper route. A Git shell alias
            // probes the real agent without contacting an external Git server.
            let response = remote
                .request_in(
                    &Request::Tool {
                        request: rode::project::Request::Git {
                            root: plan.root.clone(),
                            args: vec![
                                "-c".into(),
                                "alias.rode-agent=!ssh-add -l".into(),
                                "rode-agent".into(),
                            ],
                            input: None,
                        },
                    },
                    Some(&plan),
                )
                .unwrap();
            let Some(rode::project::Response::Command(output)) = response.tool else {
                panic!("Expected native Git output");
            };
            assert_eq!(output.success(), policy == "yes", "{output:?}");
            if policy == "yes" {
                assert_eq!(output.stdout.trim(), expected.trim());
            }
            native_terminal(&remote, &plan, &expected, policy == "yes");
            drop(login);
            remote.cleanup();
        }
    }
}

fn native_terminal(remote: &Remote, plan: &rode::sandbox::Plan, expected: &str, enabled: bool) {
    let response = remote
        .request(&Request::Tool {
            request: rode::project::Request::Sessions {
                root: plan.root.clone(),
                operation: rode::sessions::Operation::Create {
                    id: rode_session::new_id().unwrap(),
                    name: "Forwarding test".into(),
                    plan: Box::new(plan.clone()),
                    policy: rode::sessions::fingerprint(plan),
                    python: "off".into(),
                    environment: Default::default(),
                    persistent: false,
                    allocation: None,
                },
                directory: Some(format!("{}/runtime", plan.root)),
            },
        })
        .unwrap();
    let Some(rode::project::Response::Sessions(reply)) = response.tool else {
        panic!("Expected native session reply");
    };
    let rode_session::Response::Session(info) = &reply.response else {
        panic!("Expected native terminal");
    };
    let terminal = super::native_attach(remote, &reply, info);
    terminal
        .send(b"ssh-add -l > agent-identities 2>&1; printf '%s' $? > agent-status\n".to_vec())
        .unwrap();
    let status = Path::new(&plan.root).join("agent-status");
    let until = Instant::now() + Duration::from_secs(15);
    while !fs::read_to_string(&status).is_ok_and(|s| !s.is_empty()) {
        assert!(Instant::now() < until, "{}", terminal.contents());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fs::read_to_string(status).unwrap() == "0", enabled);
    if enabled {
        assert_eq!(
            fs::read_to_string(Path::new(&plan.root).join("agent-identities"))
                .unwrap()
                .trim(),
            expected.trim()
        );
    }
}

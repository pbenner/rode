//! Explorer requests use the same prepared host context over direct SSH and ProxyJump.
use super::*;
use rode::files::Mutation;

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn explorer_mutations_roundtrip_over_ssh_and_proxyjump() {
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
        let deadline = Instant::now() + Duration::from_secs(15);
        while !remote.connected() {
            assert!(
                login.is_alive() && Instant::now() < deadline,
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
        let path = |name: &str| base.path().join(name).to_string_lossy().into_owned();
        let mutate = |mutation| remote.request_in(&Request::Mutate { mutation }, Some(&plan));
        mutate(Mutation::CreateDirectory {
            path: path("folder with spaces"),
        })
        .unwrap();
        remote
            .request_in(
                &Request::Write {
                    path: path("folder with spaces/notes.txt"),
                    text: "remote contents 🦀".into(),
                    expected: None,
                },
                Some(&plan),
            )
            .unwrap();
        mutate(Mutation::Copy {
            path: path("folder with spaces"),
            destination: path("copy"),
        })
        .unwrap();
        assert!(
            mutate(Mutation::Rename {
                path: path("folder with spaces"),
                destination: path("copy")
            })
            .is_err()
        );
        mutate(Mutation::Rename {
            path: path("copy"),
            destination: path("renamed"),
        })
        .unwrap();
        let read = remote
            .request_in(
                &Request::Read {
                    path: path("renamed/notes.txt"),
                },
                Some(&plan),
            )
            .unwrap();
        assert_eq!(read.text, "remote contents 🦀");
        mutate(Mutation::Remove {
            path: path("renamed"),
        })
        .unwrap();
        assert!(!base.path().join("renamed").exists());
        assert_eq!(
            std::fs::read_to_string(base.path().join("folder with spaces/notes.txt")).unwrap(),
            "remote contents 🦀"
        );
        assert!(remote.connected());
        remote.cleanup();
    }
}

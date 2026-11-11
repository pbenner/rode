//! Raw and whole-document transfers cross SSH windows while retaining policy and revisions.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn native_proxyjump_uploads_cross_channel_windows_without_relying_on_a_warm_helper_cache() {
    let remote = Remote::new(Profile {
        host: "rode-test".into(),
        config: std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh"),
        jump: "rode-jump".into(),
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
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut command = remote.command();
    command.arg("wc -c");
    let length = 8 * 1024 * 1024;
    let output =
        rode_host_core::command::run_bounded(command, vec![0x91; length], Duration::from_secs(15))
            .unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap().trim(),
        length.to_string()
    );
    let mut command = remote.command();
    command.arg("cat");
    let payload = vec![0x91; length];
    let output =
        rode_host_core::command::run_bounded(command, payload.clone(), Duration::from_secs(15))
            .unwrap();
    assert_eq!(output, payload);
    assert!(remote.connected());
    remote.cleanup();
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn large_document_transfers_over_ssh_and_proxyjump() {
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
        let path = base
            .path()
            .join("large ' quoted 🦀.bin")
            .to_string_lossy()
            .into_owned();
        let bytes = vec![0x95; 49 * 1024 * 1024];
        let revision = rode::files::digest(&bytes);
        let data = STANDARD.encode(&bytes);
        drop(bytes);
        let saved = remote
            .request_in(
                &Request::WriteBinary {
                    path: path.clone(),
                    data: data.clone(),
                    expected: None,
                },
                Some(&plan),
            )
            .unwrap();
        assert_eq!(saved.revision, revision);
        let read = remote
            .request_in(&Request::ReadHex { path: path.clone() }, Some(&plan))
            .unwrap();
        assert_eq!(read.revision, revision);
        assert_eq!(read.binary.unwrap(), data);
        assert!(
            remote
                .request_in(
                    &Request::WriteBinary {
                        path: path.clone(),
                        data: STANDARD.encode([0]),
                        expected: Some("stale".into()),
                    },
                    Some(&plan)
                )
                .unwrap_err()
                .to_string()
                .contains("conflict")
        );
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 49 * 1024 * 1024);
        assert!(remote.connected());
        remote.cleanup();
    }
}

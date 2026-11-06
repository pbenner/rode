//! Whole-document transfers retain their policy, revision and connection identity over SSH.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

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

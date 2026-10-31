//! Real host regressions run in child test processes to keep service environment private.
use super::super::*;
use rode_runtime_core::service::{Info, Spec, Start};
use std::{
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    process::{Command, Stdio},
};

#[test]
fn notebook_shutdown_ownership_survives_view_replacement_and_reports_failures() {
    let helper = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("rode-sessiond");
    assert!(
        helper.is_file(),
        "Build rode-sessiond with cargo test --all-targets"
    );
    for case in ["reparse", "close_tab", "shutdown_failure"] {
        let root = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app::notebook_view::shutdowns::tests::notebook_shutdown_probe",
                "--ignored",
                "--nocapture",
            ])
            .env("RODE_NOTEBOOK_SHUTDOWN_CASE", case)
            .env("RODE_SESSION_DIR", root.path().join("runtime"))
            .env("RODE_SESSION_HELPER", &helper)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

struct Host {
    helper: String,
    directory: PathBuf,
    plan: sandbox::Plan,
    id: String,
    policy: String,
}
impl Host {
    fn rpc(&self, request: rode_session::Request) -> anyhow::Result<rode_session::Response> {
        let mut child = Command::new(&self.helper)
            .arg("request")
            .env("RODE_SESSION_DIR", &self.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&request)?)?;
        let output = child.wait_with_output()?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<rode_session::Response>(&output.stdout)?.checked()
    }
    fn infos(&self) -> Vec<Info> {
        let rode_session::Response::Runtimes(infos) =
            self.rpc(rode_session::Request::Runtimes).unwrap()
        else {
            panic!()
        };
        infos
    }
    fn create(&self) -> Info {
        let spec = Spec {
            id: self.id.clone(),
            name: "Shutdown regression".into(),
            sources: Default::default(),
            root: self.plan.root.clone(),
            policy: self.policy.clone(),
            mode: self.plan.label().into(),
            document: PathBuf::from(&self.plan.root)
                .join("demo.ipynb")
                .to_string_lossy()
                .into(),
            argv: vec![
                "python3".into(),
                "-u".into(),
                "-c".into(),
                include_str!("../../../../tests/fixtures/notebook_bridge.py").into(),
            ],
            prefix: vec![],
            env: std::collections::BTreeMap::from([(
                "PATH".into(),
                std::env::var("PATH").unwrap(),
            )]),
            python: "off".into(),
            python_program: false,
            persistent: true,
            target: None,
            start: Start::Notebook {
                name: "python3".into(),
            },
        };
        let rode_session::Response::Runtime(info) = self
            .rpc(rode_session::Request::CreateRuntime {
                spec: Box::new(spec),
            })
            .unwrap()
        else {
            panic!()
        };
        *info
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.rpc(rode_session::Request::EndRuntime {
            id: self.id.clone(),
            policy: self.policy.clone(),
        });
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if matches!(self.rpc(rode_session::Request::Runtimes), Ok(rode_session::Response::Runtimes(infos)) if infos.is_empty())
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.rpc(rode_session::Request::Shutdown);
    }
}

/// A legacy service can delay routing before End reaches the current service.
/// Hold that exact window open instead of relying on sleeps in a fast local test.
struct BlockedDiscovery {
    entered: mpsc::Receiver<()>,
    release: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl BlockedDiscovery {
    fn new(directory: &std::path::Path, failure: bool) -> Self {
        let socket = directory.join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (ready, entered) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(10);
            let mut peer = loop {
                match listener.accept() {
                    Ok((peer, _)) => break peer,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < until =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("No pending shutdown reached discovery: {error}"),
                }
            };
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut len = [0u8; 4];
            peer.read_exact(&mut len).unwrap();
            let mut frame = vec![0; u32::from_be_bytes(len) as usize];
            peer.read_exact(&mut frame).unwrap();
            ready.send(()).unwrap();
            let _ = gate.recv_timeout(Duration::from_secs(10));
            let response = if failure {
                br#"J{"kind":"error","data":"Fixture host connection lost"}"#.as_slice()
            } else {
                br#"J{"kind":"runtimes","data":[]}"#.as_slice()
            };
            let _ = peer
                .write_all(&(response.len() as u32).to_be_bytes())
                .and_then(|_| peer.write_all(response));
            drop(listener);
            let _ = std::fs::remove_file(socket);
        });
        Self {
            entered,
            release: Some(release),
            thread: Some(thread),
        }
    }
    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
impl Drop for BlockedDiscovery {
    fn drop(&mut self) {
        self.release();
    }
}

#[test]
#[ignore = "invoked by the isolated parent test"]
fn notebook_shutdown_probe() {
    let Ok(case) = std::env::var("RODE_NOTEBOOK_SHUTDOWN_CASE") else {
        return;
    };
    let primary = PathBuf::from(std::env::var_os("RODE_SESSION_DIR").unwrap());
    std::fs::create_dir_all(&primary).unwrap();
    std::fs::set_permissions(&primary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = tempfile::tempdir().unwrap();
    let plan = sandbox::Plan {
        root: root.path().to_string_lossy().into(),
        ..Default::default()
    };
    let host = Host {
        helper: std::env::var("RODE_SESSION_HELPER").unwrap(),
        directory: primary.join(format!("protocol-{}", rode_session::PROTOCOL)),
        id: rode_session::new_id().unwrap(),
        policy: crate::sessions::fingerprint(&plan),
        plan,
    };
    let info = host.create();
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_notebook();
    let (ready, prepared) = mpsc::channel();
    let kernel = Kernel::attach(
        host.plan.clone(),
        None,
        info,
        Arc::new(move || {
            let _ = ready.send(());
        }),
    )
    .unwrap();
    prepared.recv_timeout(Duration::from_secs(5)).unwrap();
    app.documents[0].notebook.as_mut().unwrap().kernel = Some(kernel);
    let failure = case == "shutdown_failure";
    let mut blocked = BlockedDiscovery::new(&primary, failure);
    app.toggle_notebook_source(&ctx);
    assert!(app.sessions_need_exit_check());
    blocked
        .entered
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    if case == "close_tab" {
        app.perform(crate::app::Action::CloseTab(app.documents[0].id), &ctx);
        assert!(app.documents.is_empty());
    } else {
        let doc = &app.documents[0];
        app.notebook_parsed(
            doc.id,
            doc.edit_version,
            Ok(Notebook::parse(&doc.text).unwrap()),
            &ctx,
        );
        assert!(!app.documents[0].notebook.as_ref().unwrap().source_mode);
    }
    assert!(
        app.sessions_need_exit_check(),
        "Replacing a view discarded its pending shutdown"
    );
    app.quit_terminals(&ctx);
    assert!(
        !app.quitting,
        "Window closed before the host accepted shutdown"
    );
    assert!(
        host.infos()
            .iter()
            .any(|info| info.id == host.id && info.alive && info.persistent)
    );
    blocked.release();
    let until = Instant::now() + Duration::from_secs(10);
    while app.notebook_shutdowns.pending() {
        app.poll_notebooks(&ctx);
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    if failure {
        assert!(!app.quitting);
        assert!(!app.session_exit_pending());
        assert!(app.notice.as_ref().is_some_and(
            |(message, error, _)| *error && message.contains("Fixture host connection lost")
        ));
        assert!(
            host.infos()
                .iter()
                .any(|info| info.id == host.id && info.alive)
        );
    } else {
        assert!(
            host.infos().is_empty(),
            "Shutdown was acknowledged before the runtime disappeared"
        );
        app.session_tick(&ctx);
        assert!(app.quitting);
    }
}

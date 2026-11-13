//! Real host regressions run in child test processes to keep service environment private.
use super::super::*;
use rode_runtime_core::service::{Info, Spec, Start};
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
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
    for case in [
        "reparse",
        "close_tab",
        "shutdown_failure",
        "reload_persistent",
        "reload_foreground",
        "leave_workspace",
        "leave_disabled",
        "leave_vault",
    ] {
        let root = tempfile::tempdir().unwrap();
        let wrapper = root.path().join("helper.py");
        std::fs::write(
            &wrapper,
            include_str!("../../../../tests/fixtures/session_helper_gate.py"),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app::notebook_view::shutdowns::tests::notebook_shutdown_probe",
                "--ignored",
                "--nocapture",
            ])
            .env("RODE_NOTEBOOK_SHUTDOWN_CASE", case)
            .env("RODE_SESSION_DIR", root.path().join("runtime"))
            .env("RODE_SESSION_HELPER", &wrapper)
            .env("RODE_TEST_REAL_SESSION_HELPER", &helper)
            .env("RODE_TEST_HELPER_GATE", root.path())
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

/// Hold the matching helper's readiness check while a shutdown is pending.
struct BlockedHelper {
    root: PathBuf,
}
impl BlockedHelper {
    fn new(root: &std::path::Path, failure: bool) -> Self {
        if failure {
            std::fs::write(root.join("fail"), "").unwrap();
        }
        std::fs::write(root.join("hold"), "").unwrap();
        Self { root: root.into() }
    }
    fn wait(&self) {
        let until = Instant::now() + Duration::from_secs(5);
        while !self.root.join("entered").exists() {
            assert!(
                Instant::now() < until,
                "Shutdown did not reach helper readiness check"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn release(&self) {
        std::fs::write(self.root.join("release"), "").unwrap();
    }
}
impl Drop for BlockedHelper {
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
        helper: std::env::var("RODE_TEST_REAL_SESSION_HELPER").unwrap(),
        directory: primary.clone(),
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
    let mut kernel = Kernel::attach(
        host.plan.clone(),
        None,
        info,
        Arc::new(move || {
            let _ = ready.send(());
        }),
    )
    .unwrap();
    prepared.recv_timeout(Duration::from_secs(5)).unwrap();
    if case.starts_with("leave_") {
        let until = Instant::now() + Duration::from_secs(10);
        while kernel.pending() {
            kernel.poll();
            assert!(Instant::now() < until, "Initial attachment did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        app.documents[0].notebook.as_mut().unwrap().kernel = Some(kernel);
        let reason = match case.as_str() {
            "leave_workspace" => workspace::LeaveReason::ProjectChanged,
            "leave_disabled" => workspace::LeaveReason::NotebooksDisabled,
            "leave_vault" => workspace::LeaveReason::VaultLock,
            _ => unreachable!(),
        };
        app.leave_notebooks(reason);
        let stopping = reason == workspace::LeaveReason::VaultLock;
        assert_eq!(app.notebook_shutdowns.pending(), stopping);
        loop {
            app.poll_notebooks(&ctx);
            let infos = host.infos();
            if stopping {
                if !app.notebook_shutdowns.pending() {
                    assert!(
                        infos.is_empty(),
                        "Stop was acknowledged before host cleanup"
                    );
                    break;
                }
            } else if infos
                .iter()
                .any(|info| info.id == host.id && info.alive && info.persistent && !info.attached)
            {
                break;
            }
            assert!(
                Instant::now() < until,
                "Typed leave policy did not reach the host"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        return;
    }
    if case.starts_with("reload_") {
        let until = Instant::now() + Duration::from_secs(10);
        while kernel.pending() {
            kernel.poll();
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        let persistent = case == "reload_persistent";
        if !persistent {
            kernel.set_persistent(false).unwrap();
            while kernel.pending() {
                kernel.poll();
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        app.workspace.set_fixture(Some(crate::app::Workspace {
            root: host.plan.root.clone(),
            remote: None,
            connected: true,
            ready: true,
            plan: host.plan.clone(),
            sandbox_options: Default::default(),
        }));
        let doc = &mut app.documents[0];
        let path = root
            .path()
            .join("demo.ipynb")
            .to_string_lossy()
            .into_owned();
        std::fs::write(&path, &doc.text).unwrap();
        doc.acknowledge_save(
            path,
            doc.text.clone(),
            crate::files::digest(doc.text.as_bytes()),
        );
        let version = doc.edit_version;
        let uid = doc.notebook.as_ref().unwrap().book.cells[0].uid;
        doc.notebook.as_mut().unwrap().kernel = Some(kernel);
        app.reload_clean_documents(&ctx);
        let mut events = Vec::new();
        loop {
            let event = app.workers.receive_timeout(Duration::from_secs(5)).unwrap();
            let reloaded = matches!(
                &event,
                workers::Message::Workspace(event)
                    if matches!(event.as_ref(), WorkspaceEvent::Done { job: Job::Reload { .. }, .. })
            );
            events.push(event);
            if reloaded {
                break;
            }
        }
        for event in events {
            match event {
                workers::Message::Workspace(event) => {
                    app.workers.workspace(&ctx).send(*event).unwrap();
                }
                workers::Message::Profiles(event) => {
                    app.workers.profiles(&ctx).send(event).unwrap();
                }
                workers::Message::Expired => {}
            }
        }
        app.poll(&ctx);
        let doc = &app.documents[0];
        let view = doc.notebook.as_ref().unwrap();
        assert_eq!(view.native_id(), Some(host.id.as_str()));
        assert_eq!(view.book.cells[0].uid, uid);
        assert_eq!(doc.edit_version, version);
        assert_eq!(view.kernel.as_ref().unwrap().persistent(), persistent);
        assert!(
            host.infos()
                .iter()
                .any(|info| info.id == host.id && info.attached && info.alive)
        );
        return;
    }
    app.documents[0].notebook.as_mut().unwrap().kernel = Some(kernel);
    let failure = case == "shutdown_failure";
    let blocked = BlockedHelper::new(primary.parent().unwrap(), failure);
    app.toggle_notebook_source(&ctx);
    assert!(app.runtime_work_pending());
    blocked.wait();
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
        app.runtime_work_pending(),
        "Replacing a view discarded its pending shutdown"
    );
    app.request_exit(&ctx);
    assert!(
        !app.exit.closing(),
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
        assert!(!app.exit.closing());
        assert!(!app.exit_pending());
        assert!(
            app.notice.as_ref().is_some_and(
                |(message, error, _)| *error && message.contains("RODE_SESSION_HELPER")
            )
        );
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
        assert!(app.exit.closing());
    }
}

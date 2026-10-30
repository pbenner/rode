//! Optional real CLI checks. Only a loopback fixture receives inference-shaped requests.
use rode::{
    agent::{
        Config, Event, Launch, Provider, Session,
        protocol::{Chat, Role},
    },
    sandbox, ssh,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, Instant},
};

fn response(session: &Session, id: u64) -> Value {
    let end = Instant::now() + Duration::from_secs(20);
    let mut diagnostics = String::new();
    loop {
        while let Some(event) = session.poll() {
            match event {
                Event::Message(value) if value["id"].as_u64() == Some(id) => {
                    assert!(value.get("error").is_none(), "{value}\n{diagnostics}");
                    return value["result"].clone();
                }
                Event::Error(message) | Event::Stderr(message) => diagnostics.push_str(&message),
                Event::Exited(code) => panic!("Codex exited {code:?}: {diagnostics}"),
                _ => {}
            }
        }
        assert!(
            Instant::now() < end,
            "Codex response timed out: {diagnostics}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn bundled_auto_discovery_initializes_on_the_project_host_without_external_auth() {
    let Ok(bundle) = std::env::var("RODE_TEST_CODEX_BUNDLE") else {
        return;
    };
    for isolated in [false, true] {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("codex-home")).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", "bundled_auto_child", "--nocapture"])
            .env("RODE_CODEX_BUNDLE", &bundle)
            .env("RODE_BUNDLE_PROJECT", root.path())
            .env("RODE_BUNDLE_SANDBOX", if isolated { "1" } else { "0" })
            .env("CODEX_HOME", root.path().join("codex-home"))
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY");
        let output = rode::project::run(command, vec![], Duration::from_secs(45)).unwrap();
        assert!(output.success(), "{}\n{}", output.stdout, output.stderr);
    }
}

#[test]
#[ignore = "private subprocess entry point for bundled Codex with isolated credentials"]
fn bundled_auto_child() {
    let root = std::env::var("RODE_BUNDLE_PROJECT").unwrap();
    let plan = sandbox::prepare(
        &root,
        &sandbox::Options {
            enabled: std::env::var("RODE_BUNDLE_SANDBOX").unwrap() == "1",
            ..Default::default()
        },
    )
    .unwrap();
    let session = Session::start(
        Launch {
            config: Config {
                provider: Provider::Local,
                local_model: "fixture-local-model".into(),
                local_url: "http://127.0.0.1:9/v1".into(),
                ..Default::default()
            },
            plan,
            remote: None,
            python: "off".into(),
        },
        Arc::new(|| {}),
    )
    .unwrap();
    session.send(&json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"rode","title":"RODE","version":"0.1.0"}}})).unwrap();
    assert!(response(&session, 1)["userAgent"].is_string());
    session
        .send(&json!({"method":"initialized","params":{}}))
        .unwrap();
    session.send(&json!({"id":2,"method":"thread/start","params":{"cwd":root,"model":"fixture-local-model","modelProvider":"rode_local","approvalPolicy":"on-request","sandbox":"workspace-write"}})).unwrap();
    assert_eq!(
        response(&session, 2)["thread"]["modelProvider"],
        "rode_local"
    );
    session.stop();
}

#[test]
fn installed_codex_initializes_a_project_with_an_isolated_local_provider() {
    let Ok(program) = std::env::var("RODE_TEST_CODEX") else {
        eprintln!("Set RODE_TEST_CODEX to check a real Codex app-server without inference.");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    let home = temp.path().join("codex-home");
    fs::create_dir(&home).unwrap();
    let wrapper = root.join("codex-wrapper");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport CODEX_HOME={}\nexec {} \"$@\"\n",
            ssh::quote(home.to_str().unwrap()),
            ssh::quote(&program)
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let config = Config {
        provider: Provider::Local,
        codex: wrapper.to_string_lossy().into(),
        local_model: "fixture-local-model".into(),
        local_url: "http://127.0.0.1:9/v1".into(),
        ..Default::default()
    };
    let session = Session::start(
        Launch {
            config,
            plan: sandbox::prepare(root.to_str().unwrap(), &Default::default()).unwrap(),
            remote: None,
            python: "auto".into(),
        },
        Arc::new(|| {}),
    )
    .unwrap();
    session.send(&json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"rode","title":"RODE","version":"0.1.0"}}})).unwrap();
    let initialized = response(&session, 1);
    assert!(initialized["userAgent"].is_string());
    session
        .send(&json!({"method":"initialized","params":{}}))
        .unwrap();
    session.send(&json!({"id":2,"method":"thread/start","params":{"cwd":root,"model":"fixture-local-model","modelProvider":"rode_local","approvalPolicy":"on-request","sandbox":"workspace-write"}})).unwrap();
    let thread = response(&session, 2);
    assert!(!thread["thread"]["id"].as_str().unwrap().is_empty());
    assert_eq!(thread["thread"]["modelProvider"], "rode_local");
    session.stop();
}

#[test]
fn installed_codex_completes_a_turn_against_a_loopback_responses_fixture() {
    let Ok(program) = std::env::var("RODE_TEST_CODEX") else {
        return;
    };
    for isolated in [false, true] {
        codex_turn(&program, isolated);
    }
}

fn codex_turn(program: &str, isolated: bool) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    let home = root.join("codex-home");
    fs::create_dir(&home).unwrap();
    let wrapper = root.join("codex-wrapper");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport CODEX_HOME={}\nexec {} \"$@\"\n",
            ssh::quote(home.to_str().unwrap()),
            ssh::quote(program)
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _server = Server(
        std::process::Command::new("python3")
            .args([
                "-c",
                include_str!("fixtures/responses.py"),
                temp.path().to_str().unwrap(),
            ])
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(3);
    while !temp.path().join("port").exists() {
        assert!(Instant::now() < until, "Local fixture did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let port = fs::read_to_string(temp.path().join("port")).unwrap();
    let mut chat = Chat::default();
    let launch = Launch {
        config: Config {
            provider: Provider::Local,
            codex: wrapper.to_string_lossy().into(),
            local_model: "fixture-local-model".into(),
            local_url: format!("http://127.0.0.1:{port}/v1"),
            access: rode::agent::Access::Full,
            approval: rode::agent::Approval::Never,
            reasoning_effort: "high".into(),
            personality: rode::agent::Personality::JacksonLamb,
            ..Default::default()
        },
        plan: sandbox::prepare(
            root.to_str().unwrap(),
            &sandbox::Options {
                enabled: isolated,
                read_only: vec![program.into()],
                overrides: std::collections::BTreeMap::from([(
                    "~/.codex".into(),
                    sandbox::Access::Hidden,
                )]),
                ..Default::default()
            },
        )
        .unwrap(),
        remote: None,
        python: "auto".into(),
    };
    chat.submit(
        launch.clone(),
        "Reply once without tools.".into(),
        Arc::new(|| {}),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while chat.busy {
        chat.poll();
        assert!(
            Instant::now() < until,
            "Codex turn timed out: {}",
            chat.status
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        chat.status,
        "Ready",
        "{}",
        chat.messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        chat.messages
            .iter()
            .any(|m| m.role == Role::Assistant && m.text == "Local Responses fixture reply.")
    );
    let request: Value =
        serde_json::from_str(&fs::read_to_string(temp.path().join("request.json")).unwrap())
            .unwrap();
    assert_eq!(request["model"], "fixture-local-model");
    assert_eq!(request["stream"], true);
    assert!(request["input"].to_string().contains("rode_response_style"));
    // History discovery and replay use the real on-disk app-server rollouts,
    // including threads created with source appServer, without another model call.
    drop(chat);
    let mut browser = rode::agent::history::Browser::open(launch, Arc::new(|| {})).unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    while browser.busy {
        browser.poll();
        assert!(Instant::now() < end, "{}", browser.status);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(browser.entries.len(), 1, "{}", browser.status);
    assert!(
        !browser.entries[0].preview.contains("rode_response_style"),
        "{}",
        browser.entries[0].preview
    );
    assert!(
        !browser.entries[0].title.contains("rode_response_style"),
        "{}",
        browser.entries[0].title
    );
    let id = browser.entries[0].id.clone();
    browser.resume(&id).unwrap();
    while browser.busy {
        browser.poll();
        assert!(Instant::now() < end, "{}", browser.status);
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut restored = Chat::default();
    restored.restore(
        browser
            .take_restored()
            .unwrap_or_else(|| panic!("{}", browser.status)),
    );
    assert!(restored.connected());
    assert!(
        restored
            .messages
            .iter()
            .any(|m| m.role == Role::User && m.text == "Reply once without tools.")
    );
    assert!(
        restored
            .messages
            .iter()
            .any(|m| m.role == Role::Assistant && m.text == "Local Responses fixture reply."),
        "{:?}",
        restored
            .messages
            .iter()
            .map(|m| &m.text)
            .collect::<Vec<_>>()
    );
}

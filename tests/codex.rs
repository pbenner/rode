//! Optional real CLI checks. Only a loopback fixture receives inference-shaped requests.

#[path = "support/native.rs"]
mod native_peer;

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
        native_peer::capabilities::skip(
            "codex",
            "Set RODE_TEST_CODEX_BUNDLE for the real bundled runtime",
        );
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
    let plan = native_peer::prepare(
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
        native_peer::capabilities::skip(
            "codex",
            "Set RODE_TEST_CODEX to check a real Codex app-server without inference.",
        );
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
            plan: native_peer::prepare(root.to_str().unwrap(), &Default::default()).unwrap(),
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
        native_peer::capabilities::skip("codex", "Set RODE_TEST_CODEX for the real runtime");
        return;
    };
    for (isolated, backend) in [
        (false, sandbox::Backend::Bwrap),
        (true, sandbox::Backend::Bwrap),
        (true, sandbox::Backend::Landlock),
    ] {
        codex_turn(&program, isolated, backend);
    }
}

fn codex_turn(program: &str, isolated: bool, backend: sandbox::Backend) {
    fn finish_turn(chat: &mut Chat) {
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
    }
    fn check_latest_prompt(directory: &std::path::Path, task: &str) {
        let request: Value =
            serde_json::from_str(&fs::read_to_string(directory.join("request.json")).unwrap())
                .unwrap();
        assert_eq!(request["model"], "fixture-local-model");
        assert_eq!(request["stream"], true);
        // Looking anywhere in history could pass with only the first turn's
        // hint. Check the most recent user input that actually reaches a model.
        let latest = request["input"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|v| v["role"] == "user")
            .unwrap();
        let text = latest["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(task), "{latest}");
        assert!(text.contains("name=\"jackson_lamb\""), "{latest}");
        assert!(!text.contains("<rode_response_style version="), "{latest}");
        assert_eq!(text.matches("<rode_response_style ").count(), 1, "{latest}");
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    let home = root.join("codex-home");
    fs::create_dir(&home).unwrap();
    fs::write(home.join("config.toml"), "[features]\ncode_mode = true\n").unwrap();
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
        plan: native_peer::prepare(
            root.to_str().unwrap(),
            &sandbox::Options {
                enabled: isolated,
                backend,
                read_only: vec![
                    std::path::Path::new(program)
                        .parent()
                        .unwrap()
                        .to_string_lossy()
                        .into(),
                ],
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
    if backend == sandbox::Backend::Landlock && !launch.plan.active {
        assert_ne!(
            std::env::var("RODE_TEST_REQUIRE_LANDLOCK").as_deref(),
            Ok("1"),
            "{}",
            launch.plan.reason
        );
        eprintln!(
            "Real Codex Landlock check unavailable: {}",
            launch.plan.reason
        );
        return;
    }
    chat.submit(
        launch.clone(),
        "Reply once without tools.".into(),
        Arc::new(|| {}),
    )
    .unwrap();
    finish_turn(&mut chat);
    assert!(
        chat.messages
            .iter()
            .any(|m| m.role == Role::Assistant && m.text == "Local Responses fixture reply.")
    );
    check_latest_prompt(temp.path(), "Reply once without tools.");
    for task in [
        "Continue without tools.",
        "One more follow-up without tools.",
    ] {
        chat.submit(launch.clone(), task.into(), Arc::new(|| {}))
            .unwrap();
        finish_turn(&mut chat);
        check_latest_prompt(temp.path(), task);
    }
    // A second read-only connection can recover old prompts even while this
    // process owns the live thread. It must not resume it or call the model.
    let before = fs::read(temp.path().join("request.json")).unwrap();
    let mut reader = launch
        .prompt_history(
            chat.conversation_id().unwrap(),
            vec!["One more follow-up without tools.".into()],
            Arc::new(|| {}),
        )
        .unwrap();
    let mut prompts = Vec::new();
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        reader.poll();
        assert!(Instant::now() < until, "Read-only prompt history timed out");
        if let Some(error) = reader.take_error() {
            panic!("{error}");
        }
        prompts.splice(0..0, reader.take_prompts());
        if !reader.busy() {
            if !reader.more() {
                break;
            }
            reader.load_more().unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        prompts,
        ["Reply once without tools.", "Continue without tools."]
    );
    assert_eq!(
        fs::read(temp.path().join("request.json")).unwrap(),
        before,
        "Reading recall history called the model"
    );
    fs::write(temp.path().join("tool-mode"), "run one tool").unwrap();
    chat.submit(
        launch.clone(),
        "Run the fixture command.".into(),
        Arc::new(|| {}),
    )
    .unwrap();
    finish_turn(&mut chat);
    assert_eq!(
        fs::read_to_string(root.join("rode-tool-result.txt")).unwrap_or_else(|error| panic!(
            "Tool execution failed: {error}; {:?}",
            chat.messages.iter().map(|m| &m.text).collect::<Vec<_>>()
        )),
        "RODE_TOOL_OK"
    );
    let tool_request = fs::read_to_string(temp.path().join("request.json")).unwrap();
    assert!(
        tool_request.contains("RODE_TOOL_OK"),
        "Tool output was not returned to the model"
    );
    // The real runtime must ask in ordinary coding mode and deliver the user's
    // answer back to the loopback peer. Current Codex marks this non-blocking.
    fs::write(temp.path().join("question-mode"), "").unwrap();
    chat.submit(
        launch.clone(),
        "Ask which tests to run.".into(),
        Arc::new(|| {}),
    )
    .unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    while chat.prompts.is_empty() {
        chat.poll();
        assert!(
            chat.busy && Instant::now() < end,
            "No question: {}: {:?}",
            chat.status,
            chat.messages
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        chat.status,
        if chat.prompts[0].blocking {
            "Waiting for you"
        } else {
            "Question awaiting your answer · agent working"
        }
    );
    assert_eq!(
        chat.prompts[0].questions[0].descriptions[0],
        "Run the full suite"
    );
    assert!(
        !fs::read_to_string(temp.path().join("request.json"))
            .unwrap()
            .contains("Changed tests only")
    );
    chat.prompts[0].questions[0].answer = "Changed tests only".into();
    chat.answer(chat.prompts[0].answers().unwrap()).unwrap();
    finish_turn(&mut chat);
    assert!(
        fs::read_to_string(temp.path().join("request.json"))
            .unwrap()
            .contains("Changed tests only")
    );
    // request_user_input_async uses agentMessage.questions, not a server
    // request. Let Codex finish before answering: the form must survive, and
    // the answer must start a normal follow-up turn with the captured policy.
    {
        let async_home = root.join("async-codex-home");
        fs::create_dir(&async_home).unwrap();
        fs::write(
            async_home.join("config.toml"),
            "[features]\ncode_mode = true\nsend_message_to_user_async = true\n",
        )
        .unwrap();
        let async_wrapper = root.join("codex-async-wrapper");
        fs::write(&async_wrapper, format!("#!/bin/sh\nexport CODEX_HOME={}\nexec {} \"$@\" -c 'features.default_mode_request_user_input=false'\n", ssh::quote(async_home.to_str().unwrap()), ssh::quote(program))).unwrap();
        fs::set_permissions(&async_wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let mut launch = launch.clone();
        launch.config.codex = async_wrapper.to_string_lossy().into();
        // Use bundled catalog metadata advertising asynchronous interaction.
        // Inference still goes exclusively to our loopback Responses fixture.
        launch.config.local_model = "gpt-6-astra".into();
        let mut chat = Chat::default();
        fs::write(temp.path().join("async-question-mode"), "").unwrap();
        chat.submit(
            launch.clone(),
            "Ask an asynchronous network question.".into(),
            Arc::new(|| {}),
        )
        .unwrap();
        let end = Instant::now() + Duration::from_secs(30);
        while chat.busy {
            chat.poll();
            assert!(
                Instant::now() < end,
                "Async question did not finish: {}",
                chat.status
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(chat.last_turn_succeeded, "{}", chat.status);
        if chat.prompts.is_empty() {
            let request: Value = serde_json::from_str(
                &fs::read_to_string(temp.path().join("request.json")).unwrap(),
            )
            .unwrap();
            let outputs: Vec<_> = request["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["type"] == "function_call_output")
                .collect();
            let tools = request["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|tool| {
                    tool["name"]
                        .as_str()
                        .filter(|name| name.contains("user_input"))
                })
                .collect::<Vec<_>>();
            panic!(
                "No asynchronous form: {outputs:?}\nTools: {tools:?}\n{:?}",
                chat.messages
            );
        }
        assert_eq!(chat.prompts.len(), 1);
        assert!(chat.prompts[0].is_async_message());
        assert_eq!(chat.prompts[0].questions.len(), 2);
        chat.prompts[0].questions[0].answer = "Disabled".into();
        chat.prompts[0].questions[1].answer = "Only the async regression".into();
        chat.answer(chat.prompts[0].answers().unwrap()).unwrap();
        finish_turn(&mut chat);
        assert!(chat.prompts.is_empty());
        let request = fs::read_to_string(temp.path().join("request.json")).unwrap();
        assert!(request.contains("Only the async regression"), "{request}");
    }
    // A second process cannot resume an idle chat that still has an owner.
    // Keep that chat usable, and distinguish the lock from startup diagnostics.
    {
        let mut locked =
            rode::agent::history::Browser::open(launch.clone(), Arc::new(|| {})).unwrap();
        let end = Instant::now() + Duration::from_secs(30);
        while locked.busy {
            locked.poll();
            assert!(Instant::now() < end, "{}", locked.status);
            std::thread::sleep(Duration::from_millis(10));
        }
        let id = locked
            .entries
            .iter()
            .find(|entry| Some(entry.id.as_str()) == chat.conversation_id())
            .unwrap()
            .id
            .clone();
        locked.resume(&id).unwrap();
        while locked.busy {
            locked.poll();
            assert!(Instant::now() < end, "{}", locked.status);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(locked.take_restored().is_none());
        assert!(
            locked.status.contains("In Running, choose Attach"),
            "{}",
            locked.status
        );
        assert!(
            locked
                .diagnostics()
                .contains("already has an active writer")
        );
        assert!(chat.connected());
        assert_eq!(chat.status, "Ready");
    }
    // History discovery and replay use the real on-disk app-server rollouts,
    // including threads created with source appServer, without another model call.
    drop(chat);
    let mut browser = rode::agent::history::Browser::open(launch.clone(), Arc::new(|| {})).unwrap();
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
    let task = "Continue the restored conversation without tools.";
    restored
        .submit(launch, task.into(), Arc::new(|| {}))
        .unwrap();
    finish_turn(&mut restored);
    check_latest_prompt(temp.path(), task);
}

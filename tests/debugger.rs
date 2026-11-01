//! DAP framing/state, interactive debug PTYs and owned-target cleanup on native/Python
//! coordinators; real installed adapters are opt-in.

#[path = "support/process.rs"]
mod fixture_process;

use base64::{Engine, engine::general_purpose::STANDARD};
use rode::{
    debugger::{
        self, Backend, Breakpoint, Event, Settings, Start,
        protocol::{Client, Phase},
    },
    process, sandbox,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

fn wait(client: &mut Client, check: impl Fn(&Client) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        client.poll();
        if check(client) {
            return;
        }
        assert!(
            client.error.is_empty(),
            "{}\n{}",
            client.error,
            client.output
        );
        assert!(
            Instant::now() < deadline,
            "Timed out: {}\n{}",
            client.status,
            client.output
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn make(root: &Path, peer: bool, isolated: bool, python: String, settings: Settings) -> Client {
    let mut options = sandbox::Options {
        enabled: isolated,
        ..Default::default()
    };
    if let Some(env) = Path::new(&python)
        .parent()
        .and_then(Path::parent)
        .filter(|p| p.join("pyvenv.cfg").exists())
    {
        options.read_only.push(env.to_string_lossy().into());
    }
    if let Ok(adapter) = std::env::var("RODE_TEST_LLDB") {
        options.read_only.push(
            Path::new(&adapter)
                .parent()
                .unwrap()
                .to_string_lossy()
                .into(),
        );
    }
    let plan = sandbox::prepare(root.to_str().unwrap(), &options).unwrap();
    if isolated && !plan.active {
        eprintln!("bwrap unavailable: {}", plan.label());
    }
    let argv = if peer {
        vec![
            "python3".into(),
            "-u".into(),
            "-c".into(),
            include_str!("../src/remote_debugger.py").into(),
        ]
    } else {
        let helper = root.join("rode-helper");
        fs::hard_link(env!("CARGO_BIN_EXE_rode"), &helper)
            .or_else(|_| fs::copy(env!("CARGO_BIN_EXE_rode"), &helper).map(|_| ()))
            .unwrap();
        vec![helper.to_string_lossy().into(), "--debug-helper".into()]
    };
    let session = process::Session::start(
        process::Launch {
            argv,
            plan,
            remote: None,
            python: "off".into(),
            python_program: false,
            line_limit: 2 * 1024 * 1024,
            native_debug: false,
        },
        Arc::new(|| {}),
    )
    .unwrap();
    Client::from_session(
        session,
        Start {
            root: root.to_string_lossy().into(),
            file: root.join("main.py").to_string_lossy().into(),
            python,
            config: settings.config.clone(),
        },
        settings,
    )
    .unwrap()
}
fn setup() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("project ' 🦀")).unwrap();
    fs::write(temp.path().join("private.txt"), "private").unwrap();
    temp
}
fn stopped(client: &Client) -> bool {
    client.phase == Phase::Paused && !client.scopes.is_empty() && !client.variables.is_empty()
}
fn finish(client: &mut Client, root: &Path, attach: bool) {
    // Bubblewrap gives the adapter a different PID namespace. Observe the
    // actual child in the test runner's /proc, rather than an unrelated PID.
    let pid = fixture_process::host_pid(&root.join("adapter-child"));
    client.stop();
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.cleanup().load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "cleanup timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
    let args: Value =
        serde_json::from_slice(&fs::read(root.join("disconnected.json")).unwrap()).unwrap();
    assert_eq!(args["terminateDebuggee"], !attach);
    assert!(
        !fixture_process::running(pid),
        "Detached adapter child survived"
    );
}
#[test]
fn protocol_terminal_breakpoints_steps_and_scoped_cleanup() {
    for peer in [false, true] {
        for isolated in [false, true] {
            for attach in [false, true] {
                if isolated && attach {
                    continue;
                }
                let temp = setup();
                let root = temp.path().join("project ' 🦀");
                let source = root.join("main.py");
                fs::write(&source, "# λ\nanswer=42\nprint(answer)\n").unwrap();
                let adapter = root.join("adapter.py");
                fs::write(&adapter, include_str!("fixtures/debug_adapter.py")).unwrap();
                let mut settings = Settings::default();
                settings.config.backend = Backend::Custom;
                settings.config.adapter = "python3".into();
                settings.config.adapter_arguments = vec![adapter.to_string_lossy().into()];
                settings.config.attach = attach;
                settings.breakpoints.insert(
                    source.to_string_lossy().into(),
                    vec![
                        Breakpoint {
                            line: 3,
                            condition: "answer == 42".into(),
                            ..Default::default()
                        },
                        Breakpoint {
                            line: 99,
                            ..Default::default()
                        },
                    ],
                );
                settings.watches = vec!["answer".into()];
                let mut client = make(&root, peer, isolated, "auto".into(), settings);
                wait(&mut client, |c| stopped(c) && c.watch_values[0] == "42 🦀");
                assert_eq!(client.frames[0]["line"], 3);
                assert!(client.breakpoints.values().next().unwrap()[0].verified);
                assert!(!client.breakpoints.values().next().unwrap()[1].verified);
                client.load_variables(2).unwrap();
                wait(&mut client, |c| {
                    c.variables.get(&2).is_some_and(|v| !v.is_empty())
                });
                let mut terminal = None;
                let mut text = String::new();
                wait(&mut client, |c| {
                    c.terminal_events
                        .iter()
                        .any(|e| matches!(e, Event::Data { .. }))
                });
                let deadline = Instant::now() + Duration::from_secs(5);
                while !text.contains("Name: ") {
                    client.poll();
                    while let Some(e) = client.terminal_events.pop_front() {
                        match e {
                            Event::Terminal { id, .. } => terminal = Some(id),
                            Event::Data { data, .. } => text.push_str(&String::from_utf8_lossy(
                                &STANDARD.decode(data).unwrap(),
                            )),
                            _ => {}
                        }
                    }
                    assert!(Instant::now() < deadline, "No prompt: {text}");
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert!(text.contains("TTY=True"));
                let id = terminal.unwrap();
                client
                    .writer()
                    .unwrap()
                    .send(&json!({"op":"input","id":id,"data":STANDARD.encode("RODE λ\n")}))
                    .unwrap();
                wait(&mut client, |_| root.join("input-result").exists());
                assert_eq!(
                    fs::read_to_string(root.join("input-result")).unwrap(),
                    "RODE λ"
                );
                client.control("next").unwrap();
                wait(&mut client, |c| stopped(c) && c.frames[0]["line"] == 4);
                client.evaluate("late").unwrap();
                client.control("continue").unwrap();
                wait(&mut client, |c| c.output.contains("RUNNING λ"));
                assert!(client.frames.is_empty());
                assert!(!client.output.contains("STALE"));
                let log: Vec<Value> = fs::read_to_string(root.join("debug-rpc.jsonl"))
                    .unwrap()
                    .lines()
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect();
                let commands: Vec<_> = log.iter().filter_map(|v| v["command"].as_str()).collect();
                let pos = |s: &str| commands.iter().position(|c| *c == s).unwrap();
                assert!(pos(if attach { "attach" } else { "launch" }) < pos("configurationDone"));
                finish(&mut client, &root, attach);
            }
        }
    }
}
#[test]
fn real_debugpy_break_step_evaluate_and_interactive_input() {
    let Some(python) = std::env::var("RODE_TEST_DEBUGPY")
        .ok()
        .or_else(|| std::env::var("RODE_TEST_JUPYTER").ok())
    else {
        eprintln!("Set RODE_TEST_DEBUGPY for real debugpy checks");
        return;
    };
    for peer in [false, true] {
        for isolated in [false, true] {
            let temp = setup();
            let root = temp.path().join("project ' 🦀");
            let source = root.join("main.py");
            fs::write(&source,"answer = 40\nanswer += 2\nprint(answer, flush=True)\nname=input('Name: ')\nprint('HELLO '+name, flush=True)\nimport os; print('VENV='+os.environ.get('VIRTUAL_ENV',''), flush=True)\n").unwrap();
            let mut settings = Settings::default();
            settings.breakpoints.insert(
                source.to_string_lossy().into(),
                vec![Breakpoint {
                    line: 2,
                    ..Default::default()
                }],
            );
            settings.watches = vec!["answer".into()];
            let mut client = make(&root, peer, isolated, python.clone(), settings);
            wait(&mut client, |c| stopped(c) && c.watch_values[0] == "40");
            client.control("next").unwrap();
            wait(&mut client, |c| {
                stopped(c) && c.frames[0]["line"] == 3 && c.watch_values[0] == "42"
            });
            client.evaluate("answer * 2").unwrap();
            wait(&mut client, |c| c.output.contains("84"));
            client.control("continue").unwrap();
            let mut text = String::new();
            let mut id = None;
            let deadline = Instant::now() + Duration::from_secs(15);
            while !text.contains("Name: ") {
                client.poll();
                while let Some(e) = client.terminal_events.pop_front() {
                    match e {
                        Event::Terminal { id: i, .. } => id = Some(i),
                        Event::Data { data, .. } => {
                            text.push_str(&String::from_utf8_lossy(&STANDARD.decode(data).unwrap()))
                        }
                        _ => {}
                    }
                }
                assert!(
                    client.error.is_empty(),
                    "{}\n{}",
                    client.error,
                    client.output
                );
                assert!(Instant::now() < deadline, "{text}\n{}", client.output);
                std::thread::sleep(Duration::from_millis(10));
            }
            client
                .writer()
                .unwrap()
                .send(&json!({"op":"input","id":id.unwrap(),"data":STANDARD.encode("RODE\n")}))
                .unwrap();
            wait(&mut client, |c| c.phase == Phase::Ended);
            assert!(client.error.is_empty(), "{}", client.error);
            let deadline = Instant::now() + Duration::from_millis(600);
            while Instant::now() < deadline {
                client.poll();
                while let Some(event) = client.terminal_events.pop_front() {
                    if let Event::Data { data, .. } = event {
                        text.push_str(&String::from_utf8_lossy(&STANDARD.decode(data).unwrap()));
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                text.contains(&format!(
                    "VENV={}",
                    Path::new(&python)
                        .parent()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .display()
                )),
                "Selected environment was lost: {text}"
            );
            assert!(
                text.contains("HELLO RODE"),
                "Final terminal output was lost: {text}"
            );
        }
    }
}
#[test]
fn validation_and_placeholder_expansion() {
    let mut s = Settings::default();
    s.breakpoints.insert(
        "/main.py".into(),
        vec![Breakpoint {
            line: 0,
            ..Default::default()
        }],
    );
    assert!(s.validate().is_err());
    assert_eq!(
        debugger::replace("{file} {root}", &[("file", "{root}"), ("root", "/a")]),
        "{root} /a"
    );
    assert!(debugger::expand(&json!({"a":"{file}"}), &[("file", "λ")], 0).is_ok());
}

#[test]
fn real_lldb_native_breakpoints_and_steps() {
    let Ok(adapter) = std::env::var("RODE_TEST_LLDB") else {
        eprintln!("Set RODE_TEST_LLDB for real native checks");
        return;
    };
    for peer in [false, true] {
        for isolated in [false, true] {
            for (filename, contents, line) in [
                (
                    "main.c",
                    "#include <stdio.h>\nint main(void) {\n int answer = 40;\n answer += 2;\n printf(\"%d\\n\", answer);\n return 0;\n}\n",
                    4,
                ),
                (
                    "main.cpp",
                    "#include <cstdio>\nint main() {\n int answer = 40;\n answer += 2;\n printf(\"%d\\n\", answer);\n return 0;\n}\n",
                    4,
                ),
                (
                    "main.rs",
                    "fn main() {\n let mut answer = 40_i32;\n answer += 2;\n println!(\"{}\", answer);\n}\n",
                    3,
                ),
            ] {
                let temp = setup();
                let root = temp.path().join("project ' 🦀");
                let source = root.join(filename);
                fs::write(&source, contents).unwrap();
                let binary = root.join("program");
                let compiler = if filename.ends_with(".rs") {
                    std::env::var("RUSTC").unwrap_or("rustc".into())
                } else if filename.ends_with(".cpp") {
                    "c++".into()
                } else {
                    "cc".into()
                };
                let result = std::process::Command::new(compiler)
                    .args([
                        "-g",
                        source.to_str().unwrap(),
                        "-o",
                        binary.to_str().unwrap(),
                    ])
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                let mut settings = Settings::default();
                settings.config.backend = Backend::Lldb;
                settings.config.adapter = adapter.clone();
                settings.config.program = binary.to_string_lossy().into();
                settings.config.extra = json!({"disableASLR":false});
                settings.breakpoints.insert(
                    source.to_string_lossy().into(),
                    vec![Breakpoint {
                        line,
                        ..Default::default()
                    }],
                );
                settings.watches = vec!["answer".into()];
                let mut client = make(&root, peer, isolated, "auto".into(), settings);
                wait(&mut client, |c| {
                    stopped(c) && c.watch_values[0].contains("40")
                });
                client.control("next").unwrap();
                wait(&mut client, |c| {
                    stopped(c) && c.watch_values[0].contains("42")
                });
                client.evaluate("answer * 2").unwrap();
                wait(&mut client, |c| c.output.contains("84"));
                client.stop();
                let deadline = Instant::now() + Duration::from_secs(8);
                while client.cleanup().load(Ordering::Acquire) {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

#[test]
fn real_debugpy_attach_detaches_without_ending_the_external_process() {
    let Some(python) = std::env::var("RODE_TEST_DEBUGPY")
        .ok()
        .or_else(|| std::env::var("RODE_TEST_JUPYTER").ok())
    else {
        return;
    };
    for peer in [false, true] {
        let temp = setup();
        let root = temp.path().join("project ' 🦀");
        fs::write(root.join("main.py"),"import time\nanswer=40\nwhile True:\n answer += 1\n with open('alive','w') as f: f.write(str(answer))\n time.sleep(0.03)\n").unwrap();
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let mut settings = Settings::default();
        settings.config.attach = true;
        settings.config.program.clear();
        settings.config.extra = json!({"listen":{"host":"127.0.0.1","port":port}});
        settings.breakpoints.insert(
            root.join("main.py").to_string_lossy().into(),
            vec![Breakpoint {
                line: 4,
                ..Default::default()
            }],
        );
        settings.watches = vec!["answer".into()];
        let mut client = make(&root, peer, false, python.clone(), settings);
        wait(&mut client, |c| {
            c.status.starts_with("Waiting for debug server")
        });
        let external = std::process::Command::new(&python)
            .args([
                "-m",
                "debugpy",
                "--connect",
                &format!("127.0.0.1:{port}"),
                "main.py",
            ])
            .current_dir(&root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        struct Owned(std::process::Child);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        // Transfer ownership before any assertion so failures cannot leave a job.
        let pid = external.id();
        let mut owned = Owned(external);
        wait(&mut client, stopped);
        client.stop();
        let deadline = Instant::now() + Duration::from_secs(8);
        while client.cleanup().load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            owned.0.try_wait().unwrap().is_none(),
            "Detach ended external PID {pid}"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while !fs::read_to_string(root.join("alive"))
            .is_ok_and(|v| v.parse::<u64>().is_ok_and(|v| v > 42))
        {
            assert!(
                Instant::now() < deadline,
                "External process remained suspended"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn attach_rejects_active_sandbox_before_starting_any_process() {
    let mut settings = Settings::default();
    settings.config.attach = true;
    let mut plan = sandbox::Plan::default();
    plan.active = true;
    plan.root = "/project".into();
    let result = Client::start(
        plan,
        None,
        String::new(),
        "auto".into(),
        settings,
        Arc::new(|| {}),
    );
    assert!(result.err().unwrap().to_string().contains("normal mode"));
}

#[test]
fn unread_terminal_input_cannot_block_step_or_owned_process_cleanup() {
    for peer in [false, true] {
        let temp = setup();
        let root = temp.path().join("project ' 🦀");
        fs::write(root.join("main.py"), "answer=42\n").unwrap();
        let adapter = root.join("adapter.py");
        fs::write(&adapter, include_str!("fixtures/debug_adapter.py")).unwrap();
        let mut settings = Settings::default();
        settings.config.backend = Backend::Custom;
        settings.config.adapter = "python3".into();
        settings.config.adapter_arguments = vec![adapter.to_string_lossy().into()];
        settings.config.extra = json!({"blockedInput":true});
        let mut client = make(&root, peer, false, "auto".into(), settings);
        wait(&mut client, stopped);
        let mut id = None;
        let mut output = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !output.contains("RAWREADY") {
            client.poll();
            while let Some(event) = client.terminal_events.pop_front() {
                match event {
                    Event::Terminal { id: value, .. } => id = Some(value),
                    Event::Data { data, .. } => {
                        output.push_str(&String::from_utf8_lossy(&STANDARD.decode(data).unwrap()))
                    }
                    _ => {}
                }
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let message =
            json!({"op":"input","id":id.unwrap(),"data":STANDARD.encode(vec![b'a';16384])});
        let writer = client.writer().unwrap();
        for _ in 0..16 {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !writer.try_send(&message).unwrap() {
                client.poll();
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        client.control("next").unwrap();
        wait(&mut client, |c| stopped(c) && c.frames[0]["line"] == 4);
        finish(&mut client, &root, false);
    }
}

#[test]
fn valid_maximum_breakpoints_survive_a_slow_adapter() {
    for peer in [false, true] {
        let temp = setup();
        let root = temp.path().join("project ' 🦀");
        fs::write(root.join("main.py"), "answer=42\n").unwrap();
        let adapter = root.join("adapter.py");
        fs::write(&adapter, include_str!("fixtures/debug_adapter.py")).unwrap();
        let mut settings = Settings::default();
        settings.config.backend = Backend::Custom;
        settings.config.adapter = "python3".into();
        settings.config.adapter_arguments = vec![adapter.to_string_lossy().into()];
        settings.config.extra = json!({"slowReader":true});
        for id in 0..128 {
            settings.breakpoints.insert(
                format!("{}/{}-{id}.py", root.display(), "path".repeat(50)),
                vec![Breakpoint {
                    line: 1,
                    ..Default::default()
                }],
            );
        }
        settings.validate().unwrap();
        let mut client = make(&root, peer, false, "auto".into(), settings);
        wait(&mut client, stopped);
        let messages: Vec<Value> = fs::read_to_string(root.join("debug-rpc.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            messages
                .iter()
                .filter(|v| v["command"] == "setBreakpoints")
                .count(),
            128
        );
        assert_eq!(
            messages
                .iter()
                .filter(|v| v["command"] == "configurationDone")
                .count(),
            1
        );
        finish(&mut client, &root, false);
    }
}

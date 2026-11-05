//! Older prompt pages stay independent of transcript limits and the live writer.
use rode::{
    agent::{Config, Launch},
    sandbox,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, Instant},
};

fn fixture(root: &std::path::Path, options: Value) -> Launch {
    fs::write(root.join("recall-source.json"), options.to_string()).unwrap();
    let program = root.join("history-peer");
    fs::write(&program, include_str!("fixtures/prompt_history_agent.py")).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    Launch {
        config: Config {
            codex: program.to_string_lossy().into(),
            ..Default::default()
        },
        plan: sandbox::Plan {
            root: root.to_string_lossy().into(),
            ..Default::default()
        },
        remote: None,
        python: "off".into(),
    }
}

fn poll(reader: &mut rode::agent::prompt_history::Reader) {
    let until = Instant::now() + Duration::from_secs(10);
    while reader.busy() {
        reader.poll();
        assert!(Instant::now() < until, "Prompt history peer timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn older_prompts_page_past_128_without_resuming_the_live_thread_or_losing_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let mut prompts: Vec<_> = (0..300).map(|i| format!("Prompt {i}")).collect();
    prompts[0] = "same prompt".into();
    prompts[1] = "same prompt".into();
    let launch = fixture(root.path(), json!({"prompts":prompts}));
    let mut reader = launch
        .prompt_history(
            "live-thread",
            vec![
                "Prompt 299\n\nAttachments: notes.txt".into(),
                "Failed before backend acceptance".into(),
            ],
            Arc::new(|| {}),
        )
        .unwrap();
    let mut restored = Vec::new();
    loop {
        poll(&mut reader);
        assert!(reader.take_error().is_none());
        let page = reader.take_prompts();
        restored.splice(0..0, page);
        if !reader.more() {
            break;
        }
        reader.load_more().unwrap();
    }
    assert_eq!(restored, prompts[..299]);
    let calls: Vec<Value> = fs::read_to_string(root.path().join("recall-rpc.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert!(calls.iter().all(|r| matches!(
        r["method"].as_str(),
        Some("initialize" | "initialized" | "thread/read" | "thread/turns/list")
    )));
    assert_eq!(
        calls
            .iter()
            .filter(|r| r["method"] == "thread/read")
            .count(),
        1
    );
}

#[test]
fn wrong_project_and_repeated_history_cursors_fail_without_writing_or_resuming() {
    for flag in ["wrong_root", "repeat_cursor"] {
        let root = tempfile::tempdir().unwrap();
        let launch = fixture(root.path(), json!({"prompts":["first","second"],flag:true}));
        let mut reader = launch
            .prompt_history("live-thread", vec!["second".into()], Arc::new(|| {}))
            .unwrap();
        poll(&mut reader);
        if reader.more() {
            reader.load_more().unwrap();
            poll(&mut reader);
        }
        assert!(reader.take_error().is_some());
        assert!(!reader.more());
    }
}

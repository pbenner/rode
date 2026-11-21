//! Conversation lifecycle, protocol-routing and bounded-output regressions.
use super::*;

#[test]
fn tool_lifecycle_survives_streaming_snapshots_and_turn_exit() {
    let mut chat = Chat {
        busy: true,
        ..Default::default()
    };
    let command =
        json!({"item":{"id":"command","type":"commandExecution","command":"cargo check"}});
    chat.codex_update("item/started", &command).unwrap();
    let running = chat.messages.back().unwrap().clone();
    assert_eq!(running.tool_state, Some(ToolState::Running));
    chat.codex_update("item/completed", &command).unwrap();
    let completed = chat.messages.back().unwrap();
    assert_eq!(completed.tool_state, Some(ToolState::Completed));
    assert_eq!(completed.text, running.text);
    assert_eq!(completed.title, running.title);
    assert_ne!(
        completed, &running,
        "A status-only change must reach native views"
    );
    let wire = serde_json::to_vec(&chat.view()).unwrap();
    let mut attached = Chat::default();
    attached.apply_view(serde_json::from_slice(&wire).unwrap());
    assert_eq!(attached.messages, chat.messages);

    chat.codex_update(
        "item/completed",
        &json!({"item":{
            "id":"failed-command","type":"commandExecution","exitCode":1,"status":"completed"
        }}),
    )
    .unwrap();
    assert_eq!(
        chat.messages.back().unwrap().tool_state,
        Some(ToolState::Failed)
    );
    chat.acp_update(
        &json!({"sessionUpdate":"tool_call","toolCallId":"acp","status":"in_progress"}),
    );
    assert_eq!(
        chat.messages.back().unwrap().tool_state,
        Some(ToolState::Running)
    );
    chat.acp_update(
        &json!({"sessionUpdate":"tool_call_update","toolCallId":"acp","status":"completed"}),
    );
    assert_eq!(
        chat.messages.back().unwrap().tool_state,
        Some(ToolState::Completed)
    );
    chat.acp_update(
        &json!({"sessionUpdate":"tool_call","toolCallId":"interrupted","status":"pending"}),
    );
    chat.finish(false, None);
    assert_eq!(
        chat.messages.back().unwrap().tool_state,
        Some(ToolState::Failed)
    );
    assert_eq!(chat.messages[0].tool_state, Some(ToolState::Completed));

    let legacy: Message = serde_json::from_value(json!({
        "id":"old-helper", "role":"Tool", "title":"Command", "text":"output"
    }))
    .unwrap();
    assert_eq!(legacy.tool_state, None);
}

#[test]
fn offline_guidance_covers_failed_turns_and_timeouts_for_every_provider() {
    for provider in [
        Provider::Codex,
        Provider::Local,
        Provider::Claude,
        Provider::Acp,
    ] {
        for offline in [false, true] {
            let mut chat = Chat {
                config: Config {
                    provider,
                    ..Default::default()
                },
                controller_offline: offline,
                busy: true,
                conversation: "saved-thread".into(),
                ..Default::default()
            };
            chat.finish(false, Some("Unable to connect to model"));
            assert!(!chat.busy);
            assert!(!chat.last_turn_succeeded);
            assert!(!chat.recoverable());
            assert_eq!(chat.conversation_id(), Some("saved-thread"));
            let text = &chat.messages.back().unwrap().text;
            assert!(text.starts_with("Unable to connect to model"));
            assert_eq!(text.contains("sandbox blocks network"), offline);

            chat.busy = true;
            chat.deadline = Some(Instant::now());
            chat.poll();
            assert!(chat.recoverable());
            assert!(!chat.busy);
            let text = &chat.messages.back().unwrap().text;
            assert!(text.starts_with("Agent startup timed out."));
            assert_eq!(text.contains("sandbox blocks network"), offline);
        }
    }
}

#[test]
fn routing_errors_keep_recovery_and_explain_catalog_failures_without_ending_the_chat() {
    for offline in [false, true] {
        let mut chat = Chat {
            controller_offline: offline,
            busy: true,
            conversation: "saved-thread".into(),
            last_stderr: "backend diagnostic".into(),
            ..Default::default()
        };
        chat.calls.insert(1, Call::Models);
        chat.receive(json!({"id":1,"error":{"message":"workspace routing discovery failed"}}))
            .unwrap();
        assert!(chat.busy);
        assert!(!chat.recoverable());
        assert!(
            !chat
                .messages
                .back()
                .unwrap()
                .text
                .contains("does not provide a model catalog")
        );

        chat.calls.insert(2, Call::Conversation);
        let error = chat
            .receive(json!({"id":2,"error":{"message":"workspace routing discovery failed"}}))
            .unwrap_err();
        chat.fail(&error.to_string());
        assert!(!chat.busy);
        assert!(chat.recoverable());
        assert_eq!(chat.conversation_id(), Some("saved-thread"));
        let text = &chat.messages.back().unwrap().text;
        assert!(text.starts_with("Agent: workspace routing discovery failed"));
        assert!(text.ends_with("backend diagnostic"));
        assert_eq!(text.contains("sandbox blocks network"), offline);
        assert_eq!(chat.config.network, super::super::network::Network::Project);
        let mut view = Chat::default();
        view.apply_view(chat.view());
        assert_eq!(view.messages.back().unwrap().text, *text);
    }
}

#[test]
fn questions_need_all_answers_and_preserve_request_lifetimes() {
    let mut chat = Chat {
        busy: true,
        conversation: "thread".into(),
        ..Default::default()
    };
    for (id, blocking) in [(401, false), (402, true)] {
        chat.receive(
            json!({"id":id,"method":"item/tool/requestUserInput","params":{
                "threadId":"thread", "isBlocking":blocking, "questions":[
                    {"id":"scope","header":"Scope","question":"Which tests?","options":[
                        {"label":"All","description":"Run everything"}]},
                    {"id":"note","header":"Note","question":"Anything else?","isSecret":true}
                ]
            }}),
        )
        .unwrap();
    }
    assert_eq!(chat.status, "Waiting for you");
    let prompt = &mut chat.prompts[0];
    assert!(prompt.answers().is_none());
    prompt.questions[0].answer = "All".into();
    prompt.questions[1].answer = "  ".into();
    assert!(prompt.answers().is_none());
    prompt.questions[1].answer = "Keep this exact text\nplease".into();
    assert_eq!(
        prompt.answers().unwrap(),
        json!({"answers":{
            "scope":{"answers":["All"]}, "note":{"answers":["Keep this exact text\nplease"]}
        }})
    );
    let mut incomplete = serde_json::to_value(&*prompt).unwrap();
    incomplete.as_object_mut().unwrap().remove("blocking");
    assert!(serde_json::from_value::<Prompt>(incomplete).is_err());
    chat.codex_update(
        "serverRequest/resolved",
        &json!({"threadId":"other","requestId":402}),
    )
    .unwrap();
    assert_eq!(chat.prompts.len(), 2);
    chat.codex_update(
        "serverRequest/resolved",
        &json!({"threadId":"thread","requestId":402}),
    )
    .unwrap();
    assert_eq!(chat.prompts.len(), 1);
    assert_eq!(chat.status, "Question awaiting your answer · agent working");
    chat.codex_update(
        "serverRequest/resolved",
        &json!({"threadId":"thread","requestId":401}),
    )
    .unwrap();
    assert!(chat.prompts.is_empty());
    assert_eq!(chat.status, "Working…");
    assert!(
        chat.busy,
        "Resolving a question must not complete the turn or release its queue"
    );
}

#[test]
fn escaped_prompt_history_keeps_snapshots_within_the_desktop_line_limit() {
    let mut chat = Chat::default();
    let text = "\0".repeat(crate::PROMPT_LIMIT);
    for n in 0..32 {
        let id = format!("user-{n}");
        chat.input_history.remember(&id, &text);
        chat.push(id, Role::User, "You".into(), text.clone());
    }
    assert!(serde_json::to_vec(&chat.view()).unwrap().len() < crate::LINE_LIMIT);
}

#[test]
fn stopping_a_slow_file_tool_does_not_block_or_publish_a_late_result() {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    struct Slow {
        started: mpsc::Sender<()>,
        cancelled: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl crate::Factory for Slow {
        fn start(&self, _: &Launch, _: bool, _: Arc<dyn Fn() + Send + Sync>) -> Result<Session> {
            unreachable!()
        }
        fn file_tool(&self, _: &str, _: &Value, cancel: &AtomicBool) -> Result<Value> {
            self.started.send(())?;
            let until = Instant::now() + Duration::from_secs(5);
            while !cancel.load(Ordering::Acquire) {
                ensure!(Instant::now() < until, "Cancellation was not delivered");
                std::thread::yield_now();
            }
            self.cancelled.send(())?;
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))?;
            Ok(json!({"late":true}))
        }
    }
    struct Capture(mpsc::Sender<Value>);
    impl crate::Transport for Capture {
        fn send(&self, message: &Value) -> Result<()> {
            self.0.send(message.clone())?;
            Ok(())
        }
        fn poll(&self) -> Option<Event> {
            None
        }
        fn stop(&self) {}
    }
    let (started, ready) = mpsc::channel();
    let (cancelled, stopped) = mpsc::channel();
    let (release, cleanup) = mpsc::channel();
    let (sent, received) = mpsc::channel();
    let mut chat = Chat {
        busy: true,
        conversation: "thread".into(),
        turn: "turn".into(),
        config: Config {
            access: Access::FilesOnly,
            ..Default::default()
        },
        session: Some(Session::new(Capture(sent))),
        file_factory: Some(Arc::new(Slow {
            started,
            cancelled,
            release: Mutex::new(cleanup),
        })),
        file_wake: Some(Arc::new(|| {})),
        ..Default::default()
    };
    for id in 10..15 {
        chat.file_request(json!(id), &json!({"tool":"rode_read_file","threadId":"thread","turnId":"turn","arguments":{"path":"slow"}})).unwrap();
    }
    for _ in 0..4 {
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    chat.cancel();
    for _ in 0..4 {
        stopped.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    assert!(chat.file_work_pending());
    assert!(chat.stopping());
    chat.disconnect();
    for _ in 0..4 {
        release.send(()).unwrap();
    }
    let until = Instant::now() + Duration::from_secs(5);
    while chat.file_work_pending() {
        chat.poll();
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    chat.poll();
    let replies: Vec<_> = received
        .try_iter()
        .filter(|v| (10..15).any(|id| v["id"] == id))
        .collect();
    assert_eq!(replies.len(), 5);
    assert!(
        replies
            .iter()
            .all(|reply| reply["result"]["success"] == false)
    );
    assert!(!chat.messages.iter().any(|m| m.text.contains("late")));
}

#[test]
fn only_successful_uninterrupted_turns_allow_follow_ups() {
    let mut chat = Chat::default();
    for (success, cancelling, expected) in [
        (true, false, true),
        (false, false, false),
        (true, true, false),
    ] {
        chat.busy = true;
        chat.cancelling = cancelling;
        chat.finish(success, None);
        assert_eq!(chat.last_turn_succeeded, expected);
        let mut attached = Chat::default();
        attached.apply_view(chat.view());
        assert_eq!(attached.last_turn_succeeded, expected);
    }
    chat.busy = true;
    chat.finish(true, None);
    chat.disconnect();
    assert!(!chat.last_turn_succeeded);
    let mut incomplete = serde_json::to_value(chat.view()).unwrap();
    incomplete
        .as_object_mut()
        .unwrap()
        .remove("last_turn_succeeded");
    assert!(serde_json::from_value::<crate::service::ChatView>(incomplete).is_err());
}

#[test]
fn streamed_unicode_is_bounded_even_at_small_remaining_limits() {
    let text = "🦀é界".repeat(100);
    for limit in 0..200 {
        assert!(clip(&text, limit).len() <= limit);
        let mut output = "x".repeat(limit.saturating_sub(2));
        append_bounded(&mut output, &text, limit);
        assert!(output.len() <= limit);
    }
}

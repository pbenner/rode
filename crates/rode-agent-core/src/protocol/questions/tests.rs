//! Exact message-based question wire format, acknowledgement races and lifetime.
use super::*;
use std::sync::mpsc;

struct Peer(mpsc::Sender<Value>);
impl crate::Transport for Peer {
    fn send(&self, value: &Value) -> Result<()> {
        self.0.send(value.clone())?;
        Ok(())
    }
    fn poll(&self) -> Option<Event> {
        None
    }
    fn stop(&self) {}
}

fn fixture() -> (Chat, mpsc::Receiver<Value>) {
    let (sent, received) = mpsc::channel();
    (
        Chat {
            session: Some(Session::new(Peer(sent))),
            conversation: "thread".into(),
            turn: "active-turn".into(),
            root: "/project".into(),
            busy: true,
            ..Default::default()
        },
        received,
    )
}

fn item() -> Value {
    json!({"type":"agentMessage","id":"question-item","text":"Network setting?","questions":[
        {"title":"Is network access enabled?","options":["Enabled","Disabled"]},
        {"title":"Which tests?"}
    ]})
}

fn ask(chat: &mut Chat) {
    chat.receive(json!({"method":"item/completed","params":{"threadId":"thread","turnId":"active-turn","item":item()}})).unwrap();
    assert_eq!(chat.prompts.len(), 1);
    assert!(chat.prompts[0].is_async_message());
    chat.prompts[0].questions[0].answer = "Disabled".into();
    chat.prompts[0].questions[1].answer = "Focused tests\nand preserve this line".into();
}

#[test]
fn selecting_one_async_answer_can_be_submitted_without_filling_the_other_question() {
    let (mut chat, received) = fixture();
    chat.message_questions(&item());
    chat.prompts[0].questions[0].answer = "Disabled".into();
    let answer = chat.prompts[0]
        .answers()
        .expect("One selected answer must enable asynchronous submission");
    chat.answer(answer).unwrap();
    let request = received.try_recv().unwrap();
    let text = request["params"]["input"][0]["text"].as_str().unwrap();
    assert!(text.contains("Disabled"));
    assert!(!text.contains("Which tests?"));
    chat.receive(json!({"id":request["id"],"result":{"turnId":"active-turn"}}))
        .unwrap();
    assert!(chat.prompts[0].questions[0].answered);
    assert!(!chat.prompts[0].questions[1].answered);
    assert!(!chat.prompts[0].sending);
    assert!(chat.prompts[0].answers().is_none());
    // Repeated message notifications and native view replacement must not
    // resurrect an already delivered answer or lose the remaining question.
    chat.message_questions(&item());
    let mut attached = Chat::default();
    attached.apply_view(chat.view());
    assert!(attached.prompts[0].questions[0].answered);
    chat.prompts[0].questions[1].answer = "Focused tests".into();
    let answer = chat.prompts[0].answers().unwrap();
    assert!(answer["answers"].get("question-0").is_none());
    chat.answer(answer).unwrap();
    let request = received.try_recv().unwrap();
    chat.receive(json!({"id":request["id"],"result":{"turnId":"active-turn"}}))
        .unwrap();
    assert!(chat.prompts.is_empty());
    assert_eq!(chat.input_history().count(), 2);
}

#[test]
fn older_controllers_require_complete_answers_and_unknown_or_repeated_ids_are_rejected() {
    let (mut chat, _) = fixture();
    ask(&mut chat);
    let mut legacy = serde_json::to_value(&chat.prompts[0]).unwrap();
    legacy.as_object_mut().unwrap().remove("partial_answers");
    let mut legacy: Prompt = serde_json::from_value(legacy).unwrap();
    legacy.questions[1].answer.clear();
    assert!(!legacy.partial_answers && legacy.answers().is_none());
    for answers in [json!({}), json!({"unknown":{"answers":["wrong"]}})] {
        assert!(chat.answer(json!({"answers":answers})).is_err());
        assert!(!chat.prompts[0].sending);
    }
    chat.prompts[0].questions[0].answered = true;
    assert!(
        chat.answer(json!({"answers":{"question-0":{"answers":["repeat"]}}}))
            .is_err()
    );
}

#[test]
fn refreshed_questions_do_not_reuse_drafts_after_the_question_or_secrecy_changes() {
    let (mut server, _) = fixture();
    server.message_questions(&item());
    let mut client = Chat::default();
    client.apply_view(server.view());
    client.prompts[0].questions[1].answer = "Answer to the old question".into();
    let mut replacement = item();
    replacement["questions"][1]["title"] = json!("Which directory should I use?");
    server.message_questions(&replacement);
    client.apply_view(server.view());
    assert!(client.prompts[0].questions[1].answer.is_empty());
    for secret in [true, false] {
        client.prompts[0].questions[1].answer = "An unsent private answer".into();
        let mut view = server.view();
        view.prompts[0].questions[1].secret = secret;
        client.apply_view(view);
        assert!(client.prompts[0].questions[1].answer.is_empty());
    }
    client.prompts[0].questions[0].answer = "Disabled".into();
    let mut view = server.view();
    view.prompts[0].questions[0].answered = true;
    client.apply_view(view);
    assert!(client.prompts[0].questions[0].answer.is_empty());
}

#[test]
fn asynchronous_forms_survive_completion_keep_drafts_and_deduplicate_notifications() {
    let (mut chat, _) = fixture();
    ask(&mut chat);
    chat.message_questions(&item());
    assert_eq!(chat.prompts.len(), 1);
    assert_eq!(chat.prompts[0].questions[0].answer, "Disabled");
    chat.receive(json!({"method":"item/completed","params":{"threadId":"other","item":item()}}))
        .unwrap();
    assert_eq!(chat.prompts.len(), 1);
    chat.finish(true, None);
    assert!(!chat.busy);
    assert_eq!(chat.status, "Question awaiting your answer");
    assert_eq!(chat.prompts.len(), 1);
    let mut attached = Chat::default();
    attached.apply_view(chat.view());
    attached.prompts[0].questions[1].answer = "An unsent draft".into();
    attached.apply_view(chat.view());
    assert_eq!(attached.prompts[0].questions[1].answer, "An unsent draft");
    chat.cancel();
    assert!(chat.prompts.is_empty());
    chat.message_questions(&item());
    assert!(
        chat.prompts.is_empty(),
        "Dismissed items must not reappear on duplicate delivery"
    );
}

#[test]
fn active_answers_steer_the_expected_turn_and_wait_for_acknowledgement() {
    let (mut chat, received) = fixture();
    ask(&mut chat);
    chat.answer(chat.prompts[0].answers().unwrap()).unwrap();
    assert!(chat.prompts[0].sending);
    assert!(chat.answer(chat.prompts[0].answers().unwrap()).is_err());
    let request = received.try_recv().unwrap();
    assert_eq!(request["method"], "turn/steer");
    assert_eq!(request["params"]["expectedTurnId"], "active-turn");
    assert!(
        request["params"]["input"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Focused tests\nand preserve this line")
    );
    assert!(
        received.try_recv().is_err(),
        "A second click must not send twice"
    );
    // Completion can arrive before the acknowledgement. The form remains
    // pending, preventing queued turns from overtaking the answer.
    chat.finish(true, None);
    assert!(chat.prompts[0].sending);
    chat.receive(json!({"id":request["id"],"result":{"turnId":"active-turn"}}))
        .unwrap();
    assert!(chat.prompts.is_empty());
    assert!(!chat.busy);
    assert_eq!(chat.input_history().count(), 1);
    assert!(chat.input_history().next().unwrap().contains("Disabled"));
    chat.message_questions(&item());
    assert!(chat.prompts.is_empty());
}

#[test]
fn idle_answers_start_a_turn_under_the_original_access_and_style() {
    for read_only in [false, true] {
        let (mut chat, received) = fixture();
        chat.read_only = read_only;
        chat.external = !read_only;
        chat.network = false;
        chat.config.personality = crate::Personality::JacksonLamb;
        ask(&mut chat);
        chat.finish(true, None);
        chat.answer(chat.prompts[0].answers().unwrap()).unwrap();
        let request = received.try_recv().unwrap();
        assert_eq!(request["method"], "turn/start");
        assert_eq!(
            request["params"]["sandboxPolicy"]["type"],
            if read_only {
                "readOnly"
            } else {
                "externalSandbox"
            }
        );
        if !read_only {
            assert_eq!(
                request["params"]["sandboxPolicy"]["networkAccess"],
                "restricted"
            );
        }
        assert!(
            request["params"]["input"]
                .to_string()
                .contains("jackson_lamb")
        );
        chat.receive(json!({"id":request["id"],"result":{"turn":{"id":"answer-turn"}}}))
            .unwrap();
        assert_eq!(chat.turn, "answer-turn");
        assert!(chat.busy);
        assert!(chat.prompts.is_empty());
    }
}

#[test]
fn rejected_and_timed_out_answers_keep_the_form_without_automatic_retries() {
    let (mut chat, received) = fixture();
    ask(&mut chat);
    chat.answer(chat.prompts[0].answers().unwrap()).unwrap();
    let request = received.try_recv().unwrap();
    chat.receive(
        json!({"id":request["id"],"error":{"code":-32600,"message":"No active turn to steer"}}),
    )
    .unwrap();
    assert!(chat.connected());
    assert!(!chat.prompts[0].sending);
    assert!(chat.prompts[0].error.contains("No active turn"));
    assert_eq!(chat.prompts[0].questions[0].answer, "Disabled");
    assert!(received.try_recv().is_err());
    chat.finish(true, None);
    chat.answer(chat.prompts[0].answers().unwrap()).unwrap();
    let request = received.try_recv().unwrap();
    assert_eq!(request["method"], "turn/start");
    if let Call::Answer { deadline, .. } = chat
        .calls
        .get_mut(&request["id"].as_u64().unwrap())
        .unwrap()
    {
        *deadline = Some(Instant::now());
    }
    chat.poll();
    assert!(
        chat.busy,
        "An unconfirmed turn must not be treated as rejected"
    );
    assert!(chat.prompts[0].sending);
    assert!(chat.prompts[0].error.contains("timed out"));
    assert!(
        !chat.has_deadline(),
        "An expired acknowledgement must not cause a polling loop"
    );
    assert!(chat.answer(chat.prompts[0].answers().unwrap()).is_err());
    assert!(chat.prompts[0].error.contains("timed out"));
    assert!(received.try_recv().is_err());
}

#[test]
fn late_answer_acknowledgements_resolve_the_form_after_a_timeout() {
    for idle in [false, true] {
        let (mut chat, received) = fixture();
        ask(&mut chat);
        if idle {
            chat.finish(true, None);
        }
        chat.answer(chat.prompts[0].answers().unwrap()).unwrap();
        let request = received.try_recv().unwrap();
        if let Call::Answer { deadline, .. } = chat
            .calls
            .get_mut(&request["id"].as_u64().unwrap())
            .unwrap()
        {
            *deadline = Some(Instant::now());
        }
        chat.poll();
        assert!(chat.prompts[0].sending);
        assert!(chat.answer(chat.prompts[0].answers().unwrap()).is_err());
        assert!(
            received.try_recv().is_err(),
            "An unconfirmed answer was sent twice"
        );
        if !idle {
            chat.finish(true, None);
        }
        let result = if idle {
            json!({"turn":{"id":"late-answer-turn"}})
        } else {
            json!({"turnId":"active-turn"})
        };
        chat.receive(json!({"id":request["id"],"result":result}))
            .unwrap();
        assert!(
            chat.prompts.is_empty(),
            "A late acknowledgement left a stale answer form"
        );
        assert_eq!(chat.input_history().count(), 1);
        assert_eq!(chat.busy, idle);
    }
}

#[test]
fn malformed_acknowledgements_cannot_release_unconfirmed_answers_for_resubmission() {
    for idle in [false, true] {
        let (mut chat, received) = fixture();
        ask(&mut chat);
        if idle {
            chat.finish(true, None);
        }
        chat.answer(chat.prompts[0].answers().unwrap()).unwrap();
        let request = received.try_recv().unwrap();
        chat.receive(json!({"id":request["id"],"result":{}}))
            .unwrap();
        assert!(chat.busy);
        assert!(chat.prompts[0].sending);
        assert!(chat.prompts[0].error.contains("invalid acknowledgement"));
        assert!(chat.answer(chat.prompts[0].answers().unwrap()).is_err());
        assert!(received.try_recv().is_err());
        chat.finish(true, None);
        chat.cancel();
        assert!(chat.prompts.is_empty());
        assert!(
            received.try_recv().is_err(),
            "Dismissing a question sent model input"
        );
    }
}

#[test]
fn cancelling_message_questions_never_sends_a_fabricated_rpc_reply() {
    let (mut chat, received) = fixture();
    ask(&mut chat);
    chat.cancel();
    let request = received.try_recv().unwrap();
    assert_eq!(request["method"], "turn/interrupt");
    assert!(received.try_recv().is_err());
    assert!(chat.prompts.is_empty());
}

#[test]
fn rejected_answers_keep_the_known_offline_policy_in_the_form() {
    let (mut chat, received) = fixture();
    chat.controller_offline = true;
    ask(&mut chat);
    chat.answer(chat.prompts[0].answers().unwrap()).unwrap();
    let request = received.try_recv().unwrap();
    chat.receive(json!({"id":request["id"],"error":{"code":-32603,"message":"workspace routing discovery failed"}})).unwrap();
    assert!(
        chat.prompts[0]
            .error
            .contains("workspace routing discovery failed")
    );
    assert!(chat.prompts[0].error.contains("sandbox blocks network"));
    assert!(chat.prompts[0].error.contains("Model connection only"));
    assert!(!chat.prompts[0].sending);
    assert!(chat.connected());
}

#[test]
fn invalid_answers_explain_the_error_inside_the_form_and_keep_the_draft() {
    let (mut chat, received) = fixture();
    ask(&mut chat);
    for question in &mut chat.prompts[0].questions {
        question.answer = "界".repeat(8192);
    }
    assert!(chat.answer(chat.prompts[0].answers().unwrap()).is_err());
    assert!(!chat.prompts[0].sending);
    assert!(chat.prompts[0].error.starts_with("Could not send answers:"));
    assert_eq!(chat.prompts[0].questions[0].answer, "界".repeat(8192));
    assert!(received.try_recv().is_err());
}

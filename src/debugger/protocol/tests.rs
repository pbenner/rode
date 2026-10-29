use super::*;
fn event(client: &mut Client, name: &str, body: Value) {
    client
        .handle(json!({"type":"event","event":name,"body":body}))
        .unwrap();
}
fn response(client: &mut Client, sequence: u64, body: Value) {
    client
        .handle(json!({"type":"response","request_seq":sequence,"success":true,"body":body}))
        .unwrap();
}
fn configuration_done(client: &Client) -> usize {
    client
        .queue
        .iter()
        .filter(|item| {
            serde_json::from_slice::<Value>(&item.bytes).unwrap()["message"]["command"]
                == "configurationDone"
        })
        .count()
}
#[test]
fn maximum_source_configuration_fits_and_finishes_once() {
    let mut client = Client::new(None, Settings::default());
    client.capabilities = json!({"supportsConfigurationDoneRequest":true});
    for i in 0..128 {
        client.settings.breakpoints.insert(
            format!("/source/{i}.rs"),
            vec![super::super::Breakpoint::default()],
        );
    }
    client.settings.validate().unwrap();
    client
        .request("launch", json!({}), Pending::Launch)
        .unwrap();
    event(&mut client, "initialized", json!({}));
    let initial: Vec<_> = client
        .pending
        .iter()
        .filter_map(|(id, (pending, _))| {
            matches!(pending, Pending::Breakpoints(..) | Pending::Configure).then_some(*id)
        })
        .collect();
    assert_eq!(initial.len(), 129);
    for sequence in initial {
        response(
            &mut client,
            sequence,
            json!({"breakpoints":[{"verified":true,"line":1}]}),
        );
    }
    assert_eq!(configuration_done(&client), 1);
}
#[test]
fn frame_changes_cancel_unsent_queries_and_ignore_inflight_replies() {
    let mut client = Client::default();
    client.phase = Phase::Paused;
    client.settings.watches = vec!["answer".into()];
    client.frames = vec![json!({"id":1}), json!({"id":2})];
    client.select_frame(1).unwrap();
    let obsolete = client.next - 1;
    client.select_frame(2).unwrap();
    assert_eq!(client.pending.len(), 2);
    assert_eq!(client.queue.len(), 2);
    response(&mut client, obsolete, json!({"result":"old frame"}));
    assert_eq!(client.watch_values, [""]);
    client.thread = Some(7);
    client.control("continue").unwrap();
    assert_eq!(client.queue.len(), 1);
    assert!(client.pending.values().all(|(p, _)| p.epoch().is_none()));
}
#[test]
fn changed_watches_cannot_receive_old_indexed_results() {
    let mut client = Client::default();
    client.phase = Phase::Paused;
    client.frame = Some(1);
    client.settings.watches = vec!["old".into()];
    client.refresh_watches().unwrap();
    let old = client.next - 1;
    client.settings.watches = vec!["new".into()];
    client.refresh_watches().unwrap();
    let new = client.next - 1;
    response(&mut client, new, json!({"result":"new value"}));
    response(&mut client, old, json!({"result":"wrong value"}));
    assert_eq!(client.watch_values, ["new value"]);
    assert_eq!(client.queue.len(), 1);
}
#[test]
fn runtime_breakpoint_changes_cannot_finish_initial_configuration_early() {
    let mut client = Client::new(None, Settings::default());
    client.capabilities = json!({"supportsConfigurationDoneRequest":true});
    client
        .settings
        .breakpoints
        .insert("/a.rs".into(), vec![super::super::Breakpoint::default()]);
    event(&mut client, "initialized", json!({}));
    let initial = client.breakpoint_requests["/a.rs"];
    let exceptions = client
        .pending
        .iter()
        .find(|(_, (p, _))| matches!(p, Pending::Configure))
        .map(|(id, _)| *id)
        .unwrap();
    client.sync_breakpoints("/a.rs").unwrap();
    let current = client.breakpoint_requests["/a.rs"];
    response(
        &mut client,
        current,
        json!({"breakpoints":[{"verified":true,"line":2}]}),
    );
    assert_eq!(configuration_done(&client), 0);
    response(
        &mut client,
        initial,
        json!({"breakpoints":[{"verified":true,"line":99}]}),
    );
    assert_eq!(client.breakpoints["/a.rs"][0].line, 2);
    assert_eq!(configuration_done(&client), 0);
    response(&mut client, exceptions, json!({}));
    assert_eq!(configuration_done(&client), 1);
}
#[test]
fn failed_variable_enqueue_leaves_the_scope_retryable() {
    let mut client = Client::default();
    client.phase = Phase::Paused;
    for _ in 0..MAX_REQUESTS {
        client
            .request("threads", json!({}), Pending::Threads(client.epoch))
            .unwrap();
    }
    assert!(client.load_variables(12).is_err());
    assert!(!client.variables.contains_key(&12));
    client.cancel_requests(|_| true);
    client.load_variables(12).unwrap();
    assert!(client.variables.contains_key(&12));
}

#[test]
fn refused_pause_does_not_claim_the_running_program_is_stopped() {
    let mut client = Client::default();
    client.phase = Phase::Running;
    client.thread = Some(7);
    client.control("pause").unwrap();
    let sequence = client.next - 1;
    client.handle(json!({"type":"response","request_seq":sequence,"success":false,"message":"Cannot pause"})).unwrap();
    assert_eq!(client.phase, Phase::Running);
    assert!(client.frames.is_empty());
}

#[test]
fn changing_breakpoints_resets_verification_until_the_new_reply() {
    let mut client = Client::new(None, Settings::default());
    client.initialized = true;
    client
        .settings
        .breakpoints
        .insert("/a.rs".into(), vec![super::super::Breakpoint::default()]);
    client.sync_breakpoints("/a.rs").unwrap();
    let first = client.next - 1;
    response(
        &mut client,
        first,
        json!({"breakpoints":[{"verified":true,"line":1}]}),
    );
    assert!(client.breakpoints["/a.rs"][0].verified);
    client.sync_breakpoints("/a.rs").unwrap();
    assert!(!client.breakpoints.contains_key("/a.rs"));
    client.settings.breakpoints.remove("/a.rs");
    client.sync_breakpoints("/a.rs").unwrap();
    let clear = client.next - 1;
    response(&mut client, clear, json!({"breakpoints":[]}));
    assert!(!client.breakpoint_requests.contains_key("/a.rs"));
}

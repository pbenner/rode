//! Correlated settings replies and preservation of held work.
use super::*;
use crate::agent::{Personality, response_style::Status};
use rode_agent_core::queue::{Action, Item};
use serde_json::json;

fn view(style: Personality) -> State {
    serde_json::from_value(json!({
        "info":{"id":"fixture","name":"Fixture","root":"/project","policy":"a".repeat(64),
            "mode":"NORMAL MODE","provider":"codex","alive":true,"busy":false,"status":"Ready",
            "attached":true,"persistent":true,"created":0,"history_browser":false},
        "chat":rode_agent_core::service::ChatView { config: Config { personality: style, codex:"fixture-agent".into(), ..Default::default() }, ..Default::default() },
        "queue":rode_agent_core::queue::View::default(),"history":null,"error":""
    })).unwrap()
}

#[test]
fn lifetime_changes_remain_pending_until_the_requested_mode_is_acknowledged() {
    let mut client = Client::test_style_view(Personality::Default, Personality::Default);
    client.persistence_pending = Some(Persistence::Auto);
    client.accept_state(view(Personality::Default));
    assert!(client.work_pending());
    assert_eq!(client.persistence(), Persistence::On);
    let mut acknowledged = view(Personality::Default);
    acknowledged.info.persistent = Persistence::Auto;
    client.accept_state(acknowledged);
    assert_eq!(client.persistence(), Persistence::Auto);
    assert!(client.persistence_pending.is_none());
    assert!(!client.work_pending());
}

impl Client {
    pub(crate) fn test_style_view(selected: Personality, applied: Personality) -> Self {
        let mut client = Self::pending(
            Launch {
                config: Config {
                    personality: selected,
                    codex: "fixture-agent".into(),
                    ..Default::default()
                },
                plan: crate::sandbox::Plan {
                    root: "/project".into(),
                    ..Default::default()
                },
                remote: None,
                python: "off".into(),
            },
            "fixture".into(),
            Persistence::On,
            Arc::new(|| {}),
        );
        client.preparing.pending.store(false, Ordering::Release);
        client.accept_state(view(applied));
        client
    }
}

fn push(id: u64) -> Action {
    Action::Push {
        id,
        text: format!("Keep prompt {id}"),
        attachments: vec![],
    }
}

#[test]
fn stale_snapshots_stay_pending_until_the_matching_settings_reply_and_fallback_blocks_work() {
    let mut client = Client::test_style_view(Personality::Default, Personality::Default);
    assert_eq!(
        client.response_style_status(Personality::Panne),
        Some(Status::Pending(Personality::Panne))
    );
    let mut config = client.launch.config.clone();
    config.personality = Personality::Panne;
    client.configure(&config).unwrap();
    let request = client.queue_requests[0].id.clone();
    client.accept_state(view(Personality::Default));
    assert_eq!(
        client.response_style_status(Personality::Panne),
        Some(Status::Pending(Personality::Panne))
    );
    assert!(client.queue_action(push(1)).is_err());
    let mut rejected = view(Personality::Default);
    rejected.queue.acknowledged = request;
    rejected.chat.busy = true;
    rejected.queue.items.push(Item {
        id: 9,
        text: "Already on the host".into(),
        files: vec![],
    });
    client.accept_state(rejected);
    let status = client.response_style_status(Personality::Panne).unwrap();
    assert_eq!(
        status,
        Status::Mismatch {
            selected: Personality::Panne,
            applied: Personality::Default
        }
    );
    assert!(status.message().contains("Panne"));
    assert!(status.message().contains("Default"));
    assert!(client.queue_action(push(2)).is_err());
    assert!(
        client
            .queue_action(Action::Pause { paused: false })
            .is_err()
    );
    assert!(
        client.state.as_ref().unwrap().chat.busy,
        "Never interrupt existing work to change its tone"
    );
    assert!(!client.ending);
    assert!(matches!(
        client.queue_requests.front().unwrap().operation,
        Action::Pause { paused: true }
    ));
    assert_eq!(client.state.as_ref().unwrap().queue.items.len(), 1);
}

#[test]
fn settings_confirmation_precedes_unsent_work_and_corrections_keep_payloads_and_pause_state() {
    let mut client = Client::test_style_view(Personality::Default, Personality::Default);
    client.queue_action(push(1)).unwrap();
    let mut config = client.launch.config.clone();
    config.personality = Personality::Panne;
    client.configure(&config).unwrap();
    config.personality = Personality::JacksonLamb;
    client.configure(&config).unwrap();
    assert!(
        matches!(client.queue_requests[0].operation,Action::Configure{ref config} if config.personality==Personality::Panne)
    );
    assert!(
        matches!(client.queue_requests[1].operation,Action::Configure{ref config} if config.personality==Personality::JacksonLamb)
    );
    assert!(matches!(
        client.queue_requests[2].operation,
        Action::Push { id: 1, .. }
    ));
    let mut first = view(Personality::Panne);
    first.queue.acknowledged = client.queue_requests[0].id.clone();
    client.accept_state(first);
    assert!(matches!(
        client.response_style_status(Personality::JacksonLamb),
        Some(Status::Pending(_))
    ));
    let mut second = view(Personality::Default);
    second.queue.acknowledged = client.queue_requests[0].id.clone();
    client.accept_state(second);
    assert!(matches!(
        client.response_style_status(Personality::JacksonLamb),
        Some(Status::Mismatch { .. })
    ));
    let mut paused = view(Personality::Default);
    paused.queue.acknowledged = client.queue_requests[0].id.clone();
    paused.queue.paused = true;
    client.accept_state(paused);
    config.personality = Personality::Default;
    client.configure(&config).unwrap();
    assert!(matches!(
        client.queue_requests[0].operation,
        Action::Configure { .. }
    ));
    let mut accepted = view(Personality::Default);
    accepted.queue.acknowledged = client.queue_requests[0].id.clone();
    accepted.queue.paused = true;
    client.accept_state(accepted);
    assert_eq!(client.response_style_status(Personality::Default), None);
    assert!(
        client.state.as_ref().unwrap().queue.paused,
        "Correction must not resume a paused queue"
    );
    assert!(
        matches!(&client.queue_requests[0].operation,Action::Push{id:1,text,..} if text=="Keep prompt 1")
    );
}

#[test]
fn initial_fallback_is_visible_and_reattachment_adopts_the_existing_hosts_actual_tone() {
    let mut client = Client::test_style_view(Personality::Panne, Personality::Default);
    assert!(client.require_response_style(Personality::Panne).is_err());
    assert_eq!(client.requested_style, Personality::Panne);
    client.adopt_style = true;
    client.accept_state(view(Personality::JacksonLamb));
    assert_eq!(client.requested_style, Personality::JacksonLamb);
    assert!(
        client
            .require_response_style(Personality::JacksonLamb)
            .is_ok()
    );
}

#[test]
fn held_payloads_can_be_saved_after_pause_without_resuming_work_or_blocking_clean_exit() {
    let mut client = Client::test_style_view(Personality::Default, Personality::Default);
    client.queue_action(push(1)).unwrap();
    let mut config = client.launch.config.clone();
    config.personality = Personality::Panne;
    client.configure(&config).unwrap();
    let mut state = view(Personality::Default);
    state.queue.acknowledged = client.queue_requests[0].id.clone();
    client.accept_state(state);
    assert!(matches!(
        client.queue_requests[0].operation,
        Action::Pause { paused: true }
    ));
    assert!(!client.queue_request_ready(&client.queue_requests[1]));
    let mut paused = view(Personality::Default);
    paused.queue.paused = true;
    paused.queue.acknowledged = client.queue_requests[0].id.clone();
    client.accept_state(paused);
    assert!(client.queue_request_ready(&client.queue_requests[0]));
    let mut saved = view(Personality::Default);
    saved.queue.paused = true;
    saved.queue.acknowledged = client.queue_requests[0].id.clone();
    saved.queue.items.push(Item {
        id: 1,
        text: "Keep prompt 1".into(),
        files: vec![],
    });
    client.accept_state(saved);
    assert!(!client.work_pending());
    assert!(client.state.as_ref().unwrap().queue.paused);
    assert_eq!(
        client.state.as_ref().unwrap().queue.items[0].text,
        "Keep prompt 1"
    );
    assert!(matches!(
        client.response_style_status(Personality::Panne),
        Some(Status::Mismatch { .. })
    ));
}

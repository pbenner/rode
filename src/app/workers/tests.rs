//! Worker floods retain FIFO replies while native close/input can run between batches.
use super::*;

fn profile_event(index: usize) -> Event {
    Event::Profiles(ProfileEvent::Changed {
        result: Ok(vec![Profile {
            name: index.to_string(),
            ..Default::default()
        }]),
        removed: None,
    })
}

#[test]
fn worker_flood_yields_to_close_and_delivers_every_batch_in_order() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file(); // The close guard must run even with worker replies waiting.
    let total = MAX_EVENTS_PER_FRAME * 2 + 3;
    for index in 0..total {
        app.tx.send(profile_event(index)).unwrap();
    }
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let output = ctx.run(input, |ctx| {
        app.poll(ctx);
        app.handle_close_request(ctx);
    });
    assert_eq!(
        app.connection_profiles.profiles[0].name,
        (MAX_EVENTS_PER_FRAME - 1).to_string()
    );
    assert!(matches!(app.pending, Some(Action::Quit)));
    assert!(!app.quitting);
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
    );
    let _ = ctx.run(Default::default(), |ctx| app.poll(ctx));
    assert_eq!(
        app.connection_profiles.profiles[0].name,
        (MAX_EVENTS_PER_FRAME * 2 - 1).to_string()
    );
    let _ = ctx.run(Default::default(), |ctx| app.poll(ctx));
    assert_eq!(
        app.connection_profiles.profiles[0].name,
        (total - 1).to_string()
    );
    assert!(matches!(app.rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn stale_replies_consume_the_budget_without_discarding_the_next_current_reply() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    for _ in 0..MAX_EVENTS_PER_FRAME {
        app.tx
            .send(Event::workspace(
                app.generation.wrapping_add(1),
                WorkspaceEvent::Connected {
                    result: Err("obsolete transport".into()),
                },
            ))
            .unwrap();
    }
    app.tx.send(profile_event(73)).unwrap();
    app.connection_profiles.profiles.clear();
    app.notice = None;
    let output = ctx.run(Default::default(), |ctx| app.poll(ctx));
    assert!(app.connection_profiles.profiles.is_empty());
    assert!(app.notice.is_none());
    assert_eq!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        Duration::ZERO
    );
    let _ = ctx.run(Default::default(), |ctx| app.poll(ctx));
    assert_eq!(app.connection_profiles.profiles[0].name, "73");
}

#[test]
fn stale_context_opens_cannot_clear_new_pending_reads_or_replace_retained_buffers() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    let retained = app.documents.selected_id().unwrap();
    app.documents.active_mut().unwrap().text = "retained source".into();
    let old_generation = app.generation;
    app.documents.begin_open("current.txt".into());
    app.clear_project(documents::Retention::Keep);
    assert!(!app.documents.has_pending_opens());
    app.documents.begin_open("current.txt".into());
    app.tx
        .send(Event::workspace(
            old_generation,
            WorkspaceEvent::Done {
                job: Job::Open("current.txt".into()),
                result: Box::new(Ok(Response {
                    path: "current.txt".into(),
                    text: "obsolete source".into(),
                    ..Default::default()
                })),
            },
        ))
        .unwrap();
    app.poll(&ctx);
    assert!(app.documents.is_opening("current.txt"));
    assert_eq!(app.documents.selected_id(), Some(retained));
    assert_eq!(app.documents.len(), 1);
    assert_eq!(app.documents.active().unwrap().text, "retained source");
    app.tx
        .send(Event::workspace(
            app.generation,
            WorkspaceEvent::Done {
                job: Job::Open("current.txt".into()),
                result: Box::new(Ok(Response {
                    path: "current.txt".into(),
                    text: "current source".into(),
                    ..Default::default()
                })),
            },
        ))
        .unwrap();
    app.poll(&ctx);
    assert!(!app.documents.has_pending_opens());
    assert_eq!(app.documents.len(), 2);
    assert_eq!(app.documents.active().unwrap().text, "current source");
}

#[test]
fn a_disconnect_during_a_batch_rejects_later_host_replies_but_keeps_global_completions() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.workspace = Some(crate::app::tests::remote_workspace());
    app.new_file();
    let id = app.documents.selected_id().unwrap();
    app.documents.active_mut().unwrap().text = "unsaved source".into();
    let generation = app.generation;
    app.tx
        .send(Event::workspace(
            generation,
            WorkspaceEvent::Connected {
                result: Err("lost connection".into()),
            },
        ))
        .unwrap();
    app.tx
        .send(Event::workspace(
            generation,
            WorkspaceEvent::Done {
                job: Job::Save {
                    id,
                    text: "obsolete source".into(),
                },
                result: Box::new(Ok(Response {
                    path: "old-host.txt".into(),
                    revision: "old".into(),
                    ..Default::default()
                })),
            },
        ))
        .unwrap();
    app.tx.send(profile_event(81)).unwrap();
    app.poll(&ctx);
    assert!(app.workspace.is_none());
    assert!(app.generation > generation);
    assert!(app.documents.active().unwrap().path.is_none());
    assert_eq!(app.documents.active().unwrap().text, "unsaved source");
    assert!(app.documents.active().unwrap().dirty());
    assert_eq!(app.connection_profiles.profiles[0].name, "81");
}

//! Commit editing across asynchronous host refreshes and changing status layout.
use super::*;

struct View {
    app: EditorApp,
    ctx: egui::Context,
    width: f32,
}

impl View {
    fn new(theme: &str, width: f32) -> Self {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.themes.select(theme, &ctx).unwrap();
        app.workbench.git.git_root = "/fixture".into();
        app.workbench.git.branch = "main".into();
        app.workbench.git.commit_message = "prefix replace suffix".into();
        Self { app, ctx, width }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        self.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(self.width, 1200.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| self.app.git_ui(ui, ctx));
            },
        )
    }

    fn click(&mut self, position: egui::Pos2) {
        for pressed in [true, false] {
            self.frame(vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
    }

    fn focus_message(&mut self) -> egui::Id {
        self.frame(vec![]);
        let output = self.frame(vec![]);
        let position = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape
                    && text.galley.text() == self.app.workbench.git.commit_message
                {
                    Some(text.pos + text.galley.size() / 2.)
                } else {
                    None
                }
            })
            .expect("Commit draft must be visible");
        self.click(position);
        self.ctx.memory(|memory| memory.focused()).unwrap()
    }

    fn select(&self, id: egui::Id, start: usize, end: usize) {
        let mut state = egui::TextEdit::load_state(&self.ctx, id).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(start),
                egui::text::CCursor::new(end),
            )));
        state.store(&self.ctx, id);
    }

    fn status(&mut self, status: &str) {
        self.app.handle_git(
            GitJob::Status,
            Ok(project::Response::Command(project::Output {
                stdout: status.into(),
                ..Default::default()
            })),
            &self.ctx,
        );
    }

    fn assert_focused(&self, id: egui::Id) {
        assert_eq!(self.ctx.memory(|memory| memory.focused()), Some(id));
    }
}

#[test]
fn commit_drafts_keep_focus_selection_and_typing_through_host_refreshes() {
    for theme in ["studio", "studio-neutral"] {
        for width in [360., 700.] {
            let mut view = View::new(theme, width);
            let id = view.focus_message();
            view.select(id, 7, 14);
            view.app.workbench.git.git_busy = true;
            view.frame(vec![egui::Event::Text("kept".into())]);
            view.assert_focused(id);
            assert_eq!(view.app.workbench.git.commit_message, "prefix kept suffix");
            view.status("## main\0");
            view.frame(vec![egui::Event::Text(" after refresh".into())]);
            view.assert_focused(id);
            assert_eq!(
                view.app.workbench.git.commit_message,
                "prefix kept after refresh suffix"
            );
        }
    }
}

#[test]
fn status_rows_and_error_banners_do_not_retarget_the_commit_editor() {
    let mut view = View::new("studio-neutral", 700.);
    let id = view.focus_message();
    view.select(id, 7, 14);
    view.status("## main\0?? new-file.txt\0 M tracked.txt\0");
    view.frame(vec![egui::Event::Text("new rows".into())]);
    view.assert_focused(id);
    assert_eq!(
        view.app.workbench.git.commit_message,
        "prefix new rows suffix"
    );
    view.app.handle_git(
        GitJob::Status,
        Err("Status refresh failed".into()),
        &view.ctx,
    );
    view.frame(vec![egui::Event::Text(" and error".into())]);
    view.assert_focused(id);
    view.status("## main\0");
    view.frame(vec![egui::Event::Text(" cleared".into())]);
    view.assert_focused(id);
    assert_eq!(
        view.app.workbench.git.commit_message,
        "prefix new rows and error cleared suffix"
    );
}

#[test]
fn pending_commits_lock_the_submitted_draft_and_failures_keep_it_for_retry() {
    let mut view = View::new("studio-neutral", 700.);
    let id = view.focus_message();
    view.select(id, 7, 14);
    view.app.workbench.git.git_busy = true;
    view.app.workbench.git.committing = true;
    view.frame(vec![egui::Event::Text(
        "must not replace submitted text".into(),
    )]);
    assert_eq!(
        view.app.workbench.git.commit_message,
        "prefix replace suffix"
    );
    view.app.handle_git(
        GitJob::Mutate {
            label: "Commit".into(),
            clear_message: true,
        },
        Err("Commit failed".into()),
        &view.ctx,
    );
    assert!(!view.app.workbench.git.git_busy);
    assert!(!view.app.workbench.git.committing);
    let id = view.focus_message();
    view.select(id, 7, 14);
    view.frame(vec![egui::Event::Text("retry".into())]);
    assert_eq!(view.app.workbench.git.commit_message, "prefix retry suffix");
    view.app.workbench.git.committing = true;
    view.app.workbench.git.amend = true;
    view.app.handle_git(
        GitJob::Mutate {
            label: "Commit".into(),
            clear_message: true,
        },
        Ok(project::Response::Command(project::Output::default())),
        &view.ctx,
    );
    assert!(!view.app.workbench.git.committing);
    assert!(view.app.workbench.git.commit_message.is_empty());
    assert!(!view.app.workbench.git.amend);
}

#[test]
fn conflict_actions_stay_disabled_while_a_host_request_is_in_flight() {
    let mut view = View::new("studio-neutral", 700.);
    view.status("## main\0UU conflicted.txt\0");
    view.app.workbench.git.git_busy = true;
    view.frame(vec![]);
    let output = view.frame(vec![]);
    for label in ["Abort merge", "Abort rebase"] {
        let position = layout::tests::button_frame(&output, label).0.center();
        view.click(position);
        assert!(view.app.workbench.git.confirm.is_none());
    }
}

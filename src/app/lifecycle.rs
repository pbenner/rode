//! Native window close requests follow the same guards as File → Quit.
//!
//! Native window-close events and menu/palette Quit share dirty-buffer, encrypted-lock and
//! in-flight terminal-operation checks. Explicit per-session persistence decides shell
//! lifetime without a repeated exit prompt.
use super::*;

mod exit;
pub(super) use exit::Exit;

impl EditorApp {
    /// Report an application quit currently waiting for owned lifetime acknowledgements.
    pub(super) fn exit_pending(&self) -> bool {
        self.exit.waiting()
    }
    /// Cancel actions waiting on failed acknowledgements and retain the error as a visible notice.
    pub(super) fn abort_lifecycle(&mut self, error: String, ctx: &egui::Context) {
        if self.pending.waiting_for_ack() {
            self.cancel_guarded_action();
            self.focus_editor(ctx);
        }
        if self.exit.waiting() {
            self.exit.cancel();
            self.dialog = Dialog::None;
            self.focus_editor(ctx);
        }
        self.notify(error, true);
    }
    /// Runtime views retain ordinary errors locally; a new error must also stop any
    /// ownership transition that is waiting on submitted work or cleanup.
    pub(super) fn report_runtime_failure(&mut self, error: Option<String>, ctx: &egui::Context) {
        if (self.exit_pending() || self.pending.waiting_for_ack())
            && let Some(error) = error
        {
            self.abort_lifecycle(error, ctx);
        }
    }
    /// Include pending terminal/runtime creation, lifetime changes and cleanup in the exit barrier.
    pub(super) fn runtime_work_pending(&self) -> bool {
        self.exit_readiness().runtime_pending()
    }
    fn final_quit(&mut self, ctx: &egui::Context) {
        self.exit.close();
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }
    /// Release foreground views/controllers and wait for lifetime acknowledgements before
    /// normal exit; persistent host work follows its existing acknowledged policy.
    pub(super) fn request_exit(&mut self, ctx: &egui::Context) {
        if !self.debugger.change_pending() {
            self.leave_debugger();
        }
        self.cancel_agent_installation();
        let readiness = self.exit_readiness();
        match self.exit.request(readiness, self.documents.changes()) {
            exit::Decision::Close => self.final_quit(ctx),
            exit::Decision::Wait => self.set_dialog(Dialog::TerminalExit),
        }
    }
    /// Keep pending exit cleanup visible and expose failures while the window still owns its work.
    pub(super) fn terminal_exit_dialog(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        ui.heading("Closing RODE");
        ui.spinner();
        ui.label(if self.agent_installation_pending() {
            "Finishing agent setup/lifetime changes and cleaning installation files…"
        } else {
            "Finishing submitted work and runtime lifetime changes…"
        });
        ui.small("RODE will close automatically when it completes.");
    }
    /// Advance only an existing quit intent once all domain owners acknowledge completion.
    pub(super) fn advance_exit(&mut self, ctx: &egui::Context) -> bool {
        if !self.exit.waiting() {
            return false;
        }
        match self
            .exit
            .advance(self.exit_readiness(), self.documents.changes())
        {
            exit::Progress::Idle => return false,
            exit::Progress::RecheckDocuments => {
                self.dialog = Dialog::None;
                self.guarded(Action::Quit, ctx);
            }
            exit::Progress::RetryCleanup => self.request_exit(ctx),
        }
        true
    }
    /// Aggregate narrow domain readiness without transferring their cleanup owners.
    pub(super) fn exit_readiness(&self) -> exit::Readiness {
        exit::Readiness {
            terminals: self.sessions.lifetime_pending(),
            runtimes: self.sessions.runtime_shutdown_pending(),
            notebooks: self.notebook_lifetime_pending(),
            debugger: self.debugger.lifetime_pending(),
            installation: self.agent_installation_pending(),
        }
    }
}

impl EditorApp {
    /// Cancel native close until the shared quit guards accept it. Wait for save
    /// acknowledgements before retrying a pending quit, then honor runtime/encrypted cleanup.
    pub(super) fn handle_close_request(&mut self, ctx: &egui::Context) {
        if self.exit.closing() {
            return;
        }
        let requested = ctx.input(|input| input.viewport().close_requested());
        if requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.dialog != Dialog::TerminalExit
                && !matches!(self.pending.as_ref(), Some(Action::Quit))
            {
                self.guarded(Action::Quit, ctx);
            }
        }
        self.advance_guarded_action(ctx);
    }
}

#[cfg(test)]
pub(in crate::app) mod tests {
    use super::*;

    /// Inject a native close event through the same lifecycle method used by the desktop backend.
    pub(in crate::app) fn native_close(
        app: &mut EditorApp,
        ctx: &egui::Context,
    ) -> egui::FullOutput {
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        ctx.run(input, |ctx| app.handle_close_request(ctx))
    }

    #[test]
    fn native_close_keeps_source_and_pending_mount_cleanup_guards() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        let output = native_close(&mut app, &ctx);
        assert!(!app.exit.closing());
        assert!(matches!(app.pending.as_ref(), Some(Action::Quit)));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
        );
        app.documents.clear();
        app.cancel_guarded_action();
        app.vaults.set_cleanup_required_fixture(true);
        native_close(&mut app, &ctx);
        assert!(!app.exit.closing());
        assert!(app.notice.as_ref().unwrap().0.contains("encrypted"));
        app.vaults.set_cleanup_required_fixture(false);
        native_close(&mut app, &ctx);
        assert!(app.exit.closing());
    }

    #[test]
    fn close_during_save_finishes_after_acknowledgement_without_another_close_event() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].fixture_content().text = "submitted".into();
        app.documents[0].fixture_content().saving = true;
        native_close(&mut app, &ctx);
        assert!(!app.exit.closing());
        assert!(matches!(app.pending.as_ref(), Some(Action::Quit)));

        app.documents[0].acknowledge_save("saved.txt".into(), "submitted".into(), "rev".into());
        let _ = ctx.run(Default::default(), |ctx| app.handle_close_request(ctx));
        assert!(app.exit.closing());
        assert!(app.pending.is_none());
    }

    #[test]
    fn failed_text_or_binary_save_cancels_a_waiting_quit_even_when_the_buffer_is_clean() {
        for binary in [false, true] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.new_file();
            let doc = app.documents.active_mut().unwrap();
            doc.acknowledge_save("original.txt".into(), "".into(), "rev".into());
            doc.fixture_content().saving = true;
            let id = doc.id;
            native_close(&mut app, &ctx);
            assert!(app.pending.waiting_for_save());
            assert!(!app.exit.waiting());

            let job = if binary {
                Job::SaveBinary {
                    id,
                    bytes: Arc::new(Vec::new()),
                    _lease: Arc::new(
                        Arc::new(crate::images::Budget::default())
                            .reserve_source(0)
                            .unwrap(),
                    ),
                }
            } else {
                Job::Save {
                    id,
                    text: String::new(),
                }
            };
            app.workers
                .workspace(&ctx)
                .send(WorkspaceEvent::Done {
                    job,
                    result: Box::new(Err("Save failed: permission denied".into())),
                })
                .unwrap();
            app.poll(&ctx);
            assert!(!app.exit.waiting());
            assert!(app.dialog == Dialog::None);
            assert!(!app.advance_exit(&ctx));
            assert!(!app.exit.closing());
            assert!(app.notice.as_ref().unwrap().0.contains("permission denied"));
        }
    }

    #[test]
    fn deferred_close_preserves_failed_saves_new_edits_and_cancelled_quits() {
        for outcome in ["failed", "new edits", "cancelled"] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.new_file();
            app.documents[0].fixture_content().text = "submitted".into();
            app.documents[0].fixture_content().saving = true;
            native_close(&mut app, &ctx);
            match outcome {
                "failed" => app.documents[0].fixture_content().saving = false,
                "new edits" => {
                    app.documents[0]
                        .fixture_content()
                        .text
                        .push_str(" and newer edits");
                    app.documents[0].acknowledge_save(
                        "saved.txt".into(),
                        "submitted".into(),
                        "rev".into(),
                    );
                }
                _ => {
                    app.cancel_guarded_action(); // Keep editing while the save completes.
                    app.documents[0].acknowledge_save(
                        "saved.txt".into(),
                        "submitted".into(),
                        "rev".into(),
                    );
                }
            }
            let _ = ctx.run(Default::default(), |ctx| app.handle_close_request(ctx));
            assert!(!app.exit.closing(), "{outcome}");
            assert_eq!(app.pending.is_some(), outcome != "cancelled", "{outcome}");
        }
    }

    #[test]
    fn deferred_close_still_requires_all_buffers_clean_and_mount_cleanup() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.new_file();
        native_close(&mut app, &ctx);
        app.documents[0].acknowledge_save("one.txt".into(), "".into(), "rev".into());
        let _ = ctx.run(Default::default(), |ctx| app.handle_close_request(ctx));
        assert!(!app.exit.closing());
        assert!(app.pending.is_some());
        app.documents[1].acknowledge_save("two.txt".into(), "".into(), "rev".into());
        app.vaults.set_cleanup_required_fixture(true);
        let output = native_close(&mut app, &ctx);
        assert!(!app.exit.closing());
        assert!(app.notice.as_ref().unwrap().0.contains("encrypted"));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
        );
    }
}

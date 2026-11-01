//! Native window close requests follow the same guards as File → Quit.
//!
//! Native window-close events and menu/palette Quit share dirty-buffer, encrypted-lock and
//! in-flight terminal-operation checks. Explicit per-session persistence decides shell
//! lifetime without a repeated exit prompt.
use super::*;

impl EditorApp {
    pub(super) fn handle_close_request(&mut self, ctx: &egui::Context) {
        if self.quitting {
            return;
        }
        let requested = ctx.input(|input| input.viewport().close_requested());
        // A clean buffer does not imply that encrypted mounts, startup work
        // or a debugger may be abandoned. Only the guarded quit closes us.
        if requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        // A save can reach disk before its acknowledgement reaches the UI. If
        // close arrived in between, finish the original request once every
        // buffer is clean. Failed saves and edits made during saving retain
        // the dirty-buffer guard; encrypted/runtime guards still run below.
        if matches!(self.pending, Some(Action::Quit))
            && !self.documents.iter().any(|doc| doc.dirty() || doc.saving)
        {
            self.pending = None;
            self.guarded(Action::Quit, ctx);
            return;
        }
        if requested && self.dialog != Dialog::TerminalExit {
            self.guarded(Action::Quit, ctx);
        }
    }
}

#[cfg(test)]
pub(in crate::app) mod tests {
    use super::*;

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
        assert!(!app.quitting);
        assert!(matches!(app.pending, Some(Action::Quit)));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
        );
        app.documents.clear();
        app.pending = None;
        app.vaults.needs_lock = true;
        native_close(&mut app, &ctx);
        assert!(!app.quitting);
        assert!(app.notice.as_ref().unwrap().0.contains("encrypted"));
        app.vaults.needs_lock = false;
        native_close(&mut app, &ctx);
        assert!(app.quitting);
    }

    #[test]
    fn close_during_save_finishes_after_acknowledgement_without_another_close_event() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].text = "submitted".into();
        app.documents[0].saving = true;
        native_close(&mut app, &ctx);
        assert!(!app.quitting);
        assert!(matches!(app.pending, Some(Action::Quit)));

        app.documents[0].acknowledge_save("saved.txt".into(), "submitted".into(), "rev".into());
        let _ = ctx.run(Default::default(), |ctx| app.handle_close_request(ctx));
        assert!(app.quitting);
        assert!(app.pending.is_none());
    }

    #[test]
    fn deferred_close_preserves_failed_saves_new_edits_and_cancelled_quits() {
        for outcome in ["failed", "new edits", "cancelled"] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.new_file();
            app.documents[0].text = "submitted".into();
            app.documents[0].saving = true;
            native_close(&mut app, &ctx);
            match outcome {
                "failed" => app.documents[0].saving = false,
                "new edits" => {
                    app.documents[0].text.push_str(" and newer edits");
                    app.documents[0].acknowledge_save(
                        "saved.txt".into(),
                        "submitted".into(),
                        "rev".into(),
                    );
                }
                _ => {
                    app.pending = None; // Keep editing while the save completes.
                    app.documents[0].acknowledge_save(
                        "saved.txt".into(),
                        "submitted".into(),
                        "rev".into(),
                    );
                }
            }
            let _ = ctx.run(Default::default(), |ctx| app.handle_close_request(ctx));
            assert!(!app.quitting, "{outcome}");
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
        assert!(!app.quitting);
        assert!(app.pending.is_some());
        app.documents[1].acknowledge_save("two.txt".into(), "".into(), "rev".into());
        app.vaults.needs_lock = true;
        let output = native_close(&mut app, &ctx);
        assert!(!app.quitting);
        assert!(app.notice.as_ref().unwrap().0.contains("encrypted"));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
        );
    }
}

//! Native window close requests follow the same guards as File → Quit.
//!
//! Native window-close events and menu/palette Quit share dirty-buffer, encrypted-lock and
//! in-flight terminal-operation checks. Explicit per-session persistence decides shell
//! lifetime without a repeated exit prompt.
use super::*;

impl EditorApp {
    pub(super) fn handle_close_request(&mut self, ctx: &egui::Context) {
        if self.quitting || !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }
        // A clean buffer does not imply that encrypted mounts, startup work
        // or a debugger may be abandoned. Only the guarded quit closes us.
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.dialog != Dialog::TerminalExit {
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
}

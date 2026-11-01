//! Project debugging UI. It never opens source through a different host backend.
//!
//! Managed controllers own protocol state on the host; the UI owns drafts, source jumps
//! and terminal views. Context changes detach persistent work or stop foreground work.
//! Stopped-epoch/buffer checks reject late source reads and stale execution markers.
mod configuration;
mod navigation;
mod terminals;
mod ui;
use super::*;
use crate::debugger::{
    self, Backend, Event as DebugEvent,
    protocol::{Client, Phase},
};
use std::collections::BTreeMap;

#[derive(Default)]
/// Disposable debugger attachment, source snapshot identities, terminal streams and
/// cleanup flags. Host protocol state can outlive this view when explicitly persistent.
pub(super) struct State {
    target: Option<crate::slurm::Target>,
    persistent: bool,
    reattaching: bool,
    source_digests: BTreeMap<String, String>,
    verify_sources: bool,
    pub client: Option<Client>,
    streams: BTreeMap<u64, (String, crate::terminal::Stream)>,
    serial: u64,
    cleanups: Vec<Arc<AtomicBool>>,
    versions: BTreeMap<u64, u64>,
    pending_jump: Option<navigation::Target>,
    source_loading: Option<String>,
    config: configuration::Draft,
    draft_generation: Option<u64>,
    watch: String,
    expression: String,
    was_live: bool,
}
impl State {
    /// Capture only a buffer whose current content matches the launched source.
    /// Saves change the baseline without advancing edit_version, so saved_text
    /// cannot establish which buffer version the debuggee actually uses.
    fn source_version(&self, doc: &Document) -> u64 {
        if self.verify_sources
            && doc
                .path
                .as_ref()
                .and_then(|path| self.source_digests.get(path))
                != Some(&crate::files::digest(doc.text.as_bytes()))
        {
            u64::MAX
        } else {
            doc.edit_version
        }
    }
}
impl EditorApp {
    /// Leave the current controller according to persistence and clear project-specific UI/configuration state.
    pub(super) fn reset_debugger(&mut self) {
        self.leave_debugger();
        self.debugger.client = None;
        self.debugger.versions.clear();
        self.debugger.draft_generation = None;
        self.debugger.config = Default::default();
        self.debugger.watch.clear();
        self.debugger.expression.clear();
        self.debugger.was_live = false;
        self.debugger.target = None;
        self.debugger.persistent = false;
        self.debugger.reattaching = false;
        self.debugger.source_digests.clear();
        self.debugger.verify_sources = false;
    }
    /// Detach a persistent controller or request foreground termination, then release
    /// its terminal views and pending source jump without changing unrelated shells.
    pub(super) fn leave_debugger(&mut self) {
        if self
            .debugger
            .client
            .as_ref()
            .is_some_and(Client::persistent)
        {
            if let Some(mut client) = self.debugger.client.take() {
                client.detach();
            }
            for (_, (key, stream)) in std::mem::take(&mut self.debugger.streams) {
                stream.end();
                self.remove_view(&key);
            }
            self.debugger.pending_jump = None;
            self.debugger.source_loading = None;
        } else {
            self.stop_debugger();
        }
    }
    /// Attach to the captured host runtime using its original policy and source digests;
    /// release the previous controller only after creating the replacement attachment.
    pub(in crate::app) fn attach_debugger_runtime(
        &mut self,
        info: rode_runtime_core::service::Info,
        ctx: &egui::Context,
    ) -> anyhow::Result<()> {
        let workspace = self
            .workspace
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Open the debugger's original project"))?;
        let plan = workspace.plan.clone();
        let remote = workspace.remote.clone();
        let target = info
            .target
            .clone()
            .map(serde_json::from_value)
            .transpose()?;
        let persistent = info.persistent;
        let sources = info.sources.clone();
        let wake = ctx.clone();
        let client = Client::attach(plan, remote, info, Arc::new(move || wake.request_repaint()))?;
        self.leave_debugger();
        self.debugger.client = Some(client);
        self.debugger.target = target;
        self.debugger.persistent = persistent;
        self.debugger.reattaching = true;
        self.debugger.serial += 1;
        self.debugger.source_digests = sources;
        self.debugger.verify_sources = true;
        self.debugger.versions = self
            .documents
            .iter()
            .map(|doc| (doc.id, self.debugger.source_version(doc)))
            .collect();
        self.workbench.panel = Some(Panel::Debug);
        self.workbench.surface = workbench::Surface::Tools;
        self.workbench.tools_hidden = false;
        Ok(())
    }
    /// Request explicit termination and retain cleanup flags while ending debugger-owned
    /// terminal streams; return immediately without claiming cleanup is complete.
    pub(super) fn stop_debugger(&mut self) {
        if let Some(client) = &mut self.debugger.client {
            client.stop();
            let flag = client.cleanup();
            if flag.load(Ordering::Acquire)
                && !self.debugger.cleanups.iter().any(|f| Arc::ptr_eq(f, &flag))
            {
                self.debugger.cleanups.push(flag);
            }
        }
        for (_, (key, stream)) in std::mem::take(&mut self.debugger.streams) {
            stream.end();
            self.remove_view(&key);
        }
        self.debugger.pending_jump = None;
        self.debugger.source_loading = None;
    }
    /// Validate the parsed configuration draft before replacing saved project debugger settings.
    fn apply_debug_config(&mut self) -> anyhow::Result<()> {
        let config = self.debugger.config.validated()?.clone();
        let mut settings = self.workbench.debugger_settings().clone();
        settings.config = config;
        settings.validate()?;
        self.workbench.save_debugger_settings(settings);
        Ok(())
    }
    /// Continue an existing live controller, otherwise validate saved sources, launch settings
    /// and host readiness before starting a controller under the captured project policy.
    pub(super) fn start_debugger(&mut self, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Debugger) {
            self.notify("Enable Debugger in View → Extensions.", false);
            return;
        }
        self.workbench.panel = Some(Panel::Debug);
        self.workbench.surface = workbench::Surface::Tools;
        self.workbench.tools_hidden = false;
        self.remember_tool_tab();
        if self
            .debugger
            .client
            .as_ref()
            .is_some_and(|c| c.phase != Phase::Ended)
        {
            self.debug_control("continue");
            return;
        }
        if !self.project_ready() {
            self.notify("Open a connected project to debug its programs.", false);
            return;
        }
        if self.documents.iter().any(Document::dirty) {
            self.notify(
                "Save modified sources before debugging. The debugger runs saved project files.",
                true,
            );
            return;
        }
        if self.debugger.draft_generation == Some(self.generation)
            && let Err(e) = self.apply_debug_config()
        {
            self.notify(format!("Invalid debugger configuration: {e}"), true);
            return;
        }
        self.stop_debugger();
        let workspace = self.workspace.as_ref().unwrap();
        let settings = self.workbench.debugger_settings().clone();
        let file = self
            .documents
            .get(self.active)
            .filter(|d| !d.is_preview())
            .and_then(|d| d.path.clone())
            .unwrap_or_default();
        let python = self.terminal_python_selection();
        let wake_ctx = ctx.clone();
        let sources = self
            .documents
            .iter()
            .filter_map(|doc| {
                doc.path.as_ref().map(|path| {
                    (
                        path.clone(),
                        crate::files::digest(doc.saved_text.as_bytes()),
                    )
                })
            })
            .take(128)
            .collect();
        match Client::start_managed(
            workspace.plan.clone(),
            workspace.remote.clone(),
            file,
            python,
            settings,
            self.debugger.target.clone(),
            self.debugger.persistent,
            sources,
            Arc::new(move || wake_ctx.request_repaint()),
        ) {
            Ok(client) => {
                self.debugger.verify_sources = false;
                self.debugger.client = Some(client);
                self.debugger.serial += 1;
                self.debugger.versions = self
                    .documents
                    .iter()
                    .map(|d| (d.id, d.edit_version))
                    .collect();
            }
            Err(e) => self.notify(format!("Cannot start debugger: {e:#}"), true),
        }
    }
    /// Submit a supported debugger control command and surface transport/state errors.
    pub(super) fn debug_control(&mut self, command: &str) {
        if let Some(client) = &mut self.debugger.client
            && let Err(e) = client.control(command)
        {
            self.notify(e.to_string(), true);
        }
    }
    /// Toggle a one-based source line in saved settings and synchronize the complete
    /// path breakpoint set, including an empty set to clear adapter state.
    pub(super) fn toggle_breakpoint(&mut self, path: String, line: usize) {
        if !self.feature(crate::extensions::Feature::Debugger) {
            return;
        }
        let mut settings = self.workbench.debugger_settings().clone();
        let items = settings.breakpoints.entry(path.clone()).or_default();
        if let Some(index) = items.iter().position(|b| b.line == line) {
            items.remove(index);
        } else {
            items.push(debugger::Breakpoint {
                line,
                ..Default::default()
            });
            items.sort_by_key(|b| b.line);
        }
        // A missing source is sent as an empty set to clear the adapter too.
        if items.is_empty() {
            settings.breakpoints.remove(&path);
        }
        if let Err(e) = settings.validate() {
            self.notify(e.to_string(), true);
            return;
        }
        self.workbench.save_debugger_settings(settings.clone());
        if let Some(client) = self
            .debugger
            .client
            .as_mut()
            .filter(|c| c.phase != Phase::Ended)
        {
            client.settings = settings;
            if let Err(e) = client.sync_breakpoints(&path) {
                client.error = e.to_string();
                client.stop();
            }
        }
    }
    /// Toggle the active saved source cursor line; unsaved/preview documents cannot bind a breakpoint path.
    pub(super) fn toggle_cursor_breakpoint(&mut self) {
        if let Some(doc) = self.documents.get(self.active).filter(|d| !d.is_preview())
            && let Some(path) = doc.path.clone()
        {
            self.toggle_breakpoint(path, doc.cursor.0);
        } else {
            self.notify("Open a saved source file to set a breakpoint.", false);
        }
    }
    /// Route debugger keys while respecting terminal focus and the presence of a live controller.
    pub(super) fn debugger_shortcuts(&mut self, ctx: &egui::Context, terminal_focus: bool) {
        if terminal_focus
            && !self
                .debugger
                .client
                .as_ref()
                .is_some_and(|c| c.phase != Phase::Ended)
        {
            return;
        }
        let consume = |mods, key| ctx.input_mut(|i| i.consume_key(mods, key));
        if consume(egui::Modifiers::SHIFT, Key::F5) {
            self.stop_debugger();
        } else if consume(egui::Modifiers::NONE, Key::F5) {
            self.start_debugger(ctx);
        }
        if consume(egui::Modifiers::NONE, Key::F6) {
            self.debug_control("pause");
        }
        if !terminal_focus && consume(egui::Modifiers::NONE, Key::F9) {
            self.toggle_cursor_breakpoint();
        }
        if consume(egui::Modifiers::NONE, Key::F10) {
            self.debug_control("next");
        }
        if consume(egui::Modifiers::SHIFT, Key::F11) {
            self.debug_control("stepOut");
        } else if consume(egui::Modifiers::NONE, Key::F11) {
            self.debug_control("stepIn");
        }
    }
    /// Return the paused selected frame's host source path and one-based line, or None if unavailable.
    pub(super) fn debug_line(&self) -> Option<(&str, usize)> {
        let client = self
            .debugger
            .client
            .as_ref()
            .filter(|c| c.phase == Phase::Paused)?;
        let frame = client
            .frames
            .iter()
            .find(|v| v["id"].as_i64() == client.frame)?;
        Some((
            frame["source"]["path"].as_str()?,
            frame["line"].as_u64()? as usize,
        ))
    }
    /// Suppress trusted execution markers when current source is dirty or differs from its launch version.
    pub(super) fn debug_source_stale(&self, doc: &Document) -> bool {
        self.debugger
            .client
            .as_ref()
            .is_some_and(|c| c.phase != Phase::Ended)
            && (doc.dirty()
                || self
                    .debugger
                    .versions
                    .get(&doc.id)
                    .is_some_and(|v| *v != doc.edit_version))
    }
    /// Include retired cleanup flags and the current controller's pending stop in lifecycle guards.
    pub(super) fn debugger_stopping(&self) -> bool {
        self.debugger
            .cleanups
            .iter()
            .any(|f| f.load(Ordering::Acquire))
            || self
                .debugger
                .client
                .as_ref()
                .is_some_and(Client::is_stopping)
    }
    /// Report a controller whose protocol phase has not ended; this is separate from cleanup pending.
    pub(super) fn debugger_active(&self) -> bool {
        self.debugger
            .client
            .as_ref()
            .is_some_and(|c| c.phase != Phase::Ended)
    }
    /// Drain controller events and cleanup flags, routing source/PTY updates only through
    /// the captured workspace and stopped inspection epoch.
    pub(super) fn poll_debugger(&mut self, ctx: &egui::Context) {
        self.debugger.cleanups.retain(|f| f.load(Ordering::Acquire));
        if self.debugger_stopping() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.debugger_active() && !self.project_ready() {
            self.leave_debugger();
            return;
        }
        let Some(mut client) = self.debugger.client.take() else {
            return;
        };
        let previous_error = client.error.clone();
        if client.terminal_events.is_empty() {
            client.poll();
        }
        if self.debugger.reattaching && client.attached_ready() {
            self.debugger.reattaching = false;
            self.workbench
                .save_debugger_settings(client.settings.clone());
            self.debugger.draft_generation = None;
        }
        self.debugger
            .streams
            .retain(|_, (key, _)| self.sessions.has_view(key));
        while let Some(event) = client.terminal_events.pop_front() {
            if let Some(blocked) = self.handle_debug_terminal(event, &mut client, ctx) {
                client.terminal_events.push_front(blocked);
                ctx.request_repaint_after(Duration::from_millis(16));
                break;
            }
        }
        if let Some((path, row, column)) = client.navigation.take() {
            self.debugger.pending_jump = Some(navigation::Target::new(
                path,
                row,
                column,
                client.inspection_epoch(),
            ));
        }
        self.resolve_debug_source(&client, ctx);
        if client.phase == Phase::Ended {
            for (_, stream) in self.debugger.streams.values() {
                stream.end();
            }
        }
        if client.phase == Phase::Starting || client.pending() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if self.session_exit_pending() && !client.error.is_empty() && client.error != previous_error
        {
            self.abort_session_quit(client.error.clone(), ctx);
        }
        self.debugger.client = Some(client);
    }
    /// Render cached inspection/configuration state, then apply returned actions after UI borrows end.
    pub(super) fn debugger_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.debugger.draft_generation != Some(self.generation) {
            self.debugger
                .config
                .load(&self.workbench.debugger_settings().config);
            self.debugger.draft_generation = Some(self.generation);
        }
        let host = self
            .workspace
            .as_ref()
            .map(|w| {
                format!(
                    "{} · {}",
                    w.remote
                        .as_ref()
                        .map(|r| format!("SSH {}", r.profile.endpoint()))
                        .unwrap_or_else(|| "LOCAL".into()),
                    w.plan.label()
                )
            })
            .unwrap_or_else(|| "No project open".into());
        let view = ui::Context {
            host: &host,
            ready: self.project_ready(),
            sandbox: self.workspace.as_ref().is_some_and(|w| w.plan.active),
            palette: self.terminal_colors.palette(self.themes.active()),
        };
        let drawn_epoch = self.debugger.client.as_ref().map(Client::inspection_epoch);
        ui.horizontal_wrapped(|ui| {
            ui.label("Execution target");
            let mut target = self.debugger.target.clone();
            let (queue, _) = self.slurm_programs();
            let runner = self.workbench.slurm_allocation().runner.clone();
            egui::ComboBox::from_id_salt("debugger-execution-target")
                .selected_text(target.as_ref().map(|t| format!("Slurm job {}", t.id))
                    .unwrap_or_else(|| "Project host".into()))
                .show_ui(ui, |ui| {
                ui.selectable_value(&mut target, None, "Project host");
                if let Some(snapshot) = &self.sessions.slurm {
                    for job in &snapshot.jobs {
                        if job.state == "RUNNING" && job.id.bytes().all(|b| b.is_ascii_digit()) {
                            ui.selectable_value(
                                &mut target,
                                Some(crate::slurm::Target {
                                    id: job.id.clone(), name: job.name.clone(),
                                    queue: queue.clone(), runner: runner.clone(),
                                }),
                                format!("Job {} · {}", job.id, job.nodes),
                            );
                        }
                    }
                }
            }).response.on_hover_text("The coordinator, adapter and launched program run together on this target. Changing it detaches persistent work; Start creates a new debugger.");
            if target != self.debugger.target {
                if let Some(client) = self.debugger.client.as_mut().filter(|c| c.lifetime_pending()) {
                    client.error = "Wait for submitted debugger changes before changing execution targets".into();
                } else {
                    self.leave_debugger();
                    self.debugger.client = None;
                    self.debugger.target = target;
                }
            }
            if ui.add_enabled(!self.sessions.slurm_busy, egui::Button::new("Refresh targets")).clicked() {
                self.refresh_slurm(None, ctx);
            }
            let on = self.debugger.client.as_ref().map_or(self.debugger.persistent, Client::persistent);
            if ui.add_enabled(
                self.debugger.client.as_ref().is_none_or(|c| !c.lifetime_pending()),
                egui::Button::new(if on { "Persist debugger: on" } else { "Persist debugger: off" }),
            ).clicked() {
                if let Some(client) = &mut self.debugger.client {
                    match client.set_persistent(!on) {
                        Ok(()) => self.debugger.persistent = !on,
                        Err(error) => client.error = error.to_string(),
                    }
                } else {
                    self.debugger.persistent = !on;
                }
            }
            if ui.small_button("Running debuggers…").clicked() {
                self.open_sessions(ctx);
            }
            if on && ui.add_enabled(
                self.debugger.client.as_ref().is_none_or(|c| !c.lifetime_pending()),
                egui::Button::new("Detach debugger").small(),
            ).clicked() {
                self.leave_debugger();
            }
        });
        if self
            .debugger
            .client
            .as_ref()
            .is_none_or(|c| c.phase == Phase::Ended)
            && ui.small_button("Reattach debugger").clicked()
        {
            self.find_debugger_runtime(ctx);
        }
        let actions = ui::show(
            ui,
            &mut self.debugger,
            self.workbench.edit_debugger_settings(),
            view,
        );
        if actions.settings_changed {
            match self.workbench.commit_debugger_settings() {
                Err(error) => self.notify(error.to_string(), true),
                Ok(()) => {
                    if actions.config_saved {
                        self.notify("Debugger configuration saved", false);
                    }
                    if let Some(client) = &mut self.debugger.client {
                        client.settings = self.workbench.debugger_settings().clone();
                        for path in actions.breakpoints {
                            if let Err(error) = client.sync_breakpoints(&path) {
                                client.error = error.to_string();
                                client.stop();
                                break;
                            }
                        }
                        if actions.watches_changed
                            && let Err(error) = client.refresh_watches()
                        {
                            client.error = error.to_string();
                        }
                    }
                }
            }
        }
        for message in actions.errors {
            self.notify(message, true);
        }
        for action in actions.commands {
            match action {
                ui::Action::Start => self.start_debugger(ctx),
                ui::Action::Stop => self.stop_debugger(),
                ui::Action::Control(command) => self.debug_control(command),
                ui::Action::ToggleBreakpoint => self.toggle_cursor_breakpoint(),
                command => {
                    if let Some(client) = &mut self.debugger.client {
                        let result = match command {
                            ui::Action::Thread(id) => client.select_thread(id),
                            ui::Action::Frame(id) => client.select_frame(id),
                            ui::Action::Evaluate(expression) => client.evaluate(&expression),
                            _ => unreachable!(),
                        };
                        if let Err(error) = result {
                            client.error = error.to_string();
                        }
                    }
                }
            }
            ctx.request_repaint();
        }
        if let Some(client) = &mut self.debugger.client
            && client.phase == Phase::Paused
            && drawn_epoch == Some(client.inspection_epoch())
        {
            for reference in actions.variables {
                match client.load_variables(reference) {
                    Ok(()) => ctx.request_repaint(),
                    Err(error) => {
                        let error = error.to_string();
                        if client.error != error {
                            client.error = error;
                            ctx.request_repaint();
                        }
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod source_tests {
    use super::*;

    #[test]
    fn reattached_source_versions_compare_current_buffers_and_remain_stale_after_save() {
        for current in ["answer = 42\n", "answer = 0\nanswer += 1\n"] {
            let original = "answer = 42\n";
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.new_file();
            let doc = &mut app.documents[0];
            doc.path = Some("/project/main.py".into());
            doc.saved_text = original.into();
            doc.text = current.into();
            doc.edit_version = 1;
            doc.modified = current != original;
            app.debugger.verify_sources = true;
            app.debugger.source_digests.insert(
                "/project/main.py".into(),
                crate::files::digest(original.as_bytes()),
            );
            let version = app.debugger.source_version(&app.documents[0]);
            app.debugger.versions.insert(app.documents[0].id, version);
            let mut client = Client::default();
            client.phase = Phase::Paused;
            app.debugger.client = Some(client);
            app.documents[0].acknowledge_save(
                "/project/main.py".into(),
                current.into(),
                "revision".into(),
            );
            assert_eq!(
                app.debug_source_stale(&app.documents[0]),
                current != original
            );
        }
    }

    #[test]
    fn unknown_sources_remain_stale_and_matching_new_sources_are_tracked() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].path = Some("/project/later.py".into());
        app.documents[0].text = "value = 1\n".into();
        app.documents[0].edit_version = 7;
        app.debugger.verify_sources = true;
        assert_eq!(app.debugger.source_version(&app.documents[0]), u64::MAX);
        app.debugger.source_digests.insert(
            "/project/later.py".into(),
            crate::files::digest(app.documents[0].text.as_bytes()),
        );
        assert_eq!(app.debugger.source_version(&app.documents[0]), 7);
        app.debugger.verify_sources = false;
        app.documents[0].text.push_str("value += 1\n");
        assert_eq!(app.debugger.source_version(&app.documents[0]), 7);
    }
}

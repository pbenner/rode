//! Project debugging UI. It never opens source through a different host backend.
//!
//! The client owns protocol state; the UI owns drafts, source jumps and terminal views.
//! Context changes stop the old client, and stopped-epoch/buffer checks prevent late source
//! reads or execution markers from selecting stale code.
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
pub(super) struct State {
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
impl EditorApp {
    pub(super) fn reset_debugger(&mut self) {
        self.stop_debugger();
        self.debugger.client = None;
        self.debugger.versions.clear();
        self.debugger.draft_generation = None;
        self.debugger.config = Default::default();
        self.debugger.watch.clear();
        self.debugger.expression.clear();
        self.debugger.was_live = false;
    }
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
    fn apply_debug_config(&mut self) -> anyhow::Result<()> {
        let config = self.debugger.config.validated()?.clone();
        let mut settings = self.workbench.debugger_settings().clone();
        settings.config = config;
        settings.validate()?;
        self.workbench.save_debugger_settings(settings);
        Ok(())
    }
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
        match Client::start(
            workspace.plan.clone(),
            workspace.remote.clone(),
            file,
            python,
            settings,
            Arc::new(move || wake_ctx.request_repaint()),
        ) {
            Ok(client) => {
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
    pub(super) fn debug_control(&mut self, command: &str) {
        if let Some(client) = &mut self.debugger.client
            && let Err(e) = client.control(command)
        {
            self.notify(e.to_string(), true);
        }
    }
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
    pub(super) fn toggle_cursor_breakpoint(&mut self) {
        if let Some(doc) = self.documents.get(self.active).filter(|d| !d.is_preview())
            && let Some(path) = doc.path.clone()
        {
            self.toggle_breakpoint(path, doc.cursor.0);
        } else {
            self.notify("Open a saved source file to set a breakpoint.", false);
        }
    }
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
    pub(super) fn debugger_active(&self) -> bool {
        self.debugger
            .client
            .as_ref()
            .is_some_and(|c| c.phase != Phase::Ended)
    }
    pub(super) fn poll_debugger(&mut self, ctx: &egui::Context) {
        self.debugger.cleanups.retain(|f| f.load(Ordering::Acquire));
        if self.debugger_stopping() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.debugger_active() && !self.project_ready() {
            self.stop_debugger();
            return;
        }
        let Some(mut client) = self.debugger.client.take() else {
            return;
        };
        if client.terminal_events.is_empty() {
            client.poll();
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
        self.debugger.client = Some(client);
    }
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

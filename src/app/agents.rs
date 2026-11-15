//! Project-scoped agent chat and preferences. Processes/transcripts are kept
//! out of app preferences; authentication remains with the installed agent.
//!
//! A submitted message or explicit History starts a process. Workspace/settings/feature changes stop the
//! old process; approvals remain bounded by the enclosing sandbox. Replies may refresh clean
//! disk-backed documents but never overwrite dirty buffers.
use super::*;
use crate::agent::{
    self, Config, Launch, Preferences, Provider,
    protocol::{Chat, Role},
};
#[cfg(test)]
use serde_json::json;
pub(super) mod attachments;
mod background;
mod composer;
mod controller;
mod controls;
mod header;
mod history;
mod message;
mod prompt_recall;
mod questions;
mod queue;
mod recovery;
mod runtime;
mod view;

/// Window-owned chat presentation, host-keyed configuration and disposable backend views.
/// Only bounded settings enter preferences; live transcripts and queued attachments
/// belong to the controller, and retired views retain unfinished lifetime acknowledgements.
pub(super) struct Agents {
    pub chat: Chat,
    catalog: Option<agent::controls::Catalog>,
    preferences: Preferences,
    config: Config,
    key: String,
    draft: String,
    composing: bool,
    composer: composer::State,
    queue: queue::Queue,
    pub(super) files: attachments::State,
    arguments: String,
    argument_error: String,
    settings: bool,
    pending: bool,
    pub focus: bool,
    completed: u64,
    local_root: String,
    browser: Option<agent::history::Browser>,
    history_open: bool,
    history_filter: String,
    history_scope: agent::history::Scope,
    history_delete: Option<history::Delete>,
    runtime: runtime::Runtime,
    running: background::Running,
    retired: Vec<Chat>,
    attaching: bool,
    prompt_reader: Option<agent::prompt_history::Reader>,
    prompt_reader_failed: bool,
}
impl Agents {
    /// Restore a bounded configuration map and initialize inert chat/history/setup views.
    pub(super) fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let preferences = storage
            .and_then(|s| s.get_string("agent_preferences"))
            .filter(|s| s.len() <= 256 * 1024)
            .and_then(|s| serde_json::from_str::<Preferences>(&s).ok())
            .filter(|p| p.len() <= 128)
            .unwrap_or_default();
        Self {
            chat: Chat::default(),
            catalog: None,
            preferences,
            config: Config::default(),
            key: String::new(),
            draft: String::new(),
            composing: false,
            composer: composer::State::default(),
            queue: queue::Queue::default(),
            files: attachments::State::default(),
            arguments: "[]".into(),
            argument_error: String::new(),
            settings: false,
            pending: false,
            focus: false,
            completed: 0,
            browser: None,
            history_open: false,
            history_filter: String::new(),
            history_scope: agent::history::Scope::Project,
            history_delete: None,
            runtime: runtime::Runtime::default(),
            running: Default::default(),
            retired: vec![],
            attaching: false,
            prompt_reader: None,
            prompt_reader_failed: false,
            local_root: std::env::current_dir()
                .unwrap_or_else(|_| files::expand_home("~"))
                .to_string_lossy()
                .into(),
        }
    }
    /// Write only agent configuration preferences, excluding transcripts, credentials and live jobs.
    pub(super) fn store(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(data) = serde_json::to_string(&self.preferences) {
            storage.set_string("agent_preferences", data);
        }
    }
    /// Record the current host/project configuration while keeping the saved map within
    /// its entry and serialized-byte budgets; mark the preference write pending.
    fn save_config(&mut self) {
        if !self.key.is_empty() {
            self.preferences
                .insert(self.key.clone(), self.config.clone());
            while self.preferences.len() > 128
                || serde_json::to_vec(&self.preferences).is_ok_and(|v| v.len() > 256 * 1024)
            {
                let key = self
                    .preferences
                    .keys()
                    .find(|key| *key != &self.key)
                    .cloned();
                if let Some(key) = key {
                    self.preferences.remove(&key);
                } else {
                    break;
                }
            }
            self.pending = true;
        }
    }
    /// Transfer pending queue edits to a hosted chat while transport capacity permits.
    /// Remove a command only after it is accepted; failures remain visible and retryable.
    fn flush_queue(&mut self) {
        if !self.queue.hosted() {
            return;
        }
        while self.chat.queue_capacity() {
            let Some(operation) = self.queue.command().cloned() else {
                break;
            };
            if agent::response_style::requires_confirmation(&operation)
                && self
                    .chat
                    .response_style_status(self.config.personality)
                    .is_some()
            {
                break;
            }
            match self.chat.queue_action(operation) {
                Ok(()) => self.queue.command_sent(),
                Err(error) => {
                    self.chat
                        .notice(&format!("Queue changes were not saved: {error:#}"));
                    break;
                }
            }
        }
    }
    /// New chat must not replace a working controller or an unfinished Stop.
    fn can_start_new_chat(&self) -> bool {
        !self.chat.busy && !self.chat.stopping()
    }

    /// Reset an idle conversation without starting a backend. Recheck here so
    /// a stale UI action cannot bypass the disabled header control.
    fn start_new_chat(&mut self) -> bool {
        if !self.can_start_new_chat() {
            return false;
        }
        self.browser = None;
        self.history_open = false;
        self.replace_chat();
        self.completed = 0;
        self.focus = true;
        true
    }

    /// Release the old conversation view and invalidate pending composer attachment reads.
    fn replace_chat(&mut self) {
        self.release_chat();
        self.files.clear();
    }
    /// Flush queued edits before dropping presentation state; retain a retiring chat
    /// while startup or lifetime work still requires acknowledgement.
    fn release_chat(&mut self) {
        self.flush_queue();
        self.composer.clear_history();
        self.prompt_reader = None;
        self.prompt_reader_failed = false;
        self.queue.clear();
        let old = std::mem::take(&mut self.chat);
        if old.work_pending() {
            old.cancel_startup_for_exit();
            self.retired.push(old);
        }
        self.attaching = false;
    }
}

impl EditorApp {
    /// Derive the execution root, display host and configuration key from the selected
    /// workspace; SSH configuration participates in the key to separate transport contexts.
    fn agent_location(&self) -> (String, String, String) {
        let root = self
            .workspace
            .as_ref()
            .map(|w| w.root.clone())
            .unwrap_or_else(|| self.agents.local_root.clone());
        let host = self
            .workspace
            .as_ref()
            .and_then(|w| w.remote.as_ref())
            .map(|r| {
                format!(
                    "SSH {}{}",
                    r.profile.endpoint(),
                    if r.profile.port.is_empty() {
                        String::new()
                    } else {
                        format!(":{}", r.profile.port)
                    }
                )
            })
            .unwrap_or_else(|| "LOCAL".into());
        let key = serde_json::to_string(&(
            host.as_str(),
            &root,
            self.workspace
                .as_ref()
                .and_then(|w| w.remote.as_ref())
                .map(|r| r.profile.config.as_str())
                .unwrap_or_default(),
        ))
        .unwrap_or_default();
        (root, host, key)
    }

    /// Apply cross-domain effects returned by the agent owner, then admit queued sends.
    pub(super) fn poll_agents(&mut self, ctx: &egui::Context) {
        let environment = self.agent_environment();
        let visible = self.panel_visible(Panel::Agent);
        let effects = self.agents.poll(
            &environment,
            visible,
            self.dialog == Dialog::AgentSessions,
            ctx,
        );
        for error in effects.failures {
            self.abort_lifecycle(error, ctx);
        }
        if effects.refresh_documents {
            self.reload_clean_documents(ctx);
        }
        self.agents.flush_queue();
        self.dispatch_agent_queue(ctx);
        self.agents.schedule_poll(ctx);
    }

    /// Invalidate discovery/queue state and disconnect the current controller after project settings change.
    pub(super) fn stop_project_agent(&mut self) {
        self.agents.stop_project()
    }

    /// Release the current conversation on a context change while retaining pending cleanup work.
    pub(super) fn leave_project_agent(&mut self) {
        self.agents.leave_project()
    }

    /// Clean exit lets owned setup workers release temporary files and locks.
    pub(super) fn cancel_agent_installation(&mut self) {
        self.agents.cancel_installation()
    }

    /// Include setup, confirmed background mutations and current/retired chat lifetime
    /// work when deciding whether normal application exit must wait.
    pub(super) fn agent_installation_pending(&self) -> bool {
        self.agents.installation_pending()
    }

    /// Queue changed configuration preferences and a storage flush, reporting unavailable storage.
    pub(super) fn persist_agent_preferences(
        &mut self,
        storage: Option<&mut (dyn eframe::Storage + 'static)>,
    ) {
        if !self.agents.pending {
            return;
        }
        self.agents.pending = false;
        if let Some(storage) = storage {
            self.agents.store(storage);
            storage.flush();
        } else {
            self.notify(
                "Agent settings applied, but preference storage is unavailable.",
                true,
            );
        }
    }

    /// Project immutable UI facts, render the agent owner, then apply cross-domain actions in order.
    pub(super) fn agent_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let environment = self.agent_environment();
        let (root, host, _) = self.agent_location();
        let Some(actions) = self.agents.render(
            ui,
            ctx,
            view::Facts {
                palette: self.themes.active().palette,
                root,
                host,
                environment: &environment,
                offline: self
                    .workspace
                    .as_ref()
                    .is_some_and(|w| !w.ready || (w.remote.is_some() && !w.connected)),
                unsaved: self.documents.iter().filter(|d| d.dirty()).count(),
                modal: self.dialog != Dialog::None
                    || self.pending.modal()
                    || self.workbench_modal(),
            },
        ) else {
            return;
        };
        if actions.save {
            self.save_all(ctx);
        }
        if actions.attach {
            self.set_dialog(Dialog::AgentAttachLocal);
        }
        if actions.discover_models {
            self.agents.discover_models(&environment, ctx);
        }
        if actions.send {
            if self.agents.runtime.installing() {
                self.notify(
                    "Wait for Codex installation to finish or cancel it before sending.",
                    false,
                );
            } else {
                self.send_agent(ctx);
            }
        }
        let running = actions.running || actions.history.running;
        self.agents.apply_history(
            actions.history,
            &environment,
            self.feature(crate::extensions::Feature::Agents),
            ctx,
        );
        if actions.refresh_documents {
            self.reload_clean_documents(ctx);
        }
        if running {
            self.open_background_agents(ctx);
        }
        if let Some(fresh) = actions.recover {
            self.agents.recover(&environment, fresh, ctx);
        }
        self.agents.flush_queue();
    }

    fn send_agent(&mut self, ctx: &egui::Context) {
        if self.feature(crate::extensions::Feature::Agents) {
            let environment = self.agent_environment();
            self.agents.send(&environment, ctx);
        }
    }

    /// Supply UI admission facts; the agent controller owns queue ordering and submission.
    fn dispatch_agent_queue(&mut self, ctx: &egui::Context) {
        let allowed = !self.exit.closing()
            && self.dialog == Dialog::None
            && !self.pending.modal()
            && !self.workbench_modal()
            && self.feature(crate::extensions::Feature::Agents)
            && self
                .workspace
                .as_ref()
                .is_none_or(|w| w.ready && (w.connected || w.remote.is_none()));
        let environment = self.agent_environment();
        self.agents.dispatch_queue(&environment, allowed, ctx);
    }

    fn agent_launch(&self) -> Launch {
        self.agents.launch(&self.agent_environment())
    }

    /// Capture project authority and interpreter selection without giving the controller application access.
    fn agent_environment(&self) -> controller::Environment {
        let (root, _, key) = self.agent_location();
        let target = execution::Target::capture(self.workspace.as_ref(), execution::Scope::Project);
        controller::Environment {
            key,
            plan: target.plan().cloned().unwrap_or_else(|| sandbox::Plan {
                root,
                ..Default::default()
            }),
            remote: target.remote().cloned(),
            python: self.agent_python(),
            has_project: self.workspace.is_some(),
        }
    }
}

/// Shared by the composer and question fields: consume only a fresh, unmodified Enter. egui's
/// consume_key ignores extra Shift/Alt, which would also consume newlines.
/// IME confirmation belongs to the text editor, including a commit+Enter frame.
fn text_submit(ui: &mut egui::Ui, id: egui::Id, modal: bool, composing: &mut bool) -> bool {
    if modal || !ui.memory(|m| m.has_focus(id)) {
        *composing = false;
        return false;
    }
    ui.input_mut(|input| {
        let mut ime_frame = *composing;
        for event in &input.events {
            if let egui::Event::Ime(event) = event {
                ime_frame = true;
                *composing = matches!(event, egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_));
            }
        }
        let mut send = false;
        input.events.retain(|event| {
            if !ime_frame
                && let egui::Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                    repeat,
                    ..
                } = event
            {
                send |= !repeat;
                false
            } else {
                true
            }
        });
        send
    })
}

/// Use one immutable text buffer so selection can span prose and fenced code.
fn render_message(
    ui: &mut egui::Ui,
    text: &str,
    palette: theme::Palette,
    copy_response: bool,
) -> egui::text_edit::TextEditOutput {
    let layout = message::layout(ui, text, palette);
    // A parent can request more width than the visible transcript, especially
    // during resizing. Wrap to the visible edge rather than that larger layout.
    let visible_width = ui
        .available_width()
        .min(ui.clip_rect().right() - ui.next_widget_position().x)
        .max(1.);
    let id = ui.make_persistent_id("message-text");
    let viewport =
        egui::Rect::from_min_size(ui.next_widget_position(), egui::vec2(visible_width, 0.));
    let render = |ui: &mut egui::Ui| {
        render_message_layout(ui, &layout, palette, copy_response, viewport, id)
    };
    if layout.has_tables() {
        egui::ScrollArea::horizontal()
            .id_salt(id.with("table-scroll"))
            .max_width(visible_width)
            .auto_shrink([false, true])
            .show(ui, render)
            .inner
    } else {
        render(ui)
    }
}

/// Keep one selection and clipboard path for wrapped prose and aligned table rows.
fn render_message_layout(
    ui: &mut egui::Ui,
    layout: &message::MessageLayout,
    palette: theme::Palette,
    copy_response: bool,
    viewport: egui::Rect,
    id: egui::Id,
) -> egui::text_edit::TextEditOutput {
    let job = &layout.job;
    let visible_width = viewport.width();
    let mut display = job.text.as_str();
    let mut layouter = |ui: &egui::Ui, _: &dyn egui::TextBuffer, width: f32| {
        let prose_offset = (viewport.left() - ui.next_widget_position().x).max(0.);
        layout.galley(ui, width.min(visible_width).max(1.), prose_offset)
    };
    let previous_selection =
        if ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary)) {
            egui::TextEdit::load_state(ui.ctx(), id)
                .and_then(|state| state.cursor.char_range())
                .filter(|range| !range.is_empty())
        } else {
            None
        };
    let commands_before = ui.ctx().output(|output| output.commands.len());
    let mut output = egui::TextEdit::multiline(&mut display)
        .id(id)
        .frame(false)
        .margin(egui::Vec2::ZERO)
        .desired_width(visible_width)
        .desired_rows(1)
        .layouter(&mut layouter)
        .show(ui);
    crate::keyboard::field_target(ui, "Message text", &output.response);
    // TextEdit moves the caret on any pointer press, including a context-menu click.
    if output.response.contains_pointer()
        && let Some(selection) = previous_selection
    {
        output.state.cursor.set_char_range(Some(selection));
        output.cursor_range = output.state.cursor.range(&output.galley);
        output.state.clone().store(ui.ctx(), id);
    }
    layout.paint(ui, &output, palette);
    // TextEdit owns keyboard selection/copy. Replace only commands it just emitted,
    // expanding the single-character formula slots into portable TeX source.
    if let Some(range) = output
        .state
        .cursor
        .char_range()
        .filter(|range| !range.is_empty())
    {
        ui.ctx().output_mut(|output| {
            for command in output.commands.iter_mut().skip(commands_before) {
                if let egui::OutputCommand::CopyText(text) = command {
                    *text = layout.copy(range.as_sorted_char_range());
                }
            }
        });
    }
    if copy_response {
        ui.ctx().copy_text(layout.copy(0..job.text.chars().count()));
    }
    crate::keyboard::context_menu(&output.response, |ui| {
        // The popup can take keyboard focus; use the stored, clamped selection.
        let selection = output
            .state
            .cursor
            .range(&output.galley)
            .filter(|range| !range.is_empty());
        if ui
            .add_enabled(
                selection.is_some(),
                crate::keyboard::Button::new("Copy selection"),
            )
            .clicked()
        {
            if let Some(selection) = selection {
                ui.ctx()
                    .copy_text(layout.copy(selection.as_sorted_char_range()));
            }
            ui.close();
        }
    });
    output
}

#[cfg(test)]
mod tests;

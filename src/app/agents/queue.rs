//! Queue presentation and unsent edits; native conversations own the queued payloads. Only acknowledged,
//! successful turns advance the queue. Editing and Stop suspend dispatch.
use super::super::icons::{self, Icon};
use eframe::egui::{self, RichText};
use std::collections::VecDeque;

const LIMIT: usize = 16;

struct Prompt {
    id: u64,
    text: String,
    attachments: Vec<rode_agent_core::attachment::Attachment>,
    files: Vec<String>,
}

struct Edit {
    id: u64,
    text: String,
}

#[derive(Default)]
pub(super) struct Queue {
    prompts: VecDeque<Prompt>,
    serial: u64,
    paused: bool,
    edit: Option<Edit>,
    waiting_for_reply: bool,
    hosted: bool,
    observed_ack: Option<String>,
    commands: VecDeque<rode_agent_core::queue::Action>,
}

enum Action {
    Edit(u64),
    Save,
    Cancel,
    Remove(u64),
    Move(u64, bool),
}

impl Queue {
    pub fn is_empty(&self) -> bool {
        self.prompts.is_empty()
    }

    pub fn clear(&mut self) {
        self.prompts.clear();
        self.commands.clear();
        self.hosted = false;
        self.observed_ack = None;
        self.edit = None;
        self.paused = false;
        self.waiting_for_reply = false;
    }

    pub fn pause(&mut self) {
        if !self.is_empty() && !self.paused {
            self.set_paused(true);
        }
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if self.hosted {
            self.commands
                .push_back(rode_agent_core::queue::Action::Pause { paused });
        }
    }
    pub fn set_hosted(&mut self) {
        self.hosted = true;
    }
    pub fn hosted(&self) -> bool {
        self.hosted
    }
    pub fn command(&self) -> Option<&rode_agent_core::queue::Action> {
        self.commands.front()
    }
    pub fn command_sent(&mut self) {
        self.commands.pop_front();
    }
    pub fn head_id(&self) -> Option<u64> {
        self.prompts.front().map(|p| p.id)
    }
    pub fn sync(&mut self, view: &rode_agent_core::queue::View, busy: bool) {
        if self.hosted
            && self.observed_ack.as_deref() == Some(view.acknowledged.as_str())
            && self.serial == view.serial
            && self.paused == view.paused
        {
            return;
        }
        self.observed_ack = Some(view.acknowledged.clone());
        let restoring = !self.hosted;
        self.hosted = true;
        self.serial = view.serial;
        self.paused = view.paused;
        if restoring || view.paused && !busy {
            self.waiting_for_reply = busy;
        }
        self.prompts = view
            .items
            .iter()
            .map(|p| Prompt {
                id: p.id,
                text: p.text.clone(),
                files: p.files.clone(),
                attachments: vec![],
            })
            .collect();
        if self.edit.as_ref().map(|e| e.id) != view.editing {
            self.edit = view.editing.and_then(|id| {
                self.prompts.iter().find(|p| p.id == id).map(|p| Edit {
                    id,
                    text: p.text.clone(),
                })
            });
        }
    }

    pub fn can_push(&self) -> bool {
        self.prompts.len() < LIMIT
    }

    #[cfg(test)]
    pub fn push(&mut self, text: String) -> anyhow::Result<()> {
        self.push_with_attachments(text, vec![])
    }
    pub fn push_with_attachments(
        &mut self,
        text: String,
        attachments: Vec<rode_agent_core::attachment::Attachment>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.can_push(), "The prompt queue is full (16 messages).");
        rode_agent_core::attachment::validate(&text, &attachments)?;
        self.serial += 1;
        if self.hosted {
            self.commands
                .push_back(rode_agent_core::queue::Action::Push {
                    id: self.serial,
                    text: text.clone(),
                    attachments: attachments.clone(),
                });
        }
        self.prompts.push_back(Prompt {
            id: self.serial,
            text,
            files: attachments.iter().map(|f| f.name().into()).collect(),
            attachments,
        });
        Ok(())
    }

    pub fn next(&self) -> Option<&str> {
        if self.paused || self.edit.is_some() || self.waiting_for_reply {
            return None;
        }
        self.prompts.front().map(|prompt| prompt.text.as_str())
    }

    pub fn next_attachments(&self) -> &[rode_agent_core::attachment::Attachment] {
        self.prompts
            .front()
            .map(|p| p.attachments.as_slice())
            .unwrap_or_default()
    }

    pub fn waiting_for_reply(&self) -> bool {
        self.waiting_for_reply
    }

    pub fn submitted(&mut self) {
        self.waiting_for_reply = true;
    }

    pub fn finished(&mut self) {
        self.waiting_for_reply = false;
    }

    /// Remove only after submission succeeds; failures keep the original text.
    pub fn sent(&mut self) {
        self.prompts.pop_front();
        if self.is_empty() {
            self.paused = false;
        }
    }

    pub fn height(&self) -> f32 {
        if self.is_empty() {
            0.
        } else {
            32. + self.prompts.len().min(3) as f32 * 30.
                + if self.edit.is_some() { 130. } else { 0. }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let mut action = None;
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Queued ({})", self.prompts.len())).strong())
                    .on_hover_text("Pending prompts belong to this agent. Persist on retains the queue and its attachments when Rode closes. Dispatch resumes when you reattach.");
                if ui.small_button(if self.paused { "Resume queue" } else { "Pause queue" })
                    .on_hover_text("Queued messages run in order after a successful reply. Stop or an error pauses the queue.")
                    .clicked() {
                    self.set_paused(!self.paused);
                }
                if self.edit.is_some() {
                    ui.weak("Editing · waiting");
                } else if self.paused {
                    ui.weak("Paused");
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("queued-prompts")
                .auto_shrink([false, false])
                .max_height(ui.available_height().max(1.))
                .show(ui, |ui| {
                    for (index, prompt) in self.prompts.iter().enumerate() {
                        ui.push_id(prompt.id, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(format!("{}.", index + 1));
                                if ui.add_enabled_ui(index > 0, |ui| icons::small(ui, Icon::ChevronUp, "Move earlier")).inner.clicked() {
                                    action = Some(Action::Move(prompt.id, true));
                                }
                                if ui.add_enabled_ui(index + 1 < self.prompts.len(), |ui| icons::small(ui, Icon::ChevronDown, "Move later")).inner.clicked() {
                                    action = Some(Action::Move(prompt.id, false));
                                }
                                if ui.add_enabled(self.edit.is_none(), egui::Button::new("Edit").small()).clicked() {
                                    action = Some(Action::Edit(prompt.id));
                                }
                                if ui.small_button("Delete").clicked() {
                                    action = Some(Action::Remove(prompt.id));
                                }
                                if !prompt.files.is_empty() { ui.label(format!("{} files", prompt.files.len())).on_hover_text(prompt.files.join("\n")); }
                                ui.add(egui::Label::new(prompt.text.lines().find(|line| !line.trim().is_empty()).unwrap_or_default()).truncate())
                                    .on_hover_text(&prompt.text);
                            });
                            if let Some(edit) = &mut self.edit
                                && edit.id == prompt.id {
                                ui.add(egui::TextEdit::multiline(&mut edit.text)
                                    .id_salt(("queued-edit", edit.id))
                                    .desired_rows(3)
                                    .desired_width(f32::INFINITY)
                                    .char_limit(crate::agent::PROMPT_LIMIT));
                                ui.horizontal(|ui| {
                                    if ui.add_enabled(valid(&edit.text) || edit.text.len() <= crate::agent::PROMPT_LIMIT && !prompt.files.is_empty(), egui::Button::new("Save prompt").small()).clicked() {
                                        action = Some(Action::Save);
                                    }
                                    if ui.small_button("Cancel edit").clicked() {
                                        action = Some(Action::Cancel);
                                    }
                                });
                            }
                        });
                    }
                });
        });
        if let Some(action) = action {
            self.apply(action);
            ui.ctx().request_repaint();
        }
    }

    fn apply(&mut self, action: Action) {
        if self.hosted {
            use rode_agent_core::queue::Action as Host;
            let command = match &action {
                Action::Edit(id) => Some(Host::Edit { id: *id }),
                Action::Save => self.edit.as_ref().map(|edit| Host::Save {
                    id: edit.id,
                    text: edit.text.clone(),
                }),
                Action::Cancel => self
                    .edit
                    .as_ref()
                    .map(|edit| Host::CancelEdit { id: edit.id }),
                Action::Remove(id) => Some(Host::Remove { id: *id }),
                Action::Move(id, earlier) => Some(Host::Move {
                    id: *id,
                    earlier: *earlier,
                }),
            };
            if let Some(command) = command {
                self.commands.push_back(command);
            }
        }
        match action {
            Action::Edit(id) => {
                if let Some(prompt) = self.prompts.iter().find(|prompt| prompt.id == id) {
                    self.edit = Some(Edit {
                        id,
                        text: prompt.text.clone(),
                    });
                }
            }
            Action::Save => {
                if let Some(edit) = self.edit.take()
                    && let Some(prompt) =
                        self.prompts.iter_mut().find(|prompt| prompt.id == edit.id)
                {
                    prompt.text = edit.text;
                }
            }
            Action::Cancel => self.edit = None,
            Action::Remove(id) => {
                self.prompts.retain(|prompt| prompt.id != id);
                if self.edit.as_ref().is_some_and(|edit| edit.id == id) {
                    self.edit = None;
                }
                if self.is_empty() {
                    self.paused = false;
                }
            }
            Action::Move(id, earlier) => {
                if let Some(index) = self.prompts.iter().position(|prompt| prompt.id == id) {
                    let other = if earlier {
                        index.saturating_sub(1)
                    } else {
                        (index + 1).min(self.prompts.len() - 1)
                    };
                    self.prompts.swap(index, other);
                }
            }
        }
    }
}

fn valid(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= crate::agent::PROMPT_LIMIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_suspends_the_queue_and_preserves_order_until_save_or_cancel() {
        let mut queue = Queue::default();
        queue.push("first".into()).unwrap();
        queue.push("second".into()).unwrap();
        queue.apply(Action::Edit(1));
        queue.edit.as_mut().unwrap().text = "changed".into();
        assert!(queue.next().is_none());
        queue.apply(Action::Cancel);
        assert_eq!(queue.next(), Some("first"));
        queue.apply(Action::Edit(1));
        queue.edit.as_mut().unwrap().text = "changed".into();
        queue.apply(Action::Move(1, false));
        queue.apply(Action::Save);
        assert_eq!(queue.next(), Some("second"));
        queue.sent();
        assert_eq!(queue.next(), Some("changed"));
        queue.submitted();
        assert!(queue.next().is_none());
        queue.finished();
        assert_eq!(queue.next(), Some("changed"));
        queue.pause();
        assert!(queue.next().is_none());
        assert_eq!(queue.prompts.front().unwrap().text, "changed");
    }

    #[test]
    fn queue_limits_and_deletion_never_discard_other_prompts() {
        let mut queue = Queue::default();
        assert!(queue.push(" \n".into()).is_err());
        assert!(queue.push("é".repeat(crate::agent::PROMPT_LIMIT)).is_err());
        for i in 0..LIMIT {
            queue.push(i.to_string()).unwrap();
        }
        assert!(queue.push("overflow".into()).is_err());
        queue.apply(Action::Edit(1));
        queue.apply(Action::Remove(1));
        assert_eq!(queue.next(), Some("1"));
        queue.clear();
        assert!(queue.is_empty());
        assert!(queue.next().is_none());
    }
    #[test]
    fn restored_queue_retains_order_pause_and_files_without_transferring_payloads() {
        let view = rode_agent_core::queue::View {
            serial: 7,
            paused: true,
            items: vec![rode_agent_core::queue::Item {
                id: 7,
                text: "later".into(),
                files: vec!["image.png".into()],
            }],
            ..Default::default()
        };
        let mut queue = Queue::default();
        queue.sync(&view, false);
        assert!(queue.next().is_none());
        assert_eq!(queue.prompts[0].files, ["image.png"]);
        assert!(queue.prompts[0].attachments.is_empty());
        queue.set_paused(false);
        assert_eq!(queue.next(), Some("later"));
        queue.push("next".into()).unwrap();
        assert_eq!(queue.prompts[1].id, 8);
    }
    #[test]
    fn an_old_idle_snapshot_does_not_release_a_submitted_turn() {
        let mut queue = Queue::default();
        queue.set_hosted();
        queue.submitted();
        queue.sync(&rode_agent_core::queue::View::default(), false);
        assert!(queue.waiting_for_reply());
        queue.sync(
            &rode_agent_core::queue::View {
                paused: true,
                ..Default::default()
            },
            false,
        );
        assert!(!queue.waiting_for_reply());
    }
}

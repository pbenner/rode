//! Queue presentation and unsent edits; native conversations own the queued payloads. Only acknowledged,
//! successful turns advance the queue. Editing and Stop suspend dispatch.
use std::collections::VecDeque;

mod view;

const LIMIT: usize = 16;

/// One stable queued prompt and its captured attachment payloads or hosted display filenames.
struct Prompt {
    id: u64,
    text: String,
    attachments: Vec<rode_agent_core::attachment::Attachment>,
    files: Vec<String>,
}

/// Unsent local text edit for a stable queue item; editing holds automatic dispatch.
struct Edit {
    id: u64,
    text: String,
}

#[derive(Default)]
/// Presentation mirror and pending edit commands for a host-owned prompt queue.
/// The direct transport path also uses local submission gating; hosted dispatch
/// remains the controller's responsibility after the GUI detaches.
pub(super) struct Queue {
    view: view::State,
    prompts: VecDeque<Prompt>,
    serial: u64,
    paused: bool,
    edit: Option<Edit>,
    waiting_for_reply: bool,
    hosted: bool,
    observed_ack: Option<String>,
    commands: VecDeque<rode_agent_core::queue::Action>,
    last_dispatched: Option<u64>,
    recall: VecDeque<String>,
}

/// Deferred queue-row interaction, applied after rendering releases its item borrow.
enum Action {
    Edit(u64),
    Save,
    Cancel,
    Remove(u64),
    Move(u64, bool),
}

impl Queue {
    /// Report whether the currently displayed pending-prompt list is empty.
    pub fn is_empty(&self) -> bool {
        self.prompts.is_empty()
    }

    /// Reset local queue presentation, pending commands, reply gating and recall state.
    pub fn clear(&mut self) {
        self.view.clear();
        self.prompts.clear();
        self.commands.clear();
        self.hosted = false;
        self.observed_ack = None;
        self.edit = None;
        self.paused = false;
        self.waiting_for_reply = false;
        self.last_dispatched = None;
        self.recall.clear();
    }

    /// Pause a nonempty queue once, recording a host command when the controller owns dispatch.
    pub fn pause(&mut self) {
        if !self.is_empty() && !self.paused {
            self.set_paused(true);
        }
    }

    /// Update the local pause indicator and enqueue the corresponding hosted control change.
    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if self.hosted {
            self.commands
                .push_back(rode_agent_core::queue::Action::Pause { paused });
        }
    }
    /// Mark this queue as mirrored host state before transferring locally queued operations.
    pub fn set_hosted(&mut self) {
        self.hosted = true;
    }
    /// Report whether queue scheduling is owned by the native controller.
    pub fn hosted(&self) -> bool {
        self.hosted
    }
    /// Borrow the oldest unsent host queue mutation without consuming it.
    pub fn command(&self) -> Option<&rode_agent_core::queue::Action> {
        self.commands.front()
    }
    /// Consume a queue mutation only after the transport has accepted it.
    pub fn command_sent(&mut self) {
        self.commands.pop_front();
    }
    /// Return the stable first pending-item ID for matching a submission acknowledgement.
    pub fn head_id(&self) -> Option<u64> {
        self.prompts.front().map(|p| p.id)
    }
    /// Reconcile an acknowledged host queue snapshot, preserving local edit state when
    /// its item still matches and adding newly dispatched prompts to local recall once.
    pub fn sync(&mut self, view: &rode_agent_core::queue::View, busy: bool) {
        if self.hosted
            && self.observed_ack.as_deref() == Some(view.acknowledged.as_str())
            && self.serial == view.serial
            && self.paused == view.paused
            && self
                .prompts
                .iter()
                .map(|p| p.id)
                .eq(view.items.iter().map(|p| p.id))
            && self.edit.as_ref().map(|edit| edit.id) == view.editing
            && self.last_dispatched == view.dispatched.last().map(|item| item.id)
        {
            return;
        }
        self.observed_ack = Some(view.acknowledged.clone());
        let restoring = !self.hosted;
        if !restoring {
            let first_new = self
                .last_dispatched
                .and_then(|id| view.dispatched.iter().position(|item| item.id == id))
                .map_or(0, |index| index + 1);
            self.recall.extend(
                view.dispatched[first_new..]
                    .iter()
                    .map(|item| item.text.clone()),
            );
        }
        self.last_dispatched = view.dispatched.last().map(|item| item.id);
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

    /// Check the 16-item local queue bound before accepting another captured prompt.
    pub fn can_push(&self) -> bool {
        self.prompts.len() < LIMIT
    }

    /// Consume accepted queued submissions exactly once for local prompt recall.
    pub fn take_dispatched(&mut self) -> Option<String> {
        self.recall.pop_front()
    }

    /// Queue a text-only prompt through the same bounded validation and host-backed action path
    /// used by prompts with attachments.
    #[cfg(test)]
    pub fn push(&mut self, text: String) -> anyhow::Result<()> {
        self.push_with_attachments(text, vec![])
    }
    /// Validate prompt/attachment budgets, allocate an ID and retain captured payloads;
    /// hosted queues additionally record a Push command for acknowledged transfer.
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

    /// Return the direct-path head text only when unpaused, unedited and not awaiting a reply.
    pub fn next(&self) -> Option<&str> {
        if self.paused || self.edit.is_some() || self.waiting_for_reply {
            return None;
        }
        self.prompts.front().map(|prompt| prompt.text.as_str())
    }

    /// Borrow the first queued prompt's captured payloads; hosted mirrors may contain filenames only.
    pub fn next_attachments(&self) -> &[rode_agent_core::attachment::Attachment] {
        self.prompts
            .front()
            .map(|p| p.attachments.as_slice())
            .unwrap_or_default()
    }

    /// Report the local reply gate used to prevent duplicate direct-path submissions.
    pub fn waiting_for_reply(&self) -> bool {
        self.waiting_for_reply
    }

    /// Hold local dispatch while an accepted prompt awaits its reply.
    pub fn submitted(&mut self) {
        self.waiting_for_reply = true;
    }

    /// Release the local reply gate after completion; pause/error policy is handled separately.
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
    fn resizing_preserves_queue_payloads_and_restores_height_after_a_compact_window() {
        use eframe::egui::{self, Key, Rect};
        for width in [340., 960.] {
            let ctx = egui::Context::default();
            crate::theme::install(
                &ctx,
                crate::theme::Theme::builtin("atelier-dark")
                    .unwrap()
                    .palette,
            );
            let mut queue = Queue::default();
            queue.set_hosted();
            for text in ["first prompt", "second prompt", "third prompt"] {
                queue.push(text.into()).unwrap();
            }
            let commands = queue.commands.len();
            let mut render = |height, visible, events| {
                let mut result = None;
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, height),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let bounds = ui.available_rect_before_wrap();
                            result = Some((
                                queue.layout(ui, bounds, visible, true),
                                ui.id().with("agent-queue-resize"),
                            ));
                        });
                    },
                );
                result.unwrap()
            };
            let ((_, original), _) = render(700., true, vec![]);
            let start = original.center_top() - egui::vec2(0., 2.);
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                pressed,
                button: egui::PointerButton::Primary,
                modifiers: egui::Modifiers::NONE,
            };
            render(
                700.,
                true,
                vec![egui::Event::PointerMoved(start), button(start, true)],
            );
            let moved = start - egui::vec2(0., 60.);
            let ((content, enlarged), id) =
                render(700., true, vec![egui::Event::PointerMoved(moved)]);
            assert!(enlarged.height() >= original.height() + 59.);
            assert!(content.bottom() < enlarged.top());
            render(700., true, vec![button(moved, false)]);
            let ((content, compact), _) = render(190., true, vec![]);
            assert!(content.height() >= 0. && compact.height() < enlarged.height());
            assert_eq!(render(700., true, vec![]).0.1, enlarged);
            assert_eq!(render(700., false, vec![]).0.1.height(), 0.);
            assert_eq!(render(700., true, vec![]).0.1, enlarged);
            ctx.memory_mut(|memory| memory.request_focus(id));
            let key = |key| egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            let taller = render(700., true, vec![key(Key::ArrowUp)]).0.1;
            assert_eq!(taller.height(), enlarged.height() + 20.);
            assert_eq!(render(700., true, vec![key(Key::Home)]).0.1, original);
            assert_eq!(
                queue.commands.len(),
                commands,
                "Resizing must not mutate the hosted queue"
            );
            assert_eq!(
                queue
                    .prompts
                    .iter()
                    .map(|p| p.text.as_str())
                    .collect::<Vec<_>>(),
                ["first prompt", "second prompt", "third prompt"]
            );
        }
    }

    #[test]
    fn accepted_prompts_enter_recall_once_in_dispatch_order_without_transcript_rows() {
        use rode_agent_core::queue::{Item, View};
        let mut queue = Queue::default();
        queue.set_hosted();
        let mut view = View::default();
        for (id, text) in [
            (2, "second, moved earlier"),
            (1, "first, edited before sending"),
        ] {
            view.dispatched.push(Item {
                id,
                text: text.into(),
                files: vec!["snapshot.txt".into()],
            });
            queue.sync(&view, false);
            assert_eq!(queue.take_dispatched().as_deref(), Some(text));
            queue.sync(&view, false);
            assert!(queue.take_dispatched().is_none());
        }
        queue.clear();
        queue.sync(&view, false);
        assert!(
            queue.take_dispatched().is_none(),
            "Reattachment must not duplicate restored history"
        );
    }

    #[test]
    fn host_dispatch_updates_rows_even_without_a_new_edit_acknowledgement() {
        use rode_agent_core::queue::{Item, View};
        let mut queue = Queue::default();
        let mut view = View {
            serial: 2,
            acknowledged: "same-edit".into(),
            items: vec![
                Item {
                    id: 1,
                    text: "first".into(),
                    files: vec![],
                },
                Item {
                    id: 2,
                    text: "second".into(),
                    files: vec![],
                },
            ],
            ..Default::default()
        };
        queue.sync(&view, true);
        view.items.remove(0);
        queue.sync(&view, true);
        assert_eq!(queue.head_id(), Some(2));
        view.items.clear();
        queue.sync(&view, false);
        assert!(queue.is_empty());
    }

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

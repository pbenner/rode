//! Window-owned completion transport with private lifecycle envelopes.
//!
//! Producers capture a typed reply sender on the UI thread. Workspace senders
//! retain that generation; profile senders belong to the window. Admission happens
//! for each dequeued message so a disconnect handled earlier in the same frame
//! immediately invalidates later workspace replies. Both senders wake the UI.
use super::*;

/// Only this module can construct transport envelopes or access the raw channel.
enum Envelope {
    Workspace {
        generation: u64,
        event: Box<WorkspaceEvent>,
    },
    Profiles(ProfileEvent),
}

/// One dequeued slot, including expired work which still consumes the frame budget.
pub(in crate::app) enum Message {
    Workspace(Box<WorkspaceEvent>),
    Profiles(ProfileEvent),
    Expired,
}

/// Owns the receiver and workspace clock; domain modules cannot mutate either.
pub(in crate::app) struct Inbox {
    generation: u64,
    tx: mpsc::Sender<Envelope>,
    rx: mpsc::Receiver<Envelope>,
}

impl Default for Inbox {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            generation: 0,
            tx,
            rx,
        }
    }
}

impl Inbox {
    /// Read-only identity for project-bound UI drafts and cached views.
    pub(in crate::app) fn generation(&self) -> u64 {
        self.generation
    }

    /// Invalidate captured workspace replies without affecting window-owned work.
    pub(in crate::app) fn invalidate_workspace(&mut self) {
        self.generation += 1;
    }

    /// Capture the current workspace generation and UI wake handle before starting a project-bound
    /// worker.
    pub(in crate::app) fn workspace(&self, ctx: &egui::Context) -> WorkspaceSender {
        WorkspaceSender {
            generation: self.generation,
            tx: self.tx.clone(),
            ctx: ctx.clone(),
        }
    }

    /// Capture a window-lifetime reply endpoint for profile writes, independent of workspace
    /// generations.
    pub(in crate::app) fn profiles(&self, ctx: &egui::Context) -> ProfileSender {
        ProfileSender {
            tx: self.tx.clone(),
            ctx: ctx.clone(),
        }
    }

    /// Admit one reply using the current generation, never a pre-admitted batch.
    pub(super) fn receive(&self) -> Option<Message> {
        self.rx.try_recv().ok().map(|envelope| self.admit(envelope))
    }

    fn admit(&self, envelope: Envelope) -> Message {
        match envelope {
            Envelope::Workspace { generation, event } if generation == self.generation => {
                Message::Workspace(event)
            }
            Envelope::Workspace { .. } => Message::Expired,
            Envelope::Profiles(event) => Message::Profiles(event),
        }
    }

    /// Test-only destructive emptiness probe: a present message is consumed. Production polling
    /// must use the admission methods instead.
    #[cfg(test)]
    pub(in crate::app) fn is_empty(&self) -> bool {
        self.rx.try_recv().is_err()
    }

    /// Wait for one test reply and admit it against the generation current at receipt; propagate
    /// channel timeout/disconnect errors.
    #[cfg(test)]
    pub(in crate::app) fn receive_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Message, mpsc::RecvTimeoutError> {
        self.rx
            .recv_timeout(timeout)
            .map(|envelope| self.admit(envelope))
    }

    /// Deliberately inject obsolete/future replies in lifecycle regression tests.
    /// Production code can only capture the current workspace sender.
    #[cfg(test)]
    pub(in crate::app) fn workspace_at(
        &self,
        ctx: &egui::Context,
        generation: u64,
    ) -> WorkspaceSender {
        WorkspaceSender {
            generation,
            tx: self.tx.clone(),
            ctx: ctx.clone(),
        }
    }
}

/// Cloneable workspace reply capability. Its private generation is fixed when captured and cannot
/// be chosen by a domain producer.
#[derive(Clone)]
pub(in crate::app) struct WorkspaceSender {
    generation: u64,
    tx: mpsc::Sender<Envelope>,
    ctx: egui::Context,
}

impl WorkspaceSender {
    /// The sender fixes the envelope lifetime and wakes the window after enqueueing.
    pub(in crate::app) fn send(&self, event: WorkspaceEvent) -> Result<(), mpsc::SendError<()>> {
        self.tx
            .send(Envelope::Workspace {
                generation: self.generation,
                event: Box::new(event),
            })
            .map_err(|_| mpsc::SendError(()))?;
        self.ctx.request_repaint();
        Ok(())
    }
}

/// Cloneable profile reply capability whose results survive project changes. The receiving profile
/// controller serializes writes and checks the submitted form identity before updating its baseline.
#[derive(Clone)]
pub(in crate::app) struct ProfileSender {
    tx: mpsc::Sender<Envelope>,
    ctx: egui::Context,
}

impl ProfileSender {
    /// Profile writes remain valid across project changes; form identity is checked by their owner.
    pub(in crate::app) fn send(&self, event: ProfileEvent) -> Result<(), mpsc::SendError<()>> {
        self.tx
            .send(Envelope::Profiles(event))
            .map_err(|_| mpsc::SendError(()))?;
        self.ctx.request_repaint();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected() -> WorkspaceEvent {
        WorkspaceEvent::Connected { result: Ok(()) }
    }
    fn profile() -> ProfileEvent {
        ProfileEvent::Changed {
            result: Ok(vec![]),
            removed: None,
        }
    }

    #[test]
    fn captured_workspace_senders_expire_while_window_senders_survive_resets() {
        let ctx = egui::Context::default();
        let mut inbox = Inbox::default();
        let old = inbox.workspace(&ctx);
        let profiles = inbox.profiles(&ctx);
        // Queued work and late sends from an old worker obey the same lifetime.
        old.send(connected()).unwrap();
        inbox.invalidate_workspace();
        old.send(connected()).unwrap();
        profiles.send(profile()).unwrap();
        inbox.workspace(&ctx).send(connected()).unwrap();
        assert!(matches!(inbox.receive(), Some(Message::Expired)));
        assert!(matches!(inbox.receive(), Some(Message::Expired)));
        assert!(matches!(inbox.receive(), Some(Message::Profiles(_))));
        assert!(matches!(inbox.receive(), Some(Message::Workspace(_))));
        assert!(inbox.receive().is_none());
    }

    #[test]
    fn each_admission_observes_invalidation_between_messages() {
        let ctx = egui::Context::default();
        let mut inbox = Inbox::default();
        let sender = inbox.workspace(&ctx);
        sender.send(connected()).unwrap();
        sender.send(connected()).unwrap();
        assert!(matches!(inbox.receive(), Some(Message::Workspace(_))));
        inbox.invalidate_workspace();
        assert!(matches!(inbox.receive(), Some(Message::Expired)));
    }

    #[test]
    fn senders_wake_the_ui_and_report_a_closed_owner_without_panicking() {
        let ctx = egui::Context::default();
        let inbox = Inbox::default();
        let workspace = inbox.workspace(&ctx);
        let profiles = inbox.profiles(&ctx);
        // Settle egui's initial repaint requests without wall-clock sleeps.
        for _ in 0..8 {
            if !ctx.has_requested_repaint() {
                break;
            }
            let _ = ctx.run(Default::default(), |_| {});
        }
        assert!(!ctx.has_requested_repaint());
        let worker = workspace.clone();
        std::thread::spawn(move || worker.send(connected()).unwrap())
            .join()
            .unwrap();
        assert!(ctx.has_requested_repaint());
        assert!(matches!(inbox.receive(), Some(Message::Workspace(_))));
        for _ in 0..8 {
            if !ctx.has_requested_repaint() {
                break;
            }
            let _ = ctx.run(Default::default(), |_| {});
        }
        assert!(!ctx.has_requested_repaint());
        let worker = profiles.clone();
        std::thread::spawn(move || worker.send(profile()).unwrap())
            .join()
            .unwrap();
        assert!(ctx.has_requested_repaint());
        assert!(matches!(inbox.receive(), Some(Message::Profiles(_))));
        drop(inbox);
        assert!(workspace.send(connected()).is_err());
        assert!(profiles.send(profile()).is_err());
    }
}

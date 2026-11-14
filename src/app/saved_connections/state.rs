//! Window-owned profile catalogue, draft identity and serialized preference writes.
//!
//! Profile writes outlive workspace changes. Reloads are discarded before writes;
//! save acknowledgements may update the catalogue but only the submitted form's
//! baseline. Joining the writer at shutdown preserves acknowledged storage work.
use super::*;

/// Own the connection catalogue, editable form identity and at most one preference writer.
/// Writes discard pending reload replies; save acknowledgements update only the matching form's
/// baseline while replacing the shared catalogue.
pub(in crate::app) struct State {
    pub(in crate::app) draft: Profile,
    pub(in crate::app) profiles: Vec<Profile>,
    pub(in crate::app) aliases: Vec<String>,
    pub(in crate::app) baseline: Option<Profile>,
    pub(super) form_id: u64,
    pub(super) name_focus: bool,
    pub(super) saving: bool,
    pub(super) reload: Option<mpsc::Receiver<Result<Vec<Profile>, String>>>,
    pub(super) writer: Option<std::thread::JoinHandle<()>>,
}

impl State {
    /// Load the saved catalogue and SSH aliases with a fresh, unsaved connection draft.
    pub(in crate::app) fn new() -> Self {
        Self {
            draft: Profile::default(),
            profiles: ssh::load_profiles(),
            aliases: ssh::aliases(),
            baseline: None,
            form_id: 0,
            name_focus: false,
            saving: false,
            reload: None,
            writer: None,
        }
    }

    /// Selecting a different form invalidates the identity of an older save reply.
    pub(in crate::app) fn select(&mut self, profile: Profile, saved: bool) {
        self.baseline = saved.then(|| profile.clone());
        self.draft = profile;
        self.form_id = self.form_id.wrapping_add(1);
    }

    /// Project preparation may canonicalize the root without replacing this form.
    pub(in crate::app) fn adopt(&mut self, profile: Profile) {
        if self.draft.label() != profile.label() {
            self.baseline = self
                .profiles
                .iter()
                .find(|saved| saved.label() == profile.label())
                .cloned();
            self.form_id = self.form_id.wrapping_add(1);
        }
        self.draft = profile;
    }

    /// Start a background catalogue reload unless a reload or write is already pending.
    /// The worker retains its vault work guard until the read finishes.
    pub(in crate::app) fn open(&mut self, vault_work: vaults::Work) {
        self.name_focus = false;
        if self.reload.is_some() || self.saving {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.reload = Some(rx);
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let _ = tx.send(ssh::load_profiles_checked().map_err(|e| format!("{e:#}")));
        });
    }

    /// Report a preference write awaiting acknowledgement, rather than whether the form is dirty.
    pub(in crate::app) fn saving(&self) -> bool {
        self.saving
    }
    /// Report whether a background catalogue reload still has a pending receiver.
    pub(in crate::app) fn reloading(&self) -> bool {
        self.reload.is_some()
    }

    /// Poll a reload without blocking; install a successful catalogue or return its failure notice.
    pub(in crate::app) fn poll_reload(&mut self) -> Option<String> {
        let receiver = self.reload.take()?;
        match receiver.try_recv() {
            Ok(Ok(profiles)) => {
                self.profiles = profiles;
                None
            }
            Ok(Err(error)) => Some(error),
            Err(mpsc::TryRecvError::Empty) => {
                self.reload = Some(receiver);
                None
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                Some("Connection profile reload failed.".into())
            }
        }
    }

    /// Capture the draft and form identity for one serialized background save.
    /// Discard any older reload so it cannot overwrite the resulting catalogue.
    pub(in crate::app) fn save(&mut self, replies: workers::ProfileSender) {
        if self.saving {
            return;
        }
        let profile = self.draft.clone();
        let previous = self.baseline.clone();
        let form = self.form_id;
        self.reload = None;
        self.saving = true;
        self.writer = Some(std::thread::spawn(move || {
            let result =
                ssh::save_profile(&profile, previous.as_ref()).map_err(|e| format!("{e:#}"));
            if let Err(error) = &result {
                eprintln!("Connection profile was not saved: {error}");
            }
            let _ = replies.send(ProfileEvent::Saved {
                result,
                submitted: Box::new(profile),
                form,
            });
        }));
    }

    /// Serialize a catalogue removal or reorder behind the same writer used by form saves.
    pub(in crate::app) fn edit(&mut self, edit: ProfileListEdit, replies: workers::ProfileSender) {
        if self.saving {
            return;
        }
        self.reload = None;
        self.saving = true;
        self.writer = Some(std::thread::spawn(move || {
            let removed = match &edit {
                ProfileListEdit::Remove(profile) => Some(profile.label()),
                ProfileListEdit::Move { .. } => None,
            };
            let result = ssh::edit_profiles(&edit).map_err(|e| format!("{e:#}"));
            if let Err(error) = &result {
                eprintln!("Saved connections were not updated: {error}");
            }
            let _ = replies.send(ProfileEvent::Changed { result, removed });
        }));
    }

    /// Acknowledge a save, join its writer and replace the catalogue.
    /// Update the form baseline only if the user still has the submitted form selected.
    pub(in crate::app) fn finish_save(
        &mut self,
        result: Result<Vec<Profile>, String>,
        submitted: Profile,
        form: u64,
    ) -> Result<&'static str, String> {
        self.join_writer();
        self.saving = false;
        let profiles = result?;
        if form == self.form_id {
            self.baseline = Some(submitted);
        }
        self.profiles = profiles;
        Ok("Connection profile saved. Passwords are never stored.")
    }

    /// Acknowledge a catalogue edit and clear the baseline if its saved entry was removed.
    pub(in crate::app) fn finish_edit(
        &mut self,
        result: Result<Vec<Profile>, String>,
        removed: Option<String>,
    ) -> Result<&'static str, String> {
        self.join_writer();
        self.saving = false;
        self.profiles = result?;
        if removed
            .as_ref()
            .is_some_and(|label| self.baseline.as_ref().is_some_and(|p| p.label() == *label))
        {
            self.baseline = None;
        }
        Ok(if removed.is_some() {
            "Saved connection removed."
        } else {
            "Saved connection order updated."
        })
    }

    /// Join the outstanding preference writer before releasing its owner.
    /// This may block during shutdown; ordinary UI progress uses reply polling.
    pub(in crate::app) fn join_writer(&mut self) {
        if let Some(worker) = self.writer.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.join_writer();
    }
}

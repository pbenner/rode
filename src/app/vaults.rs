//! Encrypted-project UI and host leases, independent of editor buffers/saves.
//!
//! Unlock stages a verified host lease before committing a project. Lock gates new work,
//! drains tracked operations and requires relevant jobs to stop; a busy, failed or
//! unreachable unmount must preserve recoverable state without claiming the mount is locked.
use super::*;
use crate::vault::{self, Client};
use std::sync::atomic::AtomicUsize;

/// RAII count of captured operations that may still use the decrypted mount;
/// dropping the last guard allows the lock sequence to proceed.
pub(super) struct Work(Arc<AtomicUsize>);

/// Requested ownership change, projected without giving the guard access to leases.
pub(super) enum Transition<'a> {
    Stay,
    Project(&'a str, Option<&'a Remote>),
    Connection,
    RecoveryConnection,
}
/// Lease-owner admission result for a proposed project/host transition. A nonempty barrier prevents
/// the adapter from changing file-I/O authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Barrier {
    None,
    Locking,
    Cleanup,
    MustLock,
    Recovery,
}
/// Vault exit policy stays with the lease owner; the adapter executes these effects.
pub(super) enum QuitPolicy {
    Close,
    ClearLostProject,
    Lock,
    RecoveryRequired,
}
/// A lease held across project-view teardown stays opaque to other components.
#[must_use = "Return retained encrypted leases to the vault owner after teardown"]
pub(super) struct RetainedLease(Option<Client>);
impl Barrier {
    /// User-facing explanation of a denied or delayed transition; an absent barrier has an empty
    /// message.
    pub fn message(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Locking => "Wait for encrypted-project locking to finish.",
            Self::Cleanup => {
                "Finish locking the encrypted project before switching projects or hosts."
            }
            Self::MustLock => "Lock the encrypted project before switching projects or hosts.",
            Self::Recovery => "Unlock the encrypted project again before reopening its files.",
        }
    }
}
impl Drop for Work {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
#[derive(Default)]
/// Staged/active verified mount leases and lock-form state. Closing leases and
/// work counters remain window-owned until normal unmount is acknowledged.
pub(super) struct State {
    active: Option<Client>,
    pending: Option<Client>,
    staged: Option<Client>,
    closing: Vec<(Client, bool)>,
    needs_lock: bool,
    locking: bool,
    lock_sent: bool,
    quit_after_lock: bool,
    lost: bool,
    released: bool,
    work: Arc<AtomicUsize>,
    settings: vault::Settings,
    password: String,
    confirmation: String,
    create: bool,
    remote: bool,
    options: sandbox::Options,
    message: String,
}
impl State {
    /// Present locking state without exposing mutation of the lease lifecycle.
    pub(super) fn locking(&self) -> bool {
        self.locking
    }
    /// Retire every project lease exactly once; captured work still delays release.
    pub(super) fn release_project(&mut self) {
        for client in [self.active.take(), self.staged.take(), self.pending.take()]
            .into_iter()
            .flatten()
        {
            self.release(client);
        }
    }
    /// Present reconnect recovery guidance without rebinding files or assuming that the encrypted
    /// mount has been recovered.
    pub(super) fn reconnected(&mut self) {
        self.message = "SSH reconnected. Unlock the encrypted project to restore file access; unsaved buffers are preserved.".into();
    }
    /// Set synthetic pending-cleanup state for lifecycle admission tests.
    #[cfg(test)]
    pub(super) fn set_cleanup_required_fixture(&mut self, required: bool) {
        self.needs_lock = required;
    }
    /// Set synthetic locking state for lifecycle admission tests.
    #[cfg(test)]
    pub(super) fn set_locking_fixture(&mut self, locking: bool) {
        self.locking = locking;
    }
    /// Project the next quit action from active, lost and cleanup lease state. Unacknowledged
    /// cleanup cannot be reported as a closed mount.
    pub(super) fn quit_policy(&self) -> QuitPolicy {
        if self.active.is_some() && self.lost && self.released {
            QuitPolicy::ClearLostProject
        } else if self.active.is_some() {
            QuitPolicy::Lock
        } else if self.pending.is_some() || self.staged.is_some() || self.cleanup_pending() {
            QuitPolicy::RecoveryRequired
        } else {
            QuitPolicy::Close
        }
    }
    /// Whether an active encrypted lease is retained, including a disconnected recovery lease.
    pub(super) fn has_active(&self) -> bool {
        self.active.is_some()
    }
    /// Whether an active lease was lost with its connection and requires explicit unlock recovery.
    pub(super) fn recovery_required(&self) -> bool {
        self.lost && self.active.is_some()
    }
    /// Move the active lease into an opaque handoff before project-view teardown. The caller must
    /// return it to this owner afterward.
    pub(super) fn retain_for_disconnect(&mut self) -> RetainedLease {
        RetainedLease(self.active.take())
    }
    /// Restore a retained lease as lost-connection recovery state without granting access through
    /// its old binding.
    pub(super) fn disconnected(&mut self, retained: RetainedLease) {
        if let Some(client) = retained.0 {
            self.active = Some(client);
            self.lost = true;
        }
    }
    /// Whether a verified, non-lost lease matches both the requested root and authenticated remote
    /// context.
    pub(super) fn can_restore_bindings(&self, root: &str, remote: &Remote) -> bool {
        !self.lost
            && self
                .active
                .as_ref()
                .is_some_and(|client| client.matches(root, Some(remote)))
    }
    /// Adopt a verified staged lease as the active project authority and clear lost/released
    /// recovery markers.
    pub(super) fn activate_project(&mut self, client: RetainedLease) {
        self.active = client.0;
        self.lost = false;
        self.released = false;
    }
    /// Evaluate lease barriers without moving resources. Recovery reconnect may preserve lost
    /// state, but project switches must honor lock and cleanup requirements.
    pub(super) fn admission(&self, transition: Transition<'_>) -> Barrier {
        if self.locking {
            return Barrier::Locking;
        }
        if matches!(transition, Transition::RecoveryConnection) {
            return if self.active.is_some() && !self.lost {
                Barrier::MustLock
            } else if self.pending.is_some() || self.staged.is_some() {
                Barrier::Cleanup
            } else {
                Barrier::None
            };
        }
        if matches!(transition, Transition::Stay) {
            return Barrier::None;
        }
        if self.cleanup_pending() {
            return Barrier::Cleanup;
        }
        if matches!(transition, Transition::Project(..))
            && self.lost
            && self.active.is_some()
            && self.staged.is_none()
        {
            return Barrier::Recovery;
        }
        let leaves = match transition {
            Transition::Stay => false,
            Transition::Connection => {
                self.active.is_some() || self.pending.is_some() || self.staged.is_some()
            }
            Transition::RecoveryConnection => unreachable!("handled above"),
            Transition::Project(path, remote) => {
                self.active
                    .as_ref()
                    .is_some_and(|client| !client.matches(path, remote))
                    && self.staged.is_none()
            }
        };
        if leaves {
            Barrier::MustLock
        } else {
            Barrier::None
        }
    }
    /// Retain decrypted-mount work ownership before spawning a background operation.
    pub(super) fn work(&self) -> Work {
        self.work.fetch_add(1, Ordering::AcqRel);
        Work(self.work.clone())
    }
    /// Retain an unfinished mount client for asynchronous release rather than forgetting its lease.
    pub(super) fn release(&mut self, client: Client) {
        if client.finished {
            return;
        }
        self.closing.push((client, false));
    }
    /// Report required lock/release work that normal exit must not abandon.
    pub(super) fn cleanup_pending(&self) -> bool {
        self.needs_lock || !self.closing.is_empty()
    }
    /// Transfer a staged or active lease only when its root and host match the project being prepared.
    pub(super) fn take_project(&mut self, root: &str, remote: Option<&Remote>) -> RetainedLease {
        let client = if self
            .staged
            .as_ref()
            .is_some_and(|client| client.matches(root, remote))
        {
            self.staged.take()
        } else if self
            .active
            .as_ref()
            .is_some_and(|client| client.matches(root, remote))
        {
            self.active.take()
        } else {
            None
        };
        RetainedLease(client)
    }
}
impl EditorApp {
    /// Display verified unlocked/unknown state without treating transport loss as a successful lock.
    pub(super) fn encrypted_banner(&mut self, ctx: &egui::Context) {
        let Some(info) = self
            .vaults
            .active
            .as_ref()
            .and_then(|client| client.info.clone())
        else {
            return;
        };
        let palette = self.themes.active().palette;
        egui::TopBottomPanel::top("encrypted-project-banner")
            .exact_height(34.)
            .frame(theme::frame(palette.raised))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        palette.accent,
                        if self.vaults.lost {
                            "ENCRYPTED PROJECT · LOCK STATE UNKNOWN"
                        } else {
                            "ENCRYPTED PROJECT · UNLOCKED"
                        },
                    );
                    chrome::bounded_label(
                        ui,
                        RichText::new(ssh::display_name(&info.cipherdir)).monospace(),
                        (ui.available_width() - 160.).clamp(40., 500.),
                        20.,
                    )
                    .on_hover_text(&info.cipherdir);
                    if ui.button("Encryption / Lock…").clicked() {
                        self.encrypted_dialog();
                    }
                });
            });
    }
    /// Cancel the current unlock/create presentation and release any unfinished staged client.
    pub(super) fn cancel_encrypted_dialog(&mut self) {
        self.vaults.password.clear();
        self.vaults.confirmation.clear();
        if let Some(client) = self.vaults.pending.take() {
            self.vaults.release(client);
        }
    }
    /// Open encrypted-project controls without implicitly creating or unlocking a filesystem.
    pub(super) fn encrypted_dialog(&mut self) {
        if !self.vaults.needs_lock {
            self.vaults.remote = self
                .vaults
                .active
                .as_ref()
                .filter(|_| self.vaults.lost)
                .map_or_else(
                    || self.workspace.as_ref().is_some_and(|w| w.remote.is_some()),
                    |client| client.remote.is_some(),
                );
        }
        self.vaults.options = self.project_picker.sandbox.clone();
        self.vaults.password.clear();
        self.vaults.confirmation.clear();
        self.set_dialog(Dialog::Encrypted);
    }
    fn open_encrypted(&mut self, lock_only: bool, ctx: &egui::Context) {
        if self.vaults.pending.is_some() || self.vaults.staged.is_some() {
            return;
        }
        if !self.vaults.closing.is_empty() || (self.vaults.needs_lock && !lock_only) {
            self.notify("Finish locking the previous encrypted mount before opening another project. Use Lock existing mount.", true);
            return;
        }
        if self.vaults.active.is_some() && !self.vaults.lost {
            self.notify(
                "Lock the current encrypted project before opening another one.",
                false,
            );
            return;
        }
        if let Err(error) = self
            .vaults
            .settings
            .validate()
            .and_then(|_| self.vaults.options.validate())
        {
            self.notify(error.to_string(), true);
            return;
        }
        if !lock_only && self.vaults.create && self.vaults.password != self.vaults.confirmation {
            self.notify("The new encrypted-project passwords do not match.", true);
            return;
        }
        let remote = if self.vaults.remote {
            let Some(remote) = self
                .workspace
                .as_ref()
                .filter(|w| w.connected)
                .and_then(|w| w.remote.clone())
            else {
                self.notify(
                    "Connect SSH before unlocking a remote encrypted project.",
                    true,
                );
                return;
            };
            Some(remote)
        } else {
            None
        };
        let ctx = ctx.clone();
        let request = vault::Open {
            settings: self.vaults.settings.clone(),
            password: std::mem::take(&mut self.vaults.password),
            create: self.vaults.create && !lock_only,
            lock_only,
        };
        self.vaults.confirmation.clear();
        match Client::start(request, remote, Arc::new(move || ctx.request_repaint())) {
            Ok(client) => {
                self.vaults.pending = Some(client);
                self.vaults.message =
                    "Contacting encrypted filesystem on the selected host…".into();
            }
            Err(error) => self.notify(format!("{error:#}"), true),
        }
    }
    /// Begin guarded locking only after owned jobs/views can stop; `quitting` requests
    /// exit after verified cleanup, while unknown or busy state keeps the window recoverable.
    pub(super) fn lock_encrypted(&mut self, quitting: bool, ctx: &egui::Context) {
        if self.vaults.active.is_none() {
            return;
        }
        if self.vaults.lost {
            self.encrypted_dialog();
            self.notify("The encrypted project's lock state is unknown. Reconnect and unlock it again before locking or exiting.", true);
            return;
        }
        if self.runtime_work_pending() || self.has_live_terminal_views() {
            self.notify("Stop terminal sessions with Sessions → End before locking this encrypted project or exiting RODE. Detached/shared jobs must be stopped too.", true);
            return;
        }
        if self.agents.chat.persistent() && (self.agents.chat.connected() || self.agents.chat.busy)
        {
            self.notify("Stop the persistent agent with Running → End before locking this encrypted project.", true);
            return;
        }
        self.stop_project_agent();
        self.leave_notebooks(workspace::LeaveReason::VaultLock);
        self.stop_debugger();
        self.stop_latex();
        self.cancel_extensions();
        for doc in &mut self.documents {
            if let Some(view) = &mut doc.marp {
                view.invalidate();
            }
            if let Some(view) = &mut doc.pdf {
                view.invalidate();
            }
            if let Some(view) = &mut doc.markdown {
                view.invalidate();
            }
        }
        self.vaults.locking = true;
        self.vaults.lock_sent = false;
        self.vaults.quit_after_lock = quitting;
        self.vaults.message = "Waiting for project operations to finish before locking…".into();
        self.set_dialog(Dialog::Encrypted);
        ctx.request_repaint();
    }
    /// Advance unlock/lease/unmount acknowledgements, waiting for tracked work and
    /// controller cleanup before sending lock; failures never publish a locked state.
    pub(super) fn poll_vaults(&mut self, ctx: &egui::Context) {
        if self.vaults.locking
            && !self.vaults.lock_sent
            && !self.debugger_stopping()
            && !self.agent_installation_pending()
            && self.vaults.work.load(Ordering::Acquire) == 0
            && let Some(client) = &self.vaults.active
        {
            match client.session.send(&serde_json::json!({"action": if self.vaults.quit_after_lock { "close" } else { "lock" }})) {
                Ok(()) => {
                    self.vaults.lock_sent = true;
                    self.vaults.message = "Locking encrypted filesystem…".into();
                }
                Err(error) => {
                    self.vaults.locking = false;
                    self.notify(error.to_string(), true);
                }
            }
        }
        let mut open = None;
        let mut closed = None;
        let mut errors = vec![];
        if let Some(client) = &mut self.vaults.pending {
            while let Some(event) = client.session.poll() {
                match event {
                    crate::process::Event::Message(value) => {
                        match serde_json::from_value::<vault::Reply>(value) {
                            Ok(vault::Reply::Opened { info }) => {
                                client.info = Some(info.clone());
                                open = Some(info);
                            }
                            Ok(vault::Reply::Closed {
                                info,
                                locked,
                                message,
                            }) => {
                                closed = Some((info, locked, message, false));
                            }
                            Ok(vault::Reply::Error { message }) => errors.push(message),
                            Err(error) => {
                                errors.push(format!("Invalid encrypted-project response: {error}"))
                            }
                        }
                    }
                    crate::process::Event::Error(error) | crate::process::Event::Stderr(error) => {
                        errors.push(error)
                    }
                    crate::process::Event::Exited(_) => {
                        client.finished = true;
                        if open.is_none() && closed.is_none() && errors.is_empty() {
                            errors.push("Encrypted-project helper exited before unlocking. Check gocryptfs and FUSE on the selected host.".into());
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(client) = &mut self.vaults.active {
            while let Some(event) = client.session.poll() {
                match event {
                    crate::process::Event::Message(value) => {
                        match serde_json::from_value::<vault::Reply>(value) {
                            Ok(vault::Reply::Closed {
                                info,
                                locked,
                                message,
                            }) => {
                                if self.vaults.locking && self.vaults.lock_sent {
                                    closed = Some((info, locked, message, true));
                                } else {
                                    self.vaults.lost = true;
                                    self.vaults.released = locked || message.starts_with("Another RODE window still has this encrypted project open.");
                                    self.workspace.suspend_project();
                                    errors.push(if locked {
                                        "The encrypted filesystem closed on its host. Unsaved buffers are preserved; unlock again to restore file access.".into()
                                    } else {
                                        format!("Encrypted-project connection ended. Buffers are preserved; the filesystem may remain unlocked: {message}")
                                    });
                                }
                            }
                            Ok(vault::Reply::Error { message }) => errors.push(message),
                            _ => {}
                        }
                    }
                    crate::process::Event::Error(error) => {
                        self.vaults.lost = true;
                        self.workspace.suspend_project();
                        errors.push(error);
                    }
                    crate::process::Event::Exited(_) => {
                        client.finished = true;
                        if closed.is_none() {
                            self.vaults.lost = true;
                            errors.push("Encrypted-project connection ended; its lock state is unknown. Reconnect and unlock explicitly before accessing files.".into());
                            self.workspace.suspend_project();
                        }
                    }
                    _ => {}
                }
            }
        }
        let can_release = self.vaults.work.load(Ordering::Acquire) == 0;
        self.vaults.closing.retain_mut(|(client, sent)| {
            if !*sent && can_release { client.release(); *sent = true; }
            while let Some(event) = client.session.poll() {
                match event {
                    crate::process::Event::Message(value) => match serde_json::from_value(value) {
                        Ok(vault::Reply::Opened { info }) => client.info = Some(info),
                        Ok(vault::Reply::Closed { info, locked: false, message }) if !message.starts_with("Another RODE window still has this encrypted project open.") => {
                                client.info = None;
                                self.vaults.needs_lock = true;
                                self.vaults.settings.cipherdir = info.cipherdir;
                                self.vaults.remote = client.remote.is_some();
                                errors.push(format!("Encrypted project remains unlocked at {}. Stop its jobs and use Lock existing mount: {message}", info.mountpoint));
                        },
                        Ok(vault::Reply::Closed { .. }) => client.info = None,
                        _ => {},
                    },
                    crate::process::Event::Exited(_) => {
                        if let Some(info) = &client.info {
                            self.vaults.needs_lock = true;
                            self.vaults.settings.cipherdir = info.cipherdir.clone();
                            self.vaults.remote = client.remote.is_some();
                            errors.push("Encrypted mount cleanup ended without confirmation. Use Lock existing mount before exiting.".into());
                        }
                        return false;
                    },
                    _ => {},
                }
            }
            true
        });
        if let Some(info) = open {
            self.vaults.create = false;
            self.vaults.staged = self.vaults.pending.take();
            self.dialog = Dialog::None;
            let keep_buffers = self.workspace.as_ref().is_some_and(|w| {
                PathBuf::from(&w.root).starts_with(&info.mountpoint)
                    && w.remote.is_some() == self.vaults.remote
            });
            if keep_buffers {
                self.prepare_project(
                    info.mountpoint,
                    self.vaults.options.clone(),
                    self.vaults.remote,
                    true,
                    None,
                    ctx,
                );
            } else {
                self.guarded(
                    Action::Project {
                        path: info.mountpoint,
                        options: self.vaults.options.clone(),
                        remote: self.vaults.remote,
                    },
                    ctx,
                );
            }
        }
        if let Some((info, locked, message, active)) = closed {
            let shared_exit = active
                && self.vaults.quit_after_lock
                && !locked
                && message
                    .starts_with("Another RODE window still has this encrypted project open.");
            self.vaults.locking = false;
            self.vaults.message = if locked {
                format!("Locked · {}", info.cipherdir)
            } else {
                format!(
                    "Still UNLOCKED at {}. Stop jobs and other views before locking: {message}",
                    info.mountpoint
                )
            };
            if active && (locked || shared_exit) {
                let quitting = self.vaults.quit_after_lock;
                self.vaults.active = None;
                self.vaults.needs_lock = false;
                self.clear_project(
                    documents::Retention::Discard,
                    workspace::LeaveReason::VaultLock,
                );
                self.workspace.clear_project_context();
                self.status = if shared_exit {
                    "Encrypted project remains open in another RODE window"
                } else {
                    "Encrypted project locked"
                }
                .into();
                self.dialog = Dialog::None;
                if quitting {
                    self.request_exit(ctx);
                }
            } else if !active {
                self.vaults.pending = None;
                self.vaults.needs_lock = !locked
                    && !message
                        .starts_with("Another RODE window still has this encrypted project open.");
            }
            if !locked && !shared_exit {
                // Unlock failure and cleanup refusal can arrive in one poll.
                // Preserve both: the cleanup reason explains why exit is blocked.
                errors.push(self.vaults.message.clone());
            }
        }
        if !errors.is_empty() {
            self.vaults.locking = false;
            self.vaults.message = errors.join("\n");
            let mut end = self.vaults.message.len().min(16 * 1024);
            while !self.vaults.message.is_char_boundary(end) {
                end -= 1;
            }
            self.vaults.message.truncate(end);
            if let Some(client) = self.vaults.pending.take() {
                self.vaults.release(client);
            }
            self.notify(self.vaults.message.clone(), true);
        }
        if !self.workspace.preparing()
            && self.pending.as_ref().is_none()
            && let Some(client) = self.vaults.staged.take()
        {
            self.vaults.release(client);
        }
        if self.vaults.locking || !self.vaults.closing.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    /// Render cached encrypted-project state and explicit create/unlock/lock operations; passwords are not saved.
    pub(super) fn encrypted_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        layout::heading(
            ui,
            "Encrypted projects · gocryptfs",
            "Files are encrypted as they are written. Only the unlocked mount exposes plaintext. Passwords are never saved.",
            self.themes.active().palette,
        );
        if !self.vaults.message.is_empty() {
            let message = ui.label(&self.vaults.message);
            if self.vaults.pending.is_none()
                && self.vaults.staged.is_none()
                && self.vaults.closing.is_empty()
            {
                super::ui_probe::button(ui, "vault-operation-complete", &message);
            }
        }
        if self.vaults.locking {
            ui.spinner();
            ui.label("Waiting for operations or the project host…");
            if !self.vaults.lock_sent && ui.button("Keep unlocked").clicked() {
                self.vaults.locking = false;
                self.vaults.quit_after_lock = false;
                self.dialog = Dialog::None;
            }
            return;
        }
        if let Some(info) = self
            .vaults
            .active
            .as_ref()
            .and_then(|client| client.info.clone())
        {
            ui.colored_label(
                self.themes.active().palette.accent,
                format!("UNLOCKED · {}", info.cipherdir),
            );
            ui.label(format!("Project view: {}", info.mountpoint));
            ui.small("Stop terminal jobs before locking. Other RODE windows must release this project first. Locking closes tabs after unsaved-change checks.");
            if ui.button("Stop project tools & lock").clicked() {
                self.guarded(Action::LockEncrypted, ctx);
            }
            if ui.button("Close").clicked() {
                self.dialog = Dialog::None;
            }
            if !self.vaults.lost {
                return;
            }
            ui.separator();
        }
        ui.add_enabled_ui(
            self.vaults.pending.is_none() && self.vaults.staged.is_none(),
            |ui| {
                ui.checkbox(&mut self.vaults.remote, "On the connected SSH host");
                for (label, value, hint) in [
                    (
                        "Encrypted storage folder",
                        &mut self.vaults.settings.cipherdir,
                        "~/projects/my-project.encrypted",
                    ),
                    (
                        "gocryptfs executable",
                        &mut self.vaults.settings.program,
                        "gocryptfs",
                    ),
                    (
                        "Unmount executable (blank: automatic)",
                        &mut self.vaults.settings.unmount_program,
                        "fusermount3",
                    ),
                ] {
                    ui.label(label);
                    let response = ui.add(
                        egui::TextEdit::singleline(value)
                            .desired_width(f32::INFINITY)
                            .hint_text(hint),
                    );
                    let name = if label == "Encrypted storage folder" {
                        "vault-folder"
                    } else if label == "gocryptfs executable" {
                        "vault-program"
                    } else {
                        "vault-unmount"
                    };
                    super::ui_probe::button(ui, name, &response);
                    super::ui_probe::value(ui, &format!("{name}-value"), &response, value);
                    if self.dialog_focus && label == "Encrypted storage folder" {
                        response.request_focus();
                        self.dialog_focus = false;
                    }
                }
                ui.checkbox(
                    &mut self.vaults.create,
                    "Create new encrypted storage (empty folder only)",
                );
                ui.label("Password (blank reuses an already-unlocked RODE mount)");
                password_field(ui, &mut self.vaults.password, "vault-password");
                if self.vaults.create {
                    ui.label("Confirm new password");
                    password_field(ui, &mut self.vaults.confirmation, "vault-confirmation");
                }
                ui.checkbox(
                    &mut self.vaults.options.enabled,
                    "Sandbox the unlocked project when available",
                );
                ui.horizontal(|ui| {
                    if ui
                        .button(if self.vaults.create {
                            "Create & unlock"
                        } else {
                            "Unlock & open"
                        })
                        .clicked()
                    {
                        self.open_encrypted(false, ctx);
                    }
                    if ui.button("Lock existing mount").clicked() {
                        self.open_encrypted(true, ctx);
                    }
                });
            },
        );
        if self.vaults.pending.is_some() || !self.vaults.closing.is_empty() {
            ui.spinner();
            if !self.vaults.closing.is_empty() {
                ui.label("Finishing encrypted-project cleanup…");
            }
        }
        if ui
            .button(if self.vaults.pending.is_some() {
                "Cancel unlock"
            } else {
                "Close"
            })
            .clicked()
        {
            self.cancel_encrypted_dialog();
            self.dialog = Dialog::None;
        }
    }
}

fn password_field(ui: &mut egui::Ui, value: &mut String, id: &str) {
    let mut output = egui::TextEdit::singleline(value)
        .id_salt(id)
        .password(true)
        .char_limit(1000)
        .desired_width(f32::INFINITY)
        .show(ui);
    output.state.clear_undoer();
    output.state.store(ui.ctx(), output.response.id);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn active(locked: bool) -> (EditorApp, egui::Context) {
        active_reply(locked, "Stop jobs first")
    }
    fn active_reply(locked: bool, message: &str) -> (EditorApp, egui::Context) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let reply = serde_json::json!({"kind":"closed", "info":{"cipherdir":"/cipher","mountpoint":"/plain"}, "locked":locked, "message":message});
        let code = format!(
            "import json,sys\nfor line in sys.stdin:\n print({},flush=True)\n",
            serde_json::to_string(&reply.to_string()).unwrap()
        );
        let code = code
            + &format!(
                " if json.loads(line).get('action')=='release' or (json.loads(line).get('action')=='close' and {}): break\n",
                if locked || message.starts_with("Another RODE window") {
                    "True"
                } else {
                    "False"
                }
            );
        let mut client = fixture_client(code);
        client.info = Some(vault::Info {
            cipherdir: "/cipher".into(),
            mountpoint: "/plain".into(),
        });
        app.vaults.active = Some(client);
        app.new_file();
        app.documents[0].set_dirty_fixture(false);
        (app, ctx)
    }
    fn fixture_client(code: String) -> Client {
        let session = crate::process::Session::start(
            crate::process::Launch {
                native_debug: false,
                argv: vec!["python3".into(), "-u".into(), "-c".into(), code],
                plan: sandbox::Plan {
                    root: "/".into(),
                    ..Default::default()
                },
                remote: None,
                python: String::new(),
                python_program: false,
                line_limit: 64 * 1024,
            },
            Arc::new(|| {}),
        )
        .unwrap();
        Client {
            session,
            info: None,
            remote: None,
            finished: false,
        }
    }
    #[test]
    fn lost_leases_cannot_reactivate_through_folder_open_or_a_late_preparation() {
        for prepared in [false, true] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path().to_string_lossy().into_owned();
            let (mut app, ctx) = active(true);
            app.vaults
                .active
                .as_mut()
                .unwrap()
                .info
                .as_mut()
                .unwrap()
                .mountpoint = root.clone();
            app.workspace.activate(
                root.clone(),
                None,
                sandbox::Plan {
                    root: root.clone(),
                    ..Default::default()
                },
                Default::default(),
            );
            let action = if prepared {
                let response = files::local(&Request::Prepare {
                    path: root.clone(),
                    sandbox: Default::default(),
                })
                .unwrap();
                let id = app.workspace.begin_preparation();
                assert!(app.workspace.finish_preparation(id));
                app.workspace.hold_prepared(
                    id,
                    workspace::Prepared {
                        response,
                        remote: false,
                        options: Default::default(),
                        open_file: None,
                        keep_buffers: true,
                        approved_documents: app.documents.versions(),
                    },
                );
                Action::PreparedProject { id }
            } else {
                Action::Project {
                    path: root.clone(),
                    remote: false,
                    options: Default::default(),
                }
            };
            app.vaults.lost = true;
            app.workspace.suspend_project();
            app.guarded(action, &ctx);
            assert!(
                !app.workspace.preparing(),
                "An unknown lease must not admit project preparation"
            );
            assert!(
                !app.workspace.as_ref().unwrap().ready,
                "A late plaintext preparation must not restore encrypted access"
            );
            assert!(app.vaults.lost);
            assert!(app.pending.is_none());
            assert!(app.notice.as_ref().unwrap().0.contains("Unlock"));
            // An explicit unlock is allowed to stage a newly verified lease.
            let (mut unlocked, _) = active(true);
            let mut lease = unlocked.vaults.active.take().unwrap();
            lease.info.as_mut().unwrap().mountpoint = root.clone();
            app.vaults.staged = Some(lease);
            assert_eq!(
                app.vaults.admission(Transition::Project(&root, None)),
                Barrier::None
            );
        }
    }
    #[test]
    fn project_release_retires_every_lease_once_and_waits_for_captured_work() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let client = || {
            let reply = serde_json::json!({"kind":"closed", "info":{"cipherdir":"/cipher","mountpoint":"/plain"}, "locked":true,"message":"Locked"});
            let mut client = fixture_client(format!(
                "import sys\nfor line in sys.stdin:\n print({},flush=True)\n break\n",
                serde_json::to_string(&reply.to_string()).unwrap()
            ));
            client.info = Some(vault::Info {
                cipherdir: "/cipher".into(),
                mountpoint: "/plain".into(),
            });
            client
        };
        app.vaults.active = Some(client());
        app.vaults.pending = Some(client());
        app.vaults.staged = Some(client());
        let work = app.vaults.work();
        app.vaults.release_project();
        app.vaults.release_project();
        assert!(
            app.vaults.active.is_none()
                && app.vaults.pending.is_none()
                && app.vaults.staged.is_none()
        );
        assert_eq!(app.vaults.closing.len(), 3);
        app.poll_vaults(&ctx);
        assert!(app.vaults.closing.iter().all(|(_, sent)| !sent));
        drop(work);
        let until = Instant::now() + Duration::from_secs(5);
        while !app.vaults.closing.is_empty() {
            assert!(
                Instant::now() < until,
                "Lease releases were not acknowledged"
            );
            app.poll_vaults(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.vaults.cleanup_pending());
    }
    #[test]
    fn staged_lease_survives_nonmodal_input_wait_and_releases_after_cancellation() {
        let project = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].acknowledge_save("source.txt".into(), "".into(), "revision".into());
        let input = |events| egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 700.),
            )),
            ..Default::default()
        };
        for _ in 0..2 {
            let _ = ctx.run(input(vec![]), |ctx| {
                app.route_input(ctx);
                app.editor(ctx);
            });
        }
        app.vaults.staged = Some(fixture_client(
            "import sys\nfor line in sys.stdin: pass\n".into(),
        ));
        let mut request = Some(Action::Project {
            path: project.path().to_string_lossy().into(),
            remote: false,
            options: Default::default(),
        });
        let _ = ctx.run(
            input(vec![egui::Event::Text("pending edit".into())]),
            |ctx| {
                app.route_input(ctx);
                if let Some(action) = request.take() {
                    app.guarded(action, ctx);
                    assert!(app.pending.as_ref().is_some());
                    assert!(!app.pending.modal());
                    app.poll_vaults(ctx);
                    assert!(
                        app.vaults.staged.is_some(),
                        "Input admission still owns the staged lease"
                    );
                    assert!(app.vaults.closing.is_empty());
                }
                app.editor(ctx);
                app.handle_close_request(ctx);
            },
        );
        assert_eq!(app.documents[0].text, "pending edit");
        assert!(app.pending.modal());
        assert!(app.vaults.staged.is_some());
        app.cancel_guarded_action();
        app.poll_vaults(&ctx);
        assert!(app.vaults.staged.is_none());
        assert_eq!(app.vaults.closing.len(), 1);
    }
    #[test]
    fn failed_unlock_preserves_cleanup_refusal_and_keeps_exit_guarded() {
        for locked in [true, false] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            let error =
                serde_json::json!({"kind":"error", "message":"No verified encrypted mount"});
            let closed = serde_json::json!({"kind":"closed", "info":{"cipherdir":"/cipher","mountpoint":"/plain"}, "locked":locked, "message":"Cannot verify other mount namespaces; project remains unlocked"});
            let code = format!(
                "print({}, flush=True)\nprint({}, flush=True)\n",
                serde_json::to_string(&error.to_string()).unwrap(),
                serde_json::to_string(&closed.to_string()).unwrap(),
            );
            let client = fixture_client(code);
            let until = Instant::now() + Duration::from_secs(5);
            // Put both replies in the same poll, as on a fast CI helper.
            while !client.session.is_finished() {
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(5));
            }
            app.vaults.pending = Some(client);
            app.poll_vaults(&ctx);
            assert!(app.vaults.message.contains("No verified encrypted mount"));
            assert_eq!(
                app.vaults
                    .message
                    .contains("Cannot verify other mount namespaces"),
                !locked
            );
            assert_eq!(app.vaults.needs_lock, !locked);
            crate::app::lifecycle::tests::native_close(&mut app, &ctx);
            assert_eq!(app.exit.closing(), locked);
        }
    }
    #[test]
    fn native_window_close_waits_for_verified_lock_even_with_clean_buffers() {
        for locked in [false, true] {
            let (mut app, ctx) = active(locked);
            let work = app.vaults.work();
            crate::app::lifecycle::tests::native_close(&mut app, &ctx);
            assert!(app.vaults.locking);
            assert!(!app.exit.closing());
            app.poll_vaults(&ctx);
            assert!(!app.vaults.lock_sent);
            drop(work);
            let until = Instant::now() + Duration::from_secs(5);
            while app.vaults.locking {
                assert!(Instant::now() < until);
                app.poll_vaults(&ctx);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(app.exit.closing(), locked);
            assert_eq!(app.vaults.active.is_none(), locked);
        }
    }
    #[test]
    fn locking_waits_for_operations_and_failed_lock_preserves_buffers_and_prevents_exit() {
        let (mut app, ctx) = active(false);
        app.documents[0].fixture_content().text = "keep text".into();
        let work = app.vaults.work();
        app.lock_encrypted(true, &ctx);
        app.poll_vaults(&ctx);
        assert!(app.vaults.locking && !app.vaults.lock_sent);
        assert!(!app.exit.closing());
        drop(work);
        let until = Instant::now() + Duration::from_secs(5);
        while app.vaults.locking {
            assert!(Instant::now() < until);
            app.poll_vaults(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.vaults.active.is_some());
        assert!(!app.exit.closing());
        assert_eq!(app.documents[0].text, "keep text");
        assert!(app.vaults.message.contains("UNLOCKED"));
    }
    #[test]
    fn a_shared_view_can_exit_and_a_lost_connection_cannot_claim_to_lock() {
        let (mut app, ctx) = active_reply(
            false,
            "Another RODE window still has this encrypted project open.",
        );
        app.lock_encrypted(true, &ctx);
        let until = Instant::now() + Duration::from_secs(5);
        while !app.exit.closing() {
            assert!(Instant::now() < until);
            app.poll_vaults(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.vaults.active.is_none());
        assert!(app.status.contains("another RODE window"));
        let (mut app, ctx) = active(true);
        app.vaults.lost = true;
        app.lock_encrypted(true, &ctx);
        assert!(!app.exit.closing() && !app.vaults.locking);
        assert!(!app.documents.is_empty());
    }
    #[test]
    fn unexpected_host_cleanup_preserves_dirty_buffers() {
        let (mut app, ctx) = active(true);
        app.documents[0].fixture_content().text = "do not discard on network loss".into();
        app.documents[0].set_dirty_fixture(true);
        app.vaults
            .active
            .as_ref()
            .unwrap()
            .session
            .send(&serde_json::json!({"action":"release"}))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while !app.vaults.lost {
            assert!(Instant::now() < until);
            app.poll_vaults(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(app.documents[0].text, "do not discard on network loss");
        assert!(app.documents[0].dirty());
        assert!(app.vaults.active.is_some());
        assert!(!app.exit.closing());
        assert!(app.vaults.released);
        app.guarded(Action::Quit, &ctx);
        assert!(matches!(app.pending.as_ref(), Some(Action::Quit)));
        app.cancel_guarded_action(); // Explicitly discard through the existing guard.
        app.perform_fixture(Action::Quit, &ctx);
        assert!(app.exit.closing());
    }
    #[test]
    fn ssh_loss_clears_workspace_but_retains_the_unknown_encrypted_lease() {
        let (mut app, ctx) = active(true);
        let mut workspace = crate::app::tests::remote_workspace();
        workspace.root = "/plain".into();
        workspace.remote.as_mut().unwrap().profile.root = "/plain".into();
        app.vaults.active.as_mut().unwrap().remote = workspace.remote.clone();
        app.workspace.set_fixture(Some(workspace));
        app.documents[0].fixture_content().path = Some("/plain/main.rs".into());
        app.documents[0].fixture_content().text = "unsaved encrypted source".into();
        app.documents[0].set_dirty_fixture(true);
        app.detach_ssh(&ctx);
        assert!(app.workspace.is_none());
        assert!(app.vaults.active.is_some());
        assert!(app.vaults.lost);
        assert!(app.documents[0].path.is_none());
        assert_eq!(app.documents[0].text, "unsaved encrypted source");
        app.encrypted_dialog();
        assert!(app.vaults.remote);
        app.lock_encrypted(true, &ctx);
        assert!(!app.exit.closing() && !app.vaults.locking);
    }
    #[test]
    fn cancelled_unlock_cleanup_waits_for_file_operations_and_exit_waits_for_cleanup() {
        let (mut app, ctx) = active(true);
        let client = app.vaults.active.take().unwrap();
        let work = app.vaults.work();
        app.vaults.release(client);
        app.poll_vaults(&ctx);
        assert_eq!(app.vaults.closing.len(), 1);
        assert!(!app.vaults.closing[0].1);
        app.perform_fixture(Action::Quit, &ctx);
        assert!(!app.exit.closing());
        drop(work);
        let until = Instant::now() + Duration::from_secs(5);
        while app.vaults.cleanup_pending() {
            assert!(Instant::now() < until);
            app.poll_vaults(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        app.perform_fixture(Action::Quit, &ctx);
        assert!(app.exit.closing());
    }
    #[test]
    fn lock_and_exit_keep_unsaved_change_protection() {
        let (mut app, ctx) = active(true);
        app.documents[0].fixture_content().text = "unsaved".into();
        app.documents[0].set_dirty_fixture(true);
        app.guarded(Action::LockEncrypted, &ctx);
        assert!(matches!(app.pending.as_ref(), Some(Action::LockEncrypted)));
        assert!(!app.vaults.locking);
        app.cancel_guarded_action();
        app.guarded(Action::Quit, &ctx);
        assert!(matches!(app.pending.as_ref(), Some(Action::Quit)));
        assert!(!app.exit.closing() && !app.vaults.locking);
    }
    #[test]
    fn successful_lock_clears_plaintext_tabs_and_dialog_cancel_clears_passwords() {
        let (mut app, ctx) = active(true);
        app.vaults.password = "secret".into();
        app.vaults.confirmation = "secret".into();
        app.cancel_encrypted_dialog();
        assert!(app.vaults.password.is_empty() && app.vaults.confirmation.is_empty());
        app.lock_encrypted(false, &ctx);
        let until = Instant::now() + Duration::from_secs(5);
        while app.vaults.active.is_some() {
            assert!(Instant::now() < until);
            app.poll_vaults(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.documents.is_empty());
        assert!(app.workspace.is_none());
        assert_eq!(app.status, "Encrypted project locked");
    }
}

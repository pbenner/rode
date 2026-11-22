//! Serial request ownership, confirmation snapshots and deadline scheduling.
//! Only queued requests advance the state machine; background work never submits
//! drafts, bypasses a pending confirmation or consumes a failed settings update.
use super::*;
use anyhow::{Context, Result, ensure};
use protocol::{Request, lifecycle_action};

const TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone)]
pub(super) struct Confirmation {
    pub action: Action,
    values: BTreeMap<String, String>,
    settings: BTreeMap<String, serde_json::Value>,
}

impl Instance {
    pub(super) fn ready(&self) -> bool {
        self.session.is_some() && self.busy.is_none() && !self.settings_dirty
    }
    pub(super) fn sync_settings(&mut self, settings: Option<&BTreeMap<String, serde_json::Value>>) {
        if settings.map_or(self.settings.is_empty(), |next| self.settings == *next) {
            return;
        }
        self.settings_dirty |= self.version >= 2;
        self.settings = settings.cloned().unwrap_or_default();
    }
    pub(super) fn send(&mut self, action: &str) -> Result<()> {
        ensure!(self.busy.is_none(), "The panel is still working");
        ensure!(
            lifecycle_action(action) || !self.settings_dirty,
            "Panel settings are still being applied"
        );
        let request = Request {
            api: self.version,
            id: self.sequence.wrapping_add(1),
            action: action.into(),
            values: if lifecycle_action(action) {
                BTreeMap::new()
            } else {
                self.values.clone()
            },
            settings: if self.version >= 2 {
                self.settings.clone()
            } else {
                BTreeMap::new()
            },
        };
        request.validate()?;
        self.session
            .as_ref()
            .context("Panel is not running")?
            .send(&serde_json::to_value(&request)?)?;
        // Serialization, validation and backpressure can all fail. Publish the
        // snapshot only after transport admission; failed sends remain retryable.
        self.sequence = request.id;
        self.sent = request.values;
        self.sent_settings = self.settings.clone();
        self.settings_dirty = false;
        self.busy = Some(Instant::now());
        if !matches!(action, "tick" | "settings") {
            self.error.clear();
        }
        Ok(())
    }
    pub(super) fn activate(&mut self, id: &str) -> Result<()> {
        ensure!(
            self.ready() && self.confirm.is_none(),
            "The panel is still working"
        );
        let action = self
            .view
            .actions
            .iter()
            .chain(self.view.action_groups.iter().flat_map(|g| &g.actions))
            .find(|a| a.id == id && a.enabled)
            .context("Panel action is no longer available")?
            .clone();
        if action.confirm.is_some() {
            self.confirm = Some(Confirmation {
                action,
                values: self.values.clone(),
                settings: self.settings.clone(),
            });
            Ok(())
        } else {
            self.send(id)
        }
    }
    fn confirmation_current(&self, pending: &Confirmation) -> bool {
        self.ready()
            && pending.values == self.values
            && pending.settings == self.settings
            && self
                .view
                .actions
                .iter()
                .chain(self.view.action_groups.iter().flat_map(|g| &g.actions))
                .any(|a| *a == pending.action && a.enabled)
    }
    pub(super) fn confirm_action(&mut self) -> Result<()> {
        let pending = self
            .confirm
            .take()
            .context("Panel changed; review the action again")?;
        ensure!(
            self.confirmation_current(&pending),
            "Panel changed; review the action again"
        );
        self.send(&pending.action.id)
    }
    pub(super) fn stop(&mut self) {
        self.session = None;
        self.busy = None;
        self.due = None;
        self.confirm = None;
    }
    /// Polling returns whether presentation changed, including unsolicited stderr
    /// and idle process exit, so detached owners repaint as well as the root.
    pub(super) fn poll(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..32 {
            let Some(event) = self.session.as_ref().and_then(|s| s.poll()) else {
                break;
            };
            changed = true;
            use crate::process::Event;
            match event {
                Event::Started => {}
                Event::Message(message) => {
                    let parsed = serde_json::from_value::<Reply>(message)
                        .map_err(anyhow::Error::from)
                        .and_then(|reply| {
                            ensure!(self.busy.is_some(), "Unexpected panel reply identity");
                            reply.validate(self.version, self.sequence)?;
                            Ok(reply)
                        });
                    match parsed {
                        Ok(reply) => {
                            for field in &reply.view.fields {
                                if self.values.get(&field.key) == self.sent.get(&field.key) {
                                    self.values.insert(field.key.clone(), field.value.clone());
                                }
                            }
                            self.values
                                .retain(|k, _| reply.view.fields.iter().any(|f| &f.key == k));
                            self.due = reply
                                .view
                                .refresh_seconds
                                .map(|s| Instant::now() + Duration::from_secs(s));
                            self.updated_settings = reply.settings;
                            self.chart_views
                                .retain(|key, _| reply.view.charts.iter().any(|c| c.key == *key));
                            self.view = reply.view;
                            self.busy = None;
                        }
                        Err(error) => {
                            self.stop();
                            self.error = format!("Invalid panel reply: {error}");
                            break;
                        }
                    }
                }
                Event::Stderr(text) => {
                    self.error = text.chars().take(2048).collect();
                }
                Event::Error(error) => {
                    self.stop();
                    self.error = error;
                    break;
                }
                Event::Exited(code) => {
                    self.stop();
                    self.error = format!("Panel process exited ({code:?}). Reopen it to restart.");
                    break;
                }
            }
        }
        if self.busy.is_some_and(|t| t.elapsed() >= TIMEOUT) {
            self.stop();
            self.error =
                "Panel request timed out. Check the action's outcome before retrying.".into();
            changed = true;
        }
        if self
            .confirm
            .as_ref()
            .is_some_and(|c| !self.confirmation_current(c))
        {
            self.confirm = None;
            self.error = "Panel changed; review the action again".into();
            changed = true;
        }
        changed
    }
    /// Run only after reply preferences have been merged. Queuing another request
    /// earlier would replace the snapshot needed to merge the completed reply.
    pub(super) fn schedule(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        // Configuration has priority over a due tick. Never refresh or apply
        // configuration behind a confirmation of another state/input snapshot.
        if self.session.is_some() && self.busy.is_none() && self.confirm.is_none() {
            let action = if self.version >= 2 && self.settings_dirty {
                Some("settings")
            } else if self.due.is_some_and(|t| Instant::now() >= t) {
                Some("tick")
            } else {
                None
            };
            if let Some(action) = action {
                if let Err(error) = self.send(action) {
                    self.stop();
                    self.error = error.to_string();
                }
                changed = true;
            }
        }
        if let Some(deadline) = self.next_deadline() {
            ctx.request_repaint_after(deadline.saturating_duration_since(Instant::now()));
        }
        changed
    }
    /// Stream events wake the root directly; an idle hidden panel needs only its
    /// actual refresh deadline, and a busy hidden panel only its timeout deadline.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        if let Some(busy) = self.busy {
            return Some(busy + TIMEOUT);
        }
        if self.confirm.is_none() && self.session.is_some() {
            self.due
        } else {
            None
        }
    }
}

//! Reusable setup library. Explicit edits use the existing preference writer;
//! no loading, saving or deletion starts a service or opens an SSH connection.
use super::*;
use crate::ssh::tunnels::{Host as SavedHost, Profiles, STORAGE_KEY, Saved};

#[derive(Default)]
pub(super) struct Library {
    pub entries: Profiles,
    pending: bool,
    pub error: Option<String>,
}
impl Library {
    fn load(storage: Option<&dyn eframe::Storage>, warnings: &mut Vec<String>) -> Self {
        let Some(text) = storage.and_then(|storage| storage.get_string(STORAGE_KEY)) else {
            return Self::default();
        };
        match ssh::tunnels::decode(&text) {
            Ok(entries) => Self {
                entries,
                ..Default::default()
            },
            Err(error) => {
                let error = format!(
                    "Saved tunnels could not be loaded; existing data is preserved: {error:#}"
                );
                warnings.push(error.clone());
                Self {
                    error: Some(error),
                    ..Default::default()
                }
            }
        }
    }

    fn save(&mut self, id: Option<&str>, profile: Saved) -> anyhow::Result<String> {
        anyhow::ensure!(
            self.error.is_none(),
            "Repair saved tunnel data before changing the library"
        );
        let id = id.map(str::to_owned).or_else(|| {
            self.entries
                .iter()
                .find(|(_, saved)| {
                    saved.host == profile.host && saved.spec.name == profile.spec.name
                })
                .map(|(id, _)| id.clone())
        });
        let id = match id {
            Some(id) => id,
            None => rode_session::new_id()?,
        };
        anyhow::ensure!(
            self.entries
                .get(&id)
                .is_none_or(|saved| saved.host == profile.host),
            "Saved tunnel belongs to a different host"
        );
        let mut next = self.entries.clone();
        next.insert(id.clone(), profile);
        ssh::tunnels::decode(&serde_json::to_string(&next)?)?;
        if self.entries != next {
            self.entries = next;
            self.pending = true;
        }
        Ok(id)
    }

    fn remove(&mut self, id: &str) {
        if self.error.is_none() && self.entries.remove(id).is_some() {
            self.pending = true;
        }
    }
}
impl Tunnels {
    pub(in crate::app) fn load(
        storage: Option<&dyn eframe::Storage>,
        warnings: &mut Vec<String>,
    ) -> Self {
        Self {
            saved: Library::load(storage, warnings),
            ..Default::default()
        }
    }

    pub(in crate::app) fn store(&mut self, storage: &mut dyn eframe::Storage) {
        if self.saved.pending {
            storage.set_string(
                STORAGE_KEY,
                serde_json::to_string(&self.saved.entries).expect("serializable tunnel settings"),
            );
            self.saved.pending = false;
        }
    }
}
impl EditorApp {
    pub(super) fn tunnel_saved_host(&self) -> SavedHost {
        SavedHost::for_profile(
            self.workspace
                .as_ref()
                .and_then(|workspace| workspace.remote.as_ref())
                .map(|remote| &remote.profile),
        )
    }

    pub(super) fn tunnel_save_label(&self, spec: &Spec, from_form: bool) -> &'static str {
        let host = self.tunnel_saved_host();
        if (from_form && self.tunnels.saved_id.is_some())
            || self
                .tunnels
                .saved
                .entries
                .values()
                .any(|saved| saved.host == host && saved.spec.name == spec.name)
        {
            "Update configuration"
        } else {
            "Save configuration"
        }
    }

    pub(super) fn save_tunnel_configuration(&mut self, spec: Spec, from_form: bool) {
        let profile = Saved {
            host: self.tunnel_saved_host(),
            spec,
            relay_jump: self.tunnels.relay_jump.trim().to_owned(),
        };
        let id = from_form
            .then_some(self.tunnels.saved_id.as_deref())
            .flatten();
        match self.tunnels.saved.save(id, profile) {
            Ok(id) => {
                if from_form {
                    self.tunnels.saved_id = Some(id);
                }
                self.tunnels.error.clear();
                self.notify("Tunnel configuration saved.", false);
            }
            Err(error) => {
                self.tunnels.error = format!("Cannot save tunnel configuration: {error:#}")
            }
        }
    }

    pub(super) fn load_tunnel_configuration(&mut self, id: &str) {
        let Some(saved) = self.tunnels.saved.entries.get(id) else {
            return;
        };
        if saved.host != self.tunnel_saved_host() {
            self.tunnels.error = format!(
                "Open {} before loading this tunnel configuration",
                saved.host.label()
            );
            return;
        }
        self.tunnels.spec = saved.spec.clone();
        self.tunnels.relay_jump = saved.relay_jump.clone();
        self.tunnels.saved_id = Some(id.into());
        self.tunnels.draft_epoch += 1;
        self.tunnels.page = Page::New;
        self.tunnels.error.clear();
    }

    pub(super) fn delete_tunnel_configuration(&mut self, id: &str) {
        self.tunnels.saved.remove(id);
        if self.tunnels.saved_id.as_deref() == Some(id) {
            self.tunnels.saved_id = None;
        }
        self.tunnels.delete_saved = None;
    }
}

//! Installed desktop panels use the shared supervised JSON transport. Lifecycle
//! is independent of SSH/project changes; enablement, provider replacement and
//! window shutdown own cancellation. Rendering never runs a provider synchronously.
use super::*;
use std::collections::BTreeMap;
mod chart;
mod protocol;
mod runtime;
#[cfg(test)]
mod tests;
mod view;
use protocol::{Action, Reply, View};

#[derive(Default)]
pub(super) struct Instance {
    pub id: String,
    provider: Option<crate::extensions::Manifest>,
    session: Option<crate::process::Session>,
    view: View,
    values: BTreeMap<String, String>,
    sent: BTreeMap<String, String>,
    sequence: u64,
    busy: Option<Instant>,
    due: Option<Instant>,
    error: String,
    settings_open: bool,
    confirm: Option<runtime::Confirmation>,
    version: u32,
    owner: u64,
    settings: BTreeMap<String, serde_json::Value>,
    sent_settings: BTreeMap<String, serde_json::Value>,
    settings_dirty: bool,
    updated_settings: Option<BTreeMap<String, serde_json::Value>>,
    chart_views: BTreeMap<String, chart::View>,
}
/// Each enabled provider owns its process, settings snapshot, draft and refresh
/// timer. Selecting a different panel never cancels another extension's work.
#[derive(Default)]
pub(super) struct State {
    pub selected: String,
    instances: BTreeMap<String, Instance>,
    next_owner: u64,
}
impl State {
    pub(super) fn tab(&self, id: &str) -> Option<ToolTab> {
        self.instances.get(id).map(|s| ToolTab::Extension(s.owner))
    }
    pub(super) fn id_for_tab(&self, tab: ToolTab) -> Option<&str> {
        let ToolTab::Extension(owner) = tab else {
            return None;
        };
        self.instances
            .values()
            .find(|s| s.owner == owner)
            .map(|s| s.id.as_str())
    }
    pub(super) fn confirming_tab(&self) -> Option<ToolTab> {
        self.instances
            .values()
            .find(|s| s.confirm.is_some())
            .map(|s| ToolTab::Extension(s.owner))
    }
    pub(super) fn confirming(&self) -> bool {
        self.instances.values().any(|s| s.confirm.is_some())
    }
    pub(super) fn remove(&mut self, id: &str) {
        self.instances.remove(id);
        if self.selected == id {
            self.selected.clear();
        }
    }
}

impl EditorApp {
    /// Installed and included executables enter through the same native host.
    pub(super) fn open_extension_panel(&mut self, id: &str, ctx: &egui::Context) {
        let registry = &self.extensions.registry;
        let Some(entry) = registry
            .entries
            .iter()
            .find(|e| e.id == id && registry.enabled(id))
        else {
            return;
        };
        let Some(manifest) = entry
            .manifest
            .as_ref()
            .filter(|m| m.panel().is_some())
            .cloned()
        else {
            return;
        };
        let included = entry.included();
        if self
            .extension_panels
            .instances
            .get(id)
            .is_none_or(|s| s.session.is_none())
        {
            let settings = registry
                .preferences
                .get(id)
                .map(|p| p.settings.clone())
                .unwrap_or_default();
            let owner = if let Some(old) = self.extension_panels.instances.get(id) {
                old.owner
            } else {
                self.extension_panels.next_owner += 1;
                self.extension_panels.next_owner
            };
            // A stopped transport does not own the user's draft or chart viewport.
            // Keep presentation state, and never submit a recovered draft as `open`.
            let mut state = self
                .extension_panels
                .instances
                .remove(id)
                .unwrap_or_default();
            state.stop();
            state.owner = owner;
            state.id = id.into();
            state.provider = Some(manifest.clone());
            state.version = manifest.panel().unwrap().protocol;
            state.settings = settings;
            state.updated_settings = None;
            let mut program = manifest.panel().unwrap().tool.program.clone();
            if included
                && !program.contains('/')
                && let Ok(exe) = std::env::current_exe()
            {
                let sibling = exe.parent().unwrap().join(&program);
                if sibling.is_file() {
                    program = sibling.to_string_lossy().into();
                }
            }
            let mut argv = vec![program];
            argv.extend(manifest.panel().unwrap().tool.args.clone());
            let wake = ctx.clone();
            let root = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(crate::extensions::directory);
            match crate::process::Session::start(
                crate::process::Launch {
                    argv,
                    plan: sandbox::Plan {
                        root: root.to_string_lossy().into(),
                        ..Default::default()
                    },
                    remote: None,
                    python: String::new(),
                    python_program: false,
                    native_debug: false,
                    line_limit: protocol::REPLY_LIMIT,
                },
                Arc::new(move || wake.request_repaint_of(egui::ViewportId::ROOT)),
            ) {
                Ok(session) => {
                    state.session = Some(session);
                    if let Err(e) = state.send("open") {
                        state.error = e.to_string();
                    }
                }
                Err(e) => state.error = e.to_string(),
            }
            self.extension_panels.instances.insert(id.into(), state);
        }
        let tab = self.extension_panels.tab(id).unwrap();
        if self.tab_detached(tab) {
            self.focus_tool_tab(tab, ctx);
        } else {
            self.extension_panels.selected = id.into();
            self.open_area(areas::Area::Extension, ctx);
        }
    }
    pub(super) fn remove_extension_panel(&mut self, id: &str) {
        if let Some(tab) = self.extension_panels.tab(id) {
            self.presentation.reattach_tab(tab);
        }
        self.extension_panels.remove(id);
    }
    /// Root-owned scheduling covers hidden and detached views equally. Replacing
    /// or disabling a provider invalidates only that provider's process/results.
    pub(super) fn poll_extension_panels(&mut self, ctx: &egui::Context) {
        let registry = &mut self.extensions.registry;
        let project_revision = registry.revision;
        let mut removed = Vec::new();
        for (id, state) in &mut self.extension_panels.instances {
            let current = registry
                .entries
                .iter()
                .find(|e| e.id == *id && registry.enabled(id))
                .and_then(|e| e.manifest.as_ref());
            if current.is_none_or(|m| {
                m.panel().map(|p| (p.tool, p.protocol))
                    != state
                        .provider
                        .as_ref()
                        .and_then(|p| p.panel())
                        .map(|p| (p.tool, p.protocol))
            }) {
                removed.push(id.clone());
                continue;
            }
            let settings = registry.preferences.get(id).map(|p| &p.settings);
            if state.provider.as_ref() != current {
                state.provider = current.cloned();
            }
            state.sync_settings(settings);
            let was_busy = state.busy.is_some();
            let mut changed = state.session.is_some() && state.poll();
            if let Some(updated) = state.updated_settings.take() {
                match update_preferences(registry, state, updated) {
                    Ok(settings) => state.settings = settings,
                    Err(error) => {
                        state.stop();
                        state.error = error.to_string();
                    }
                }
            }
            changed |= state.schedule(ctx);
            if changed || was_busy || state.busy.is_some() {
                let tab = ToolTab::Extension(state.owner);
                if self.presentation.floating_tabs.contains(&tab) {
                    ctx.request_repaint_of(tab.viewport());
                }
            }
        }
        if registry.revision != project_revision {
            self.cancel_extensions();
            self.workbench.invalidate_tools();
        }
        for id in removed {
            let selected = self.extension_panels.selected == id;
            self.remove_extension_panel(&id);
            if selected && self.presentation.area == areas::Area::Extension {
                self.presentation.enter_area(areas::Area::Projects);
            }
        }
    }
}

/// Merge only leaves changed by the provider that still equal the sent snapshot.
/// Host edits made while a request was running keep ownership of their fields.
fn merge_settings(
    before: &serde_json::Value,
    current: &mut serde_json::Value,
    after: &serde_json::Value,
) {
    fn merge(
        before: Option<&serde_json::Value>,
        current: Option<&serde_json::Value>,
        after: Option<&serde_json::Value>,
    ) -> Option<serde_json::Value> {
        if before == after {
            return current.cloned();
        }
        if current == before {
            return after.cloned();
        }
        // Missing is distinct from JSON null. Recurse through a deleted object
        // so unchanged old leaves disappear while concurrent host edits survive.
        if let Some(serde_json::Value::Object(now)) = current
            && before.is_none_or(serde_json::Value::is_object)
            && after.is_none_or(serde_json::Value::is_object)
        {
            let old = before.and_then(serde_json::Value::as_object);
            let new = after.and_then(serde_json::Value::as_object);
            let mut result = now.clone();
            let keys: std::collections::BTreeSet<_> = old
                .into_iter()
                .flat_map(|m| m.keys())
                .chain(new.into_iter().flat_map(|m| m.keys()))
                .collect();
            for key in keys {
                match merge(
                    old.and_then(|m| m.get(key)),
                    now.get(key),
                    new.and_then(|m| m.get(key)),
                ) {
                    Some(value) => {
                        result.insert(key.clone(), value);
                    }
                    None => {
                        result.remove(key);
                    }
                }
            }
            return Some(serde_json::Value::Object(result));
        }
        current.cloned()
    }
    *current =
        merge(Some(before), Some(current), Some(after)).expect("settings root remains an object");
}

/// Validate the whole preference document before publication. Per-provider
/// limits must not produce a file the host refuses to read on its next launch.
fn update_preferences(
    registry: &mut crate::extensions::Registry,
    state: &Instance,
    updated: BTreeMap<String, serde_json::Value>,
) -> anyhow::Result<BTreeMap<String, serde_json::Value>> {
    anyhow::ensure!(
        serde_json::to_vec(&updated)?.len() <= protocol::SETTINGS_LIMIT,
        "Extension settings exceed their limit"
    );
    let prefs = registry
        .preferences
        .get(&state.id)
        .map(|p| p.settings.clone())
        .unwrap_or_default();
    let mut current = serde_json::to_value(prefs)?;
    merge_settings(
        &serde_json::to_value(&state.sent_settings)?,
        &mut current,
        &serde_json::to_value(updated)?,
    );
    let merged: BTreeMap<String, serde_json::Value> = serde_json::from_value(current)?;
    registry.set_settings(&state.id, merged.clone())?;
    Ok(merged)
}

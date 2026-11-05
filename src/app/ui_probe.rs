//! Opt-in, read-only geometry for native UI fixtures. Normal startup installs no
//! hooks or file I/O. Fixtures still use real X11 input and observe real effects;
//! this reports only registered controls' geometry, focus and edit revisions,
//! never chat or document text.
use eframe::egui::{self, Id};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize)]
struct Button {
    #[serde(skip)]
    id: Id,
    name: String,
    enabled: bool,
    focused: bool,
    // Retain the edit's pass across repaints so a polling fixture cannot miss it.
    changed_pass: Option<u64>,
    rect: [f32; 4],
}

#[derive(Default)]
struct Controls {
    visible: Vec<Button>,
    last_change: HashMap<Id, u64>,
}

#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Controls>>);

/// Enable explicit fixture metadata only when a destination is supplied at launch.
pub(super) fn install(ctx: &egui::Context) {
    if let Some(path) = std::env::var_os("RODE_UI_TEST_STATE").filter(|path| !path.is_empty()) {
        install_at(ctx, PathBuf::from(path));
    }
}

fn install_at(ctx: &egui::Context, path: PathBuf) {
    let probe = Probe::default();
    ctx.data_mut(|data| data.insert_temp(Id::new("native-ui-probe"), probe.clone()));
    let begin = probe.clone();
    ctx.on_begin_pass(
        "native-ui-probe",
        Arc::new(move |_| begin.0.lock().unwrap().visible.clear()),
    );
    ctx.on_end_pass(
        "native-ui-probe",
        Arc::new(move |ctx| {
            // Discarded layout passes can place controls at temporary positions.
            if ctx.will_discard() {
                return;
            }
            let mut controls = probe.0.lock().unwrap();
            let mut buttons = controls.visible.clone();
            // Keep revisions across repaints, but discard hidden/closed widgets.
            controls
                .last_change
                .retain(|id, _| buttons.iter().any(|button| button.id == *id));
            drop(controls);
            // Resolve focus after every widget has had its turn: a later modal
            // can take focus away from an editor registered earlier in the pass.
            ctx.memory(|memory| {
                for button in &mut buttons {
                    button.focused = memory.has_focus(button.id);
                }
            });
            let value = serde_json::json!({
                "pid": std::process::id(),
                "pass": ctx.cumulative_pass_nr(),
                "pixels_per_point": ctx.pixels_per_point(),
                "size": [ctx.content_rect().width(), ctx.content_rect().height()],
                "buttons": buttons,
            });
            if let Err(error) = publish(&path, &value) {
                eprintln!(
                    "Cannot publish native UI fixture state to {}: {error}",
                    path.display()
                );
            }
        }),
    );
}

fn publish(path: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    serde_json::to_writer(file.as_file_mut(), value)?;
    file.persist(path)?;
    Ok(())
}

/// Register an existing response without changing its layout, state or input handling.
pub(super) fn button(ui: &egui::Ui, name: &str, response: &egui::Response) {
    let Some(probe) = ui
        .ctx()
        .data(|data| data.get_temp::<Probe>(Id::new("native-ui-probe")))
    else {
        return;
    };
    let rect = response.rect.intersect(ui.clip_rect());
    if rect.is_positive() {
        let changed = response.changed().then(|| ui.ctx().cumulative_pass_nr());
        let mut controls = probe.0.lock().unwrap();
        if let Some(pass) = changed {
            controls.last_change.insert(response.id, pass);
        }
        let changed_pass = controls.last_change.get(&response.id).copied();
        controls.visible.push(Button {
            id: response.id,
            name: name.into(),
            enabled: response.enabled(),
            focused: false,
            changed_pass,
            rect: [rect.left(), rect.top(), rect.right(), rect.bottom()],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_follow_enabled_visible_controls_and_clear_hidden_or_discarded_layout() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.5);
        install_at(&ctx, path.clone());
        let mut expected = None;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.add_enabled(false, egui::Button::new("Install Codex"));
                expected = Some(response.rect);
                button(ui, "Install Codex", &response);
                if ctx.current_pass_index() == 0 {
                    let transient = ui.button("Temporary layout");
                    button(ui, "Temporary layout", &transient);
                    ctx.request_discard("fixture verifies final layout only");
                }
            });
        });
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let rect = expected.unwrap();
        assert_eq!(state["buttons"].as_array().unwrap().len(), 1);
        assert_eq!(state["buttons"][0]["enabled"], false);
        assert_eq!(
            state["buttons"][0]["rect"],
            serde_json::json!([rect.left(), rect.top(), rect.right(), rect.bottom()])
        );
        assert_eq!(state["pixels_per_point"], 1.5);
        let _ = ctx.run(Default::default(), |_| {});
        let hidden: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(hidden["buttons"].as_array().unwrap().is_empty());
        assert!(hidden["pass"].as_u64().unwrap() > state["pass"].as_u64().unwrap());
    }

    #[test]
    fn focus_is_final_and_edit_revisions_survive_repaints_but_not_closed_widgets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        install_at(&ctx, path.clone());
        let id = Id::new("source");
        let mut text = String::new();
        ctx.memory_mut(|memory| memory.request_focus(id));
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Text("native input".into())],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.add(egui::TextEdit::singleline(&mut text).id(id));
                    button(ui, "source-editor", &response);
                    // Simulate a modal taking focus after source was registered.
                    let later = ui.button("Later control");
                    later.request_focus();
                    button(ui, "later", &later);
                });
            },
        );
        let edited: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(text, "native input");
        assert_eq!(edited["buttons"][0]["focused"], false);
        assert_eq!(edited["buttons"][1]["focused"], true);
        let changed = edited["buttons"][0]["changed_pass"].as_u64().unwrap();
        let render = |text: &mut String| {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.add(egui::TextEdit::singleline(text).id(id));
                    button(ui, "source-editor", &response);
                });
            });
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap()
        };
        let repainted = render(&mut text);
        assert_eq!(repainted["buttons"][0]["changed_pass"], changed);
        assert!(repainted["pass"].as_u64().unwrap() > changed);
        let _ = ctx.run(Default::default(), |_| {});
        let reopened = render(&mut text);
        assert!(reopened["buttons"][0]["changed_pass"].is_null());
    }
}

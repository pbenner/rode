//! Opt-in, read-only geometry for native UI fixtures. Normal startup installs no
//! hooks or file I/O. Fixtures still use real X11 input and observe real effects;
//! this reports only explicitly registered controls, never chat or document text.
use eframe::egui::{self, Id};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize)]
struct Button {
    name: String,
    enabled: bool,
    rect: [f32; 4],
}

#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Vec<Button>>>);

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
        Arc::new(move |_| begin.0.lock().unwrap().clear()),
    );
    ctx.on_end_pass(
        "native-ui-probe",
        Arc::new(move |ctx| {
            // Discarded layout passes can place controls at temporary positions.
            if ctx.will_discard() {
                return;
            }
            let buttons = probe.0.lock().unwrap().clone();
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
        probe.0.lock().unwrap().push(Button {
            name: name.into(),
            enabled: response.enabled(),
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
}

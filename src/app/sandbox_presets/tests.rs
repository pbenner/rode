//! Real egui interactions and persistence without starting a project or process.
use super::*;
use eframe::Storage;
use egui::{Event, FullOutput, PointerButton, Pos2, Rect};

#[derive(Default)]
struct StorageMap(HashMap<String, String>);

impl Storage for StorageMap {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn flush(&mut self) {}
}

#[derive(Default)]
struct Form {
    ctx: egui::Context,
    library: State,
    options: sandbox::Options,
    time: f64,
    scope: &'static str,
}

impl Form {
    fn frame(&mut self, events: Vec<Event>) -> FullOutput {
        self.time += 0.25;
        self.ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600., 700.))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.push_id(self.scope, |ui| {
                        self.library
                            .ui(ui, &mut self.options, theme::Palette::default())
                    });
                });
            },
        )
    }

    fn click(&mut self, label: &str) {
        let output = self.frame(vec![]);
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    let pos = text.pos + text.galley.size() / 2.;
                    if text.galley.text() == label && shape.clip_rect.contains(pos) {
                        return Some(pos);
                    }
                }
                None
            })
            .unwrap_or_else(|| panic!("No visible control: {label}"));
        for pressed in [true, false] {
            self.frame(vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
    }
}

#[test]
fn saving_and_loading_between_forms_changes_only_the_draft() {
    let mut form = Form {
        scope: "local-project",
        options: sandbox::Options {
            enabled: true,
            backend: sandbox::Backend::Landlock,
            share_network: false,
            read_only: vec!["~/.config/site-shell".into()],
            read_write: vec!["~/.cache/site-tools".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    let policy = form.options.clone();
    form.frame(vec![]);
    form.click("Reusable sandbox presets");
    form.click("Preset name");
    form.frame(vec![Event::Text("Site tools".into())]);
    form.click("Save current rules");
    assert_eq!(form.options, policy, "Saving must not change the form");
    assert_eq!(form.library.entries["Site tools"], policy);
    let mut storage = StorageMap::default();
    form.library.store(&mut storage);
    assert!(!form.library.pending);

    // A different form and fresh library exercise the same path as restarting
    // RODE and choosing the saved policy for a remote host.
    let mut warnings = vec![];
    let mut form = Form {
        scope: "ssh-project",
        library: State::load(Some(&storage), &mut warnings),
        ..Default::default()
    };
    assert!(warnings.is_empty());
    form.frame(vec![]);
    form.click("Reusable sandbox presets");
    form.click("Choose preset…");
    form.click("Site tools");
    assert_eq!(
        form.options,
        sandbox::Options::default(),
        "Selecting must not load a policy"
    );
    form.click("Load into form");
    assert_eq!(form.options, policy);
    assert!(
        !form.library.pending,
        "Loading must not edit the saved library"
    );
    form.options.share_network = true;
    form.click("Replace saved preset");
    assert!(form.library.entries["Site tools"].share_network);
    form.click("Delete preset");
    assert!(form.library.entries.is_empty());
    assert!(form.options.share_network);
    assert_eq!(form.options.read_only, policy.read_only);
    form.library.store(&mut storage);
    assert_eq!(
        storage.get_string(presets::STORAGE_KEY).as_deref(),
        Some("{}")
    );
}

#[test]
fn invalid_preset_drafts_and_malformed_storage_preserve_the_library() {
    let mut state = State::default();
    state.save("Valid", &sandbox::Options::default()).unwrap();
    let saved = state.entries.clone();
    let invalid = sandbox::Options {
        read_only: vec!["relative/path".into()],
        ..Default::default()
    };
    assert!(state.save("Valid", &invalid).is_err());
    assert_eq!(state.entries, saved);

    let mut storage = StorageMap(HashMap::from([(
        presets::STORAGE_KEY.into(),
        "{broken".into(),
    )]));
    let mut warnings = vec![];
    let mut state = State::load(Some(&storage), &mut warnings);
    assert_eq!(warnings.len(), 1);
    assert!(state.load_error.is_some());
    state.store(&mut storage);
    assert_eq!(
        storage.get_string(presets::STORAGE_KEY).as_deref(),
        Some("{broken")
    );
}

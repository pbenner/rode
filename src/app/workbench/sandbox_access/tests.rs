//! Interaction regressions for editing grants without moving another path under the cursor.
use super::*;
use egui::{Context, Event, FullOutput, PointerButton, Pos2, Rect};

struct Form {
    ctx: Context,
    options: sandbox::Options,
    time: f64,
}

impl Form {
    fn new() -> Self {
        let mut form = Self {
            ctx: Context::default(),
            options: sandbox::Options {
                enabled: true,
                read_only: vec!["/data/alpha".into(), "/data/bravo".into()],
                read_write: vec!["/data/output".into()],
                ..Default::default()
            },
            time: 0.,
        };
        form.frame(vec![]);
        form
    }

    fn frame(&mut self, events: Vec<Event>) -> FullOutput {
        self.time += 0.25;
        self.ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600., 1800.))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    rules_ui(ui, &mut self.options, Palette::default())
                });
            },
        )
    }

    fn click(&mut self, pos: Pos2) {
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

    fn focus_path(&mut self, path: &str) -> egui::Id {
        let output = self.frame(vec![]);
        self.click(labels(&output, path)[0]);
        self.ctx.memory(|memory| memory.focused()).unwrap()
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        for pressed in [true, false] {
            self.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            }]);
        }
    }

    fn replace_path(&mut self, value: &str) {
        self.key(egui::Key::A, egui::Modifiers::COMMAND);
        self.frame(vec![Event::Text(value.into())]);
        self.frame(vec![]);
    }
}

fn labels(output: &FullOutput, label: &str) -> Vec<Pos2> {
    output
        .shapes
        .iter()
        .filter_map(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                let center = text.pos + text.galley.size() / 2.;
                if text.galley.text() == label && shape.clip_rect.contains(center) {
                    return Some(center);
                }
            }
            None
        })
        .collect()
}

#[test]
fn changing_access_keeps_the_grant_in_its_row() {
    let mut form = Form::new();
    let output = form.frame(vec![]);
    let alpha = labels(&output, "/data/alpha")[0];
    let access = labels(&output, "Read only")
        .into_iter()
        .find(|pos| (pos.y - alpha.y).abs() < 4.)
        .unwrap();
    form.click(access);
    let output = form.frame(vec![]);
    // The other writable grant is below the popup; choose its first matching label.
    let write = labels(&output, "Read + write")
        .into_iter()
        .filter(|pos| pos.y > alpha.y)
        .min_by(|a, b| a.y.total_cmp(&b.y))
        .unwrap();
    form.click(write);
    assert_eq!(form.options.read_only, ["/data/bravo"]);
    assert_eq!(form.options.read_write, ["/data/alpha", "/data/output"]);
    let output = form.frame(vec![]);
    assert_eq!(
        labels(&output, "/data/alpha")[0].y,
        alpha.y,
        "Changing access must not move a different grant under the pointer"
    );
    assert!(labels(&output, "/data/bravo")[0].y > alpha.y);
}

#[test]
fn removing_a_grant_preserves_the_other_paths_edit_history() {
    let mut form = Form::new();
    form.focus_path("/data/alpha");
    form.replace_path("/data/alpha-edited");
    let bravo = form.focus_path("/data/bravo");
    form.replace_path("/data/bravo-edited");
    form.focus_path("/data/alpha-edited");
    // From the path, tab through its access picker to its remove button.
    form.key(egui::Key::Tab, egui::Modifiers::NONE);
    form.key(egui::Key::Tab, egui::Modifiers::NONE);
    form.key(egui::Key::Space, egui::Modifiers::NONE);
    assert_eq!(form.options.read_only, ["/data/bravo-edited"]);
    assert_eq!(form.focus_path("/data/bravo-edited"), bravo);
    form.key(egui::Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(form.options.read_only, ["/data/bravo"]);
    assert_eq!(form.options.read_write, ["/data/output"]);
}

#[test]
fn replacing_a_policy_discards_old_paths_and_undo_history() {
    let mut form = Form::new();
    let old_id = form.focus_path("/data/alpha");
    form.replace_path("/data/alpha-edited");
    form.options.read_only = vec!["/data/replacement".into()];
    form.options.read_write.clear();
    assert_ne!(form.focus_path("/data/replacement"), old_id);
    form.key(egui::Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(form.options.read_only, ["/data/replacement"]);
    assert!(form.options.read_write.is_empty());
    form.options.read_only.clear();
    form.frame(vec![]);
    assert!(form.options.read_only.is_empty());
    assert!(form.options.read_write.is_empty());
}

#[test]
fn adding_a_grant_appends_an_editable_row_after_writable_paths() {
    let mut form = Form::new();
    let output = form.frame(vec![]);
    let previous_last = labels(&output, "/data/output")[0].y;
    form.click(labels(&output, "+ Add folder or file")[0]);
    let output = form.frame(vec![]);
    let new_path = labels(&output, "/path or ~/folder")[0];
    assert!(new_path.y > previous_last);
    form.click(new_path);
    form.replace_path("~/new-grant");
    assert_eq!(
        form.options.read_only,
        ["/data/alpha", "/data/bravo", "~/new-grant"]
    );
    assert_eq!(form.options.read_write, ["/data/output"]);
}

#[test]
fn folder_and_ssh_dialogs_do_not_share_rule_editing_state() {
    use crate::app::{Dialog, EditorApp, Startup};
    let ctx = Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let options = sandbox::Options {
        enabled: true,
        read_only: vec!["/data/shared".into()],
        ..Default::default()
    };
    app.folder_sandbox = options.clone();
    app.profile.sandbox = options;
    ctx.memory_mut(|memory| memory.set_everything_is_visible(true));
    for dialog in [Dialog::Folder, Dialog::Connect] {
        app.dialog = dialog;
        app.dialog_focus = false;
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1000., 1800.))),
                    ..Default::default()
                },
                |ctx| app.dialogs(ctx),
            );
        }
    }
    assert_eq!(
        ctx.data(|data| data.count::<ExtraPaths>()),
        2,
        "Folder and SSH forms must have independent rule order and edit history"
    );
}

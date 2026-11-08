//! Named native controls must target the intended row action after layout changes.
use super::*;

#[test]
fn history_row_controls_keep_delete_separate_from_restore_at_compact_widths() {
    for width in [320., 960.] {
        for scale in [1., 1.5] {
            for (availability, target, expected) in [
                (
                    Availability::Saved,
                    "agent-history-delete:thread",
                    (false, true),
                ),
                (
                    Availability::Saved,
                    "agent-history-restore:thread",
                    (true, false),
                ),
                (
                    Availability::Active,
                    "agent-history-delete:thread",
                    (false, false),
                ),
                (
                    Availability::Current,
                    "agent-history-delete:thread",
                    (false, false),
                ),
            ] {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("widgets.json");
                let ctx = egui::Context::default();
                ctx.set_pixels_per_point(scale);
                ui_probe::install_at(&ctx, path.clone());
                let entry = Entry {
                    id: "thread".into(),
                    title: "Private long title\nwith multiple lines ".repeat(20),
                    preview: "Private preview ".repeat(40),
                    updated: "5m ago".into(),
                    active: availability == Availability::Active,
                };
                let frame = |events| {
                    let mut action = (false, false);
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 500.),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                ui.add_space(67.); // The row need not start at a fixed screen offset.
                                action = conversation(
                                    ui,
                                    &entry,
                                    true,
                                    availability == Availability::Saved,
                                    availability,
                                    true,
                                    Palette::default(),
                                );
                            });
                        },
                    );
                    let snapshot = std::fs::read_to_string(&path).unwrap();
                    assert!(!snapshot.contains("Private"));
                    (
                        action,
                        serde_json::from_str::<serde_json::Value>(&snapshot).unwrap(),
                    )
                };
                frame(vec![]);
                let (_, state) = frame(vec![]);
                let button = state["buttons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|button| button["name"] == target)
                    .unwrap();
                assert_eq!(button["enabled"], availability == Availability::Saved);
                let rect: [f32; 4] = serde_json::from_value(button["rect"].clone()).unwrap();
                assert!(rect[2] <= width && rect[3] <= 500.);
                let point = egui::pos2((rect[0] + rect[2]) / 2., (rect[1] + rect[3]) / 2.);
                frame(vec![egui::Event::PointerMoved(point)]);
                let mut actual = (false, false);
                for pressed in [true, false] {
                    let (action, _) = frame(vec![egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }]);
                    actual.0 |= action.0;
                    actual.1 |= action.1;
                }
                assert_eq!(actual, expected, "{target}, width={width}, scale={scale}");
            }
        }
    }
}

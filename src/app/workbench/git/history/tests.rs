//! Rendered history rows: bounds, action alignment, filtering and immutable targets.
use super::*;
use crate::app::{EditorApp, Startup};

fn fixture(count: usize) -> String {
    (1..=count).map(|index| format!("{index:040x}\0{index:07x}\0{}\02026-11-06\0{}\n",
        if index == 2 { "Very long author name with Unicode ユニコード and more text" } else { "Fixture author" },
        if index == 2 { "A long commit subject with quoted paths 'src/a file.rs' and many more details to truncate cleanly across compact panes" } else { "Short commit" }
    )).collect()
}

fn frame(
    ctx: &egui::Context,
    state: &mut State,
    data: &str,
    palette: Palette,
    width: f32,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<(String, Action)>) {
    let mut selected = None;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 700.),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let available = ui.available_rect_before_wrap();
                let body = ui
                    .scope(|ui| selected = show(ui, state, data, palette))
                    .response
                    .rect;
                assert!(
                    body.right() <= available.right() + 1.,
                    "History overflow: {body:?} vs {available:?}"
                );
            });
        },
    );
    (output, selected)
}

fn details(output: &egui::FullOutput) -> Vec<egui::Pos2> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Details" => {
                let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                assert!(
                    shape.clip_rect.contains_rect(rect),
                    "Details clipped: {rect:?} in {:?}",
                    shape.clip_rect
                );
                Some(rect.center())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn history_actions_align_and_rows_stay_uniform_with_long_metadata_and_compact_themes() {
    for theme in [
        "phosphor",
        "studio",
        "studio-neutral",
        "studio-hacker",
        "telemetry",
    ] {
        for width in [360., 680., 1100.] {
            for scale in [0.75, 1., 1.5] {
                let ctx = egui::Context::default();
                let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                let mut app = EditorApp::new(&cc, Startup::default());
                app.themes.select(theme, &ctx).unwrap();
                ctx.set_zoom_factor(scale);
                let mut state = State::default();
                let data = fixture(3);
                let palette = app.themes.active().palette;
                frame(&ctx, &mut state, &data, palette, width, vec![]);
                let (output, _) = frame(&ctx, &mut state, &data, palette, width, vec![]);
                let buttons = details(&output);
                assert_eq!(buttons.len(), 3, "{theme}, width={width}, scale={scale}");
                assert!(
                    buttons
                        .windows(2)
                        .all(|pair| (pair[0].x - pair[1].x).abs() < 1.),
                    "{theme}, width={width}, scale={scale}: {buttons:?}"
                );
                assert!(((buttons[1].y - buttons[0].y) - (buttons[2].y - buttons[1].y)).abs() < 1.);
                layout::tests::assert_button_alignment(
                    &output,
                    palette,
                    ctx.pixels_per_point(),
                    "Git history",
                );
            }
        }
    }
}

#[test]
fn filtered_history_clicks_keep_the_actual_commit_identity_and_large_lists_are_virtualized() {
    let ctx = egui::Context::default();
    let mut state = State {
        filter: "very long author".into(),
        ..Default::default()
    };
    let data = fixture(100);
    let palette = Palette::default();
    frame(&ctx, &mut state, &data, palette, 680., vec![]);
    let (output, _) = frame(&ctx, &mut state, &data, palette, 680., vec![]);
    let target = details(&output);
    assert_eq!(target.len(), 1);
    let mut chosen = None;
    for pressed in [true, false] {
        chosen = frame(
            &ctx,
            &mut state,
            &data,
            palette,
            680.,
            vec![
                egui::Event::PointerMoved(target[0]),
                egui::Event::PointerButton {
                    pos: target[0],
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .1
        .or(chosen);
    }
    assert_eq!(chosen, Some((format!("{:040x}", 2), Action::Details)));
    state.filter.clear();
    let (output, _) = frame(&ctx, &mut state, &data, palette, 680., vec![]);
    // Only viewport rows are rendered; a partly visible final row may be clipped vertically.
    let count = output.shapes.iter().filter(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Details")).count();
    assert!(count > 1 && count < 20, "Rendered {count} of 100 rows");
}

#[test]
fn empty_history_and_malformed_records_are_distinct_from_unrelated_git_errors() {
    assert!(Commit::parse("bad\0data").is_none());
    assert!(Commit::parse(&format!("{}\0hash\0author\0date\0subject", "x".repeat(40))).is_none());
    assert!(empty_error(&crate::project::Output {
        code: 128,
        stderr: "fatal: your current branch 'main' does not have any commits yet".into(),
        ..Default::default()
    }));
    assert!(!empty_error(&crate::project::Output {
        code: 128,
        stderr: "fatal: not a git repository".into(),
        ..Default::default()
    }));
}

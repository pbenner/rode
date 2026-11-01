use super::*;
use egui::{Context, Event, FullOutput, PointerButton, Pos2, Rect};

fn frame(
    ctx: &Context,
    files: &[GitFile],
    width: f32,
    events: Vec<Event>,
) -> (FullOutput, Option<(usize, Action)>, Rect) {
    let mut action = None;
    let mut rect = Rect::NOTHING;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 500.))),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let result = ui.scope(|ui| show(ui, files, theme::Palette::default()));
                action = result.inner;
                rect = result.response.rect;
            });
        },
    );
    (output, action, rect)
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

fn click(ctx: &Context, files: &[GitFile], width: f32, pos: Pos2) -> Option<(usize, Action)> {
    let mut result = None;
    for pressed in [true, false] {
        result = frame(
            ctx,
            files,
            width,
            vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .1
        .or(result);
    }
    result
}

fn fixture() -> Vec<GitFile> {
    (0..100)
        .map(|index| {
            let (staged, worktree) = match index / 25 {
                0 => ('?', '?'),
                1 => ('M', ' '),
                2 => ('U', 'U'),
                _ => (' ', 'M'),
            };
            GitFile {
                index: staged,
                worktree,
                path: if index < 25 {
                    format!("file-{index:03}.rs")
                } else {
                    format!(
                        "file-{index:03}/a very long directory/ユニコード/{}\nline.rs",
                        "nested/".repeat(30)
                    )
                },
                original: None,
            }
        })
        .collect()
}

#[test]
fn scrolling_mixed_statuses_keeps_width_and_action_columns() {
    let files = fixture();
    for width in [320., 540., 1000.] {
        let ctx = Context::default();
        theme::install(&ctx, theme::Palette::default());
        frame(&ctx, &files, width, vec![]);
        let (mut output, _, first_rect) = frame(&ctx, &files, width, vec![]);
        let diff_x = labels(&output, "Diff")[0].x;
        let stage_x = labels(&output, "Stage")[0].x;
        let last_label = if labels(&output, "More").is_empty() {
            "Discard"
        } else {
            "More"
        };
        let last_x = labels(&output, last_label)[0].x;
        assert!(
            (first_rect.width() - (width - 16.)).abs() < 1.,
            "List must fill the pane: {first_rect:?}"
        );
        let mut saw_conflict = false;
        for step in 0..4 {
            let diffs = labels(&output, "Diff");
            let stages: Vec<_> = labels(&output, "Stage")
                .into_iter()
                .chain(labels(&output, "Mark resolved"))
                .collect();
            let last = labels(&output, last_label);
            assert!(
                diffs.len() >= 5,
                "Rows grew or stopped being virtualized at {width}, step {step}"
            );
            assert_eq!(diffs.len(), stages.len());
            assert_eq!(diffs.len(), last.len());
            for (points, expected) in [(&diffs, diff_x), (&stages, stage_x), (&last, last_x)] {
                assert!(
                    points.iter().all(|p| (p.x - expected).abs() < 1.),
                    "Columns moved at {width}, step {step}: {points:?}"
                );
            }
            assert!(
                diffs
                    .iter()
                    .all(|diff| stages.iter().any(|stage| (diff.y - stage.y).abs() < 1.))
            );
            saw_conflict |= !labels(&output, "Mark resolved").is_empty();
            if step == 3 {
                break;
            }
            frame(
                &ctx,
                &files,
                width,
                vec![
                    Event::PointerMoved(egui::pos2(width / 2., 150.)),
                    Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0., -700.),
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            for _ in 0..30 {
                let (next, _, rect) = frame(&ctx, &files, width, vec![]);
                assert!((rect.width() - first_rect.width()).abs() < 1.);
                assert!(rect.right() <= width - 7.);
                output = next;
            }
        }
        assert!(
            saw_conflict,
            "The scroll did not reach a different action set at {width}"
        );
    }
}

#[test]
fn compact_menu_keeps_actions_bound_to_their_file() {
    let files = vec![fixture()[0].clone(), fixture()[25].clone()];
    let ctx = Context::default();
    theme::install(&ctx, theme::Palette::default());
    let width = 360.;
    frame(&ctx, &files, width, vec![]);
    let (output, _, _) = frame(&ctx, &files, width, vec![]);
    assert_eq!(
        click(&ctx, &files, width, labels(&output, "Stage")[0]),
        Some((0, Action::Stage))
    );
    let more = labels(&output, "More");
    click(&ctx, &files, width, more[1]);
    let (output, _, _) = frame(&ctx, &files, width, vec![]);
    assert_eq!(
        click(&ctx, &files, width, labels(&output, "Unstage")[0]),
        Some((1, Action::Unstage))
    );
    click(&ctx, &files, width, more[0]);
    let (output, _, _) = frame(&ctx, &files, width, vec![]);
    assert_eq!(
        click(&ctx, &files, width, labels(&output, "Discard")[0]),
        None,
        "Untracked files must not offer discard"
    );
}

//! Mixed live-agent metadata must not change card geometry or action ownership.
use super::*;
use crate::{agent::Provider, theme};
use egui::{Event, FullOutput, PointerButton, Pos2, Rect};

fn fixture() -> Vec<Info> {
    (0..4)
        .map(|index| Info {
            supports_attachments: true,
            supports_queue: true,
            automatic_queue: true,
            id: format!("agent-{index}"),
            name: if index == 1 {
                "A long agent title with Unicode 🦀 and many details ".repeat(5)
            } else {
                format!("Agent {index}")
            },
            root: if index == 1 {
                format!("/different/{}/project", "long directory/".repeat(15))
            } else {
                "/project".into()
            },
            policy: if index == 3 {
                "different".into()
            } else {
                "policy".into()
            },
            mode: if index == 3 {
                "SANDBOX · read only".into()
            } else {
                "NORMAL MODE".into()
            },
            provider: [
                Provider::Codex,
                Provider::Acp,
                Provider::Local,
                Provider::Codex,
            ][index],
            alive: true,
            busy: index == 1,
            status: if index == 2 {
                "A detailed status with queued tasks\nand more information ".repeat(8)
            } else {
                "Ready".into()
            },
            attached: index == 0 || index == 3,
            persistent: index != 1,
            created: 0,
        })
        .collect()
}

fn context() -> Context<'static> {
    Context {
        current: Some("agent-0"),
        root: "/project",
        policy: "policy",
        mode: "NORMAL MODE",
        can_leave: true,
    }
}

fn frame(
    ctx: &egui::Context,
    items: &[Info],
    width: f32,
    events: Vec<Event>,
) -> (FullOutput, Option<(String, Action)>) {
    let mut selected = None;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 1100.))),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                selected = show(ui, items, &context(), Palette::default());
            });
        },
    );
    (output, selected)
}

fn labels(output: &FullOutput, label: &str) -> Vec<Pos2> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.pos + text.galley.size() / 2.)
            }
            _ => None,
        })
        .collect()
}

fn click(ctx: &egui::Context, items: &[Info], width: f32, point: Pos2) -> Option<(String, Action)> {
    let mut selected = None;
    for pressed in [true, false] {
        selected = frame(
            ctx,
            items,
            width,
            vec![
                Event::PointerMoved(point),
                Event::PointerButton {
                    pos: point,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .1
        .or(selected);
    }
    selected
}

#[test]
fn cards_and_actions_align_with_short_long_and_incompatible_metadata() {
    let items = fixture();
    for width in [320., 390., 680., 1000.] {
        let ctx = egui::Context::default();
        theme::install(&ctx, Palette::default());
        frame(&ctx, &items, width, vec![]);
        let (output, _) = frame(&ctx, &items, width, vec![]);
        let palette = Palette::default();
        let cards: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if (rect.fill == palette.panel || rect.fill == palette.active)
                        && rect.rect.width() > width * 0.8 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            cards.len(),
            items.len(),
            "Missing cards at width {width}: {cards:?}"
        );
        for card in &cards {
            assert!((card.width() - cards[0].width()).abs() < 0.5);
            assert!(
                (card.height() - cards[0].height()).abs() < 0.5,
                "Unequal heights at {width}: {cards:?}"
            );
            assert!((card.left() - cards[0].left()).abs() < 0.5);
            assert!(card.right() <= width - 7.);
        }
        let ends = labels(&output, "End");
        assert_eq!(ends.len(), items.len());
        assert!(ends.iter().all(|pos| (pos.x - ends[0].x).abs() < 0.5));
        let attaches = labels(&output, "Attach");
        let show = labels(&output, "Show chat")[0];
        assert!(attaches.iter().all(|pos| (pos.x - show.x).abs() < 0.5));
        let details: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text)
                    if items.iter().any(|info| {
                        text.galley.text() == single_line(&info.root)
                            || text.galley.text() == single_line(&info.mode)
                            || text.galley.text() == single_line(&info.status)
                    }) =>
                {
                    Some(text.pos.x)
                }
                _ => None,
            })
            .collect();
        assert_eq!(details.len(), items.len() * 3);
        assert!(
            details.iter().all(|x| (x - details[0]).abs() < 0.5),
            "Metadata columns are misaligned at {width}: {details:?}"
        );
        assert_eq!(
            click(&ctx, &items, width, show),
            Some(("agent-0".into(), Action::Show))
        );
        assert_eq!(
            click(&ctx, &items, width, attaches[0]),
            None,
            "Different-project agent allowed attachment"
        );
        assert_eq!(
            click(&ctx, &items, width, attaches[1]),
            Some(("agent-2".into(), Action::Attach))
        );
        assert_eq!(
            click(&ctx, &items, width, ends[3]),
            Some(("agent-3".into(), Action::End))
        );
    }
}

#[test]
fn blocked_attachments_keep_their_full_explanations() {
    let mut context = context();
    context.can_leave = false;
    let items = fixture();
    let current = context.attachment(&items[0]);
    assert!(current.enabled && current.current);
    let blocked = context.attachment(&items[3]);
    assert!(!blocked.enabled);
    assert!(blocked.explanation.contains("Different sandbox mode"));
    assert!(
        blocked
            .explanation
            .contains("Controlled by another RODE window")
    );
    assert!(blocked.explanation.contains("Persist off"));
    assert!(!context.attachment(&items[2]).enabled);
}

use super::*;
use std::collections::BTreeMap;

struct Browser {
    ctx: egui::Context,
    entries: Vec<Entry>,
    list: navigation::List,
    checked: HashSet<String>,
    view: View,
    width: f32,
}
impl Browser {
    fn new(view: View, count: usize) -> Self {
        Self {
            ctx: egui::Context::default(),
            entries: (0..count)
                .map(|n| Entry {
                    path: format!("/files/{n}.txt"),
                    name: format!("File {n} with a long Unicode name 日本語.txt"),
                    dir: n == 0,
                })
                .collect(),
            list: navigation::List::default(),
            checked: HashSet::new(),
            view,
            width: 430.,
        }
    }

    fn frame(
        &mut self,
        events: Vec<egui::Event>,
    ) -> (
        egui::FullOutput,
        Option<Choice>,
        BTreeMap<String, egui::Rect>,
    ) {
        let ctx = self.ctx.clone();
        let mut choice = None;
        let mut rects = BTreeMap::new();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(self.width, 500.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ctx.memory_mut(|m| m.request_focus(ui.make_persistent_id(("items", "focus"))));
                    let mut hook = |_: &mut egui::Ui, entry: &Entry, response: &egui::Response| {
                        rects.insert(entry.path.clone(), response.rect);
                    };
                    choice = rows_with_options(
                        ui,
                        "items",
                        &self.entries,
                        false,
                        &mut self.list,
                        "",
                        (0, false),
                        None,
                        true,
                        RowOptions {
                            height: Some(300.),
                            checked: Some(&self.checked),
                            icons: true,
                            view: self.view,
                            on_row: Some(&mut hook),
                        },
                    );
                });
            },
        );
        (output, choice, rects)
    }

    fn key(&mut self, key: Key) -> (Option<Choice>, BTreeMap<String, egui::Rect>) {
        let (_, choice, rects) = self.frame(
            [true, false]
                .into_iter()
                .map(|pressed| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
                .collect(),
        );
        (choice, rects)
    }
}

#[test]
fn clearing_checked_files_removes_the_fill_even_at_the_navigation_cursor() {
    for theme in theme::Theme::builtins() {
        for view in [View::List, View::Icons] {
            let mut browser = Browser::new(view, 2);
            theme::install(&browser.ctx, theme.palette);
            browser.list.navigated = true;
            browser.checked.insert(browser.entries[0].path.clone());
            browser.frame(vec![]);
            let (checked, _, rects) = browser.frame(vec![]);
            let rect = rects[&browser.entries[0].path];
            let selected_fill = selection_fill(&browser.ctx.style().visuals);
            let painted_selection = |out: &egui::FullOutput| {
                out.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(r) if r.fill == selected_fill && r.rect.intersects(rect))
            })
            };
            assert!(painted_selection(&checked), "No selection fill in {view:?}");
            let accent = browser
                .ctx
                .style()
                .visuals
                .selection
                .stroke
                .color
                .to_opaque();
            let marked = |out: &egui::FullOutput| {
                out.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(r) if r.fill == accent && r.rect.width() < 20. && r.rect.intersects(rect))
            })
            };
            assert!(marked(&checked), "No explicit selection mark in {view:?}");
            browser.checked.clear();
            let (cleared, _, _) = browser.frame(vec![]);
            assert!(
                !painted_selection(&cleared),
                "Navigation left a selected fill in {view:?}"
            );
            assert!(!marked(&cleared), "Cleared file still has a checked mark");
            assert!(browser.list.navigated);
            assert!(cleared.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(r) if r.stroke.color == theme.palette.accent && r.rect.intersects(rect))
            }), "Clearing selection lost the keyboard cursor");
        }
    }
}

#[test]
fn selection_marks_stay_clear_of_item_outlines_at_different_scales() {
    for theme in theme::Theme::builtins() {
        for view in [View::List, View::Icons] {
            for (pixels_per_point, font_scale) in [(1., 1.), (1.25, 1.5), (2., 2.)] {
                for width in [180., 430.] {
                    let mut browser = Browser::new(view, 2);
                    browser.width = width;
                    theme::install(&browser.ctx, theme.palette);
                    browser.ctx.set_pixels_per_point(pixels_per_point);
                    browser.ctx.style_mut(|style| {
                        // Embedded panes can remove ordinary button padding.
                        // File items must still reserve their own border gutter.
                        style.spacing.button_padding = egui::Vec2::ZERO;
                        for font in style.text_styles.values_mut() {
                            font.size *= font_scale;
                        }
                    });
                    browser.list.navigated = true;
                    browser
                        .checked
                        .extend(browser.entries.iter().map(|e| e.path.clone()));
                    browser.frame(vec![]);
                    let (output, _, rects) = browser.frame(vec![]);
                    let accent = browser
                        .ctx
                        .style()
                        .visuals
                        .selection
                        .stroke
                        .color
                        .to_opaque();
                    for entry in &browser.entries {
                        let row = rects[&entry.path];
                        let border = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Rect(r)
                                    if r.stroke.width == 2.
                                        && r.stroke.color == accent
                                        && r.rect.contains(row.center()) =>
                                {
                                    Some(r)
                                }
                                _ => None,
                            })
                            .expect("Missing selected item outline");
                        let mark = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Rect(r)
                                    if r.fill == accent
                                        && r.rect.width() < 20.
                                        && row.contains_rect(r.rect) =>
                                {
                                    Some(r.rect)
                                }
                                _ => None,
                            })
                            .expect("Missing checked selection mark");
                        assert!(
                            border
                                .rect
                                .shrink(border.stroke.width + 3.)
                                .contains_rect(mark),
                            "Checkbox touches the item border: theme={}, view={view:?}, dpi={pixels_per_point}, font={font_scale}, width={width}, row={row:?}, mark={mark:?}",
                            theme.id
                        );
                    }
                    let first = rects[&browser.entries[0].path];
                    let second = rects[&browser.entries[1].path];
                    assert!(
                        !first.intersects(second),
                        "File items overlap after scaling"
                    );
                }
            }
        }
    }
}

#[test]
fn tile_navigation_tracks_columns_and_reveals_the_same_file_after_resizing() {
    let mut browser = Browser::new(View::Icons, 100);
    browser.frame(vec![]);
    let (_, _, rects) = browser.frame(vec![]);
    let first = rects[&browser.entries[0].path];
    let columns = rects
        .values()
        .filter(|rect| (rect.top() - first.top()).abs() < 1.)
        .count();
    assert!(columns > 1);
    browser.key(Key::ArrowRight);
    assert_eq!(browser.list.index, 1);
    browser.key(Key::ArrowDown);
    assert_eq!(browser.list.index, columns + 1);
    browser.key(Key::ArrowLeft);
    assert_eq!(browser.list.index, columns);
    assert!(
        matches!(browser.key(Key::Space).0, Some(Choice::Select(e)) if e.path == browser.entries[columns].path)
    );
    assert!(
        matches!(browser.key(Key::Enter).0, Some(Choice::Open(e)) if e.path == browser.entries[columns].path)
    );
    let (_, rects) = browser.key(Key::End);
    assert_eq!(browser.list.index, 99);
    assert!(rects.contains_key(&browser.entries[99].path));
    browser.width = 180.;
    let (_, _, rects) = browser.frame(vec![]);
    assert_eq!(browser.list.index, 99);
    assert!(rects.contains_key(&browser.entries[99].path));
    assert!(
        rects
            .values()
            .all(|rect| rect.left() >= 0. && rect.right() <= browser.width)
    );
    browser.view = View::List;
    let (_, _, rects) = browser.frame(vec![]);
    assert_eq!(browser.list.index, 99);
    assert!(rects.contains_key(&browser.entries[99].path));
    assert!(matches!(
        browser.key(Key::ArrowLeft).0,
        Some(Choice::Parent)
    ));
}

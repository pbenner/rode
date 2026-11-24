//! Exercise actual table interaction and persisted geometry with native egui input.
use super::*;

struct Fixture {
    ctx: egui::Context,
    directory: tempfile::TempDir,
    table: protocol::Table,
    frame: u64,
    width: f32,
    index: usize,
}

impl Fixture {
    fn new() -> Self {
        let ctx = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        ui_probe::install_at(&ctx, directory.path().join("table.json"));
        Self {
            ctx,
            directory,
            table: protocol::Table {
                key: "budgets".into(),
                title: "Budget overview".into(),
                columns: vec!["Project".into(), "Total".into()],
                rows: (0..512)
                    .map(|_| {
                        vec![
                            "Research funding for a long project description".into(),
                            "123.45".into(),
                        ]
                    })
                    .collect(),
            },
            frame: 0,
            width: 1000.,
            index: 0,
        }
    }

    fn render(&mut self, events: Vec<egui::Event>) -> serde_json::Value {
        self.frame += 1;
        let _ = self.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(self.width, 600.),
                )),
                time: Some(self.frame as f64 * 0.1),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.push_id(self.frame, |ui| {
                        let count = render_table(ui, ("test", self.index), &self.table);
                        assert!(count > 0 && count < 40, "Laid out {count} rows");
                    });
                });
            },
        );
        serde_json::from_slice(&std::fs::read(self.directory.path().join("table.json")).unwrap())
            .unwrap()
    }

    fn key(&mut self, key: Key) {
        self.render(vec![
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }

    fn pointer(&mut self, pos: egui::Pos2, pressed: bool) {
        self.render(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }

    fn geometry(&mut self, column: usize, grip: bool) -> egui::Rect {
        let kind = if grip { "column-resize" } else { "column" };
        let name = format!("extension-table-{kind}:test:{}:{column}", self.index);
        self.control_rect(&name)
    }

    fn control_rect(&mut self, name: &str) -> egui::Rect {
        let state = self.render(vec![]);
        let rect = &state["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == name)
            .unwrap_or_else(|| panic!("Missing {name}: {state}"))["rect"];
        egui::Rect::from_min_max(
            egui::pos2(
                rect[0].as_f64().unwrap() as f32,
                rect[1].as_f64().unwrap() as f32,
            ),
            egui::pos2(
                rect[2].as_f64().unwrap() as f32,
                rect[3].as_f64().unwrap() as f32,
            ),
        )
    }
}

#[test]
fn nested_tables_keep_their_height_after_content_near_the_bottom_of_the_page() {
    let f = Fixture::new();
    let mut allocated = 0.;
    for frame in 0..2 {
        let _ = f.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 600.),
                )),
                time: Some(frame as f64 * 0.1),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add_space(500.);
                        let top = ui.next_widget_position().y;
                        render_table(ui, ("test", 0), &f.table);
                        allocated = ui.next_widget_position().y - top;
                    });
                });
            },
        );
    }
    assert!(
        allocated > 390.,
        "Table collapsed to remaining page height: {allocated}"
    );
}

#[test]
fn table_height_adapts_to_the_window_resizes_without_compounding_and_resets() {
    let mut f = Fixture::new();
    f.render(vec![]);
    let table = "extension-table:test:0";
    let initial = f.control_rect(table).height();
    assert!(
        initial > 320. && initial < 500.,
        "Default table height: {initial}"
    );
    let start = f
        .control_rect("extension-table-height-resize:test:0")
        .center();
    f.pointer(start, true);
    f.render(vec![egui::Event::PointerMoved(start + egui::vec2(0., 30.))]);
    f.render(vec![egui::Event::PointerMoved(start + egui::vec2(0., 60.))]);
    f.pointer(start + egui::vec2(0., 60.), false);
    let resized = f.control_rect(table).height();
    assert!(
        (resized - initial - 60.).abs() < 2.,
        "{initial} -> {resized}"
    );
    f.key(Key::ArrowDown);
    assert!((f.control_rect(table).height() - resized - 20.).abs() < 2.);
    f.key(Key::ArrowUp);
    assert!((f.control_rect(table).height() - resized).abs() < 2.);
    f.key(Key::Home);
    assert!((f.control_rect(table).height() - initial).abs() < 2.);
}

#[test]
fn table_height_survives_reordering_and_persistence_without_affecting_other_tables() {
    let mut f = Fixture::new();
    let id = egui::Id::new((
        "extension-table-height",
        ("test", "key:budgets".to_string()),
    ));
    f.ctx.data_mut(|data| data.insert_persisted(id, 450_f32));
    f.index = 4;
    f.table.title = "Updated budget overview".into();
    assert!((f.control_rect("extension-table:test:4").height() - 458.).abs() < 2.);
    let memory = f.ctx.memory(|memory| ron::to_string(memory).unwrap());
    let ctx = egui::Context::default();
    ctx.memory_mut(|m| *m = ron::from_str(&memory).unwrap());
    ui_probe::install_at(&ctx, f.directory.path().join("table.json"));
    f.ctx = ctx;
    assert!((f.control_rect("extension-table:test:4").height() - 458.).abs() < 2.);
    f.table.key = "another-table".into();
    assert!(f.control_rect("extension-table:test:4").height() < 420.);
}

#[test]
fn content_widths_resize_by_mouse_and_keyboard_and_reset_to_automatic() {
    let mut f = Fixture::new();
    f.render(vec![]);
    let initial = f.geometry(0, false).width();
    assert!(initial > f.geometry(1, false).width() + 100.);
    let start = f.geometry(0, true).center();
    f.pointer(start, true);
    f.render(vec![egui::Event::PointerMoved(start + egui::vec2(80., 0.))]);
    f.render(vec![egui::Event::PointerMoved(
        start + egui::vec2(100., 0.),
    )]);
    f.pointer(start + egui::vec2(100., 0.), false);
    let resized = f.geometry(0, false).width();
    assert!(
        (resized - initial - 100.).abs() < 1.,
        "{initial} -> {resized}"
    );
    for _ in 0..3 {
        assert!((f.geometry(0, false).width() - resized).abs() < 1.);
    }
    f.key(Key::ArrowLeft);
    assert!((f.geometry(0, false).width() - resized + 20.).abs() < 1.);
    f.key(Key::Home);
    assert!((f.geometry(0, false).width() - initial).abs() < 1.);
}

#[test]
fn column_preferences_survive_replies_reordering_and_persistence_without_aliasing() {
    let mut f = Fixture::new();
    f.render(vec![]);
    let id = column_id("test", "key:budgets", &f.table.columns);
    f.ctx
        .data_mut(|data| data.insert_persisted(id, vec![Some(300_f32), None]));
    f.table.title = "8 of 8 rows".into();
    f.table.rows[0][0] = "Changed content".into();
    f.index = 4;
    f.width = 600.;
    assert!((f.geometry(0, false).width() - 300.).abs() < 1.);
    let memory = f.ctx.memory(|memory| ron::to_string(memory).unwrap());
    let ctx = egui::Context::default();
    ctx.memory_mut(|m| *m = ron::from_str(&memory).unwrap());
    ui_probe::install_at(&ctx, f.directory.path().join("table.json"));
    f.ctx = ctx;
    assert!((f.geometry(0, false).width() - 300.).abs() < 1.);
    assert_ne!(id, column_id("other", "key:budgets", &f.table.columns));
    assert_ne!(id, column_id("test", "key:other", &f.table.columns));
    f.table.columns[0] = "Different schema".into();
    assert_ne!(id, column_id("test", "key:budgets", &f.table.columns));
    assert!((f.geometry(0, false).width() - 300.).abs() > 10.);
}

#[test]
fn large_fonts_and_multiline_cells_keep_fixed_rows_and_separate_resize_grips() {
    let mut f = Fixture::new();
    f.width = 480.;
    f.ctx.style_mut(|style| {
        style.override_font_id = Some(egui::FontId::proportional(30.));
        style.spacing.interact_size.y = 50.;
    });
    f.table.rows[0][0] = "Multiple\nlines\r\nwith Unicode 日本語".into();
    f.render(vec![]);
    let header = f.geometry(0, false);
    let grip = f.geometry(0, true);
    assert!(header.contains_rect(grip));
    assert!(grip.width() >= 8.);
    assert!(header.height() >= 30.);
    assert!(grip.right() <= f.width);
}

#[test]
fn wide_columns_scroll_horizontally_to_reach_the_last_divider() {
    let mut f = Fixture::new();
    f.width = 480.;
    let id = column_id("test", "key:budgets", &f.table.columns);
    f.ctx.data_mut(|data| {
        data.insert_persisted(id, vec![Some(800_f32), Some(300_f32)]);
    });
    f.render(vec![]);
    let focus = egui::Id::new(("extension-table", "test", "budgets"))
        .with(("extension-table-focus", ("test", "key:budgets".to_string())));
    f.ctx.memory_mut(|memory| memory.request_focus(focus));
    // Let the focused widget install its arrow-key lock before sending input.
    f.render(vec![]);
    for _ in 0..20 {
        f.key(Key::ArrowRight);
    }
    let last = f.geometry(1, false);
    assert!((last.width() - 300.).abs() < 1.);
    let grip = f.geometry(1, true);
    assert!(last.contains_rect(grip));
    assert!(grip.right() <= f.width);
    assert!(grip.width() >= 8.);
}

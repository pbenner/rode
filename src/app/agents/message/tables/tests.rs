//! Column geometry, viewport containment and native copying through the shared message buffer.
use super::*;

fn frame(
    ctx: &egui::Context,
    width: f32,
    source: &str,
    events: Vec<egui::Event>,
) -> (
    egui::FullOutput,
    egui::text_edit::TextEditOutput,
    super::super::MessageLayout,
) {
    scaled_frame(ctx, width, source, events, 1.)
}

fn scaled_frame(
    ctx: &egui::Context,
    width: f32,
    source: &str,
    events: Vec<egui::Event>,
    scale: f32,
) -> (
    egui::FullOutput,
    egui::text_edit::TextEditOutput,
    super::super::MessageLayout,
) {
    let mut output = None;
    let mut parsed = None;
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, 900.),
        )),
        events,
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(scale);
    let full = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            parsed = Some(super::super::layout(ui, source, theme::Palette::default()));
            output = Some(crate::app::agents::render_message(
                ui,
                source,
                theme::Palette::default(),
                false,
            ));
            assert!(
                ui.min_rect().right() <= width,
                "table expanded its parent past the viewport: {} > {width} at scale {scale}",
                ui.min_rect().right()
            );
        });
    });
    (full, output.unwrap(), parsed.unwrap())
}

fn edges(galley: &egui::Galley, text: &str, cell: &Range<usize>) -> (f32, f32) {
    let first = text[..cell.start].chars().count();
    let last = text[..cell.end].chars().count() - 1;
    let position = |index| galley.layout_from_cursor(egui::text::CCursor::new(index));
    let start = position(first);
    let end = position(last);
    let row = &galley.rows[start.row];
    assert_eq!(
        start.row, end.row,
        "a table cell wrapped independently of its row"
    );
    (
        row.pos.x + row.glyphs[start.column].pos.x,
        row.pos.x + row.glyphs[end.column].max_x(),
    )
}

#[test]
fn columns_align_styled_unicode_cells_and_only_prose_wraps_in_narrow_panes() {
    let ctx = egui::Context::default();
    let source = "Before **bold** prose that should wrap in a narrow chat pane.\n\n| Name | Center | Amount |\n|:---|:---:|---:|\n| WWWWW | *β* | 1 |\n| i | `Ω` | 12345 |\n| empty | | |\n\nAfter prose that should also wrap at the visible edge.\n\n```text\n| literal | code |\n```";
    let expected = "Before bold prose that should wrap in a narrow chat pane.\n\nName | Center | Amount\nWWWWW | β | 1\ni | Ω | 12345\nempty |  | \n\nAfter prose that should also wrap at the visible edge.\n\n| literal | code |";
    for (scale, width) in [1., 1.5, 2.]
        .into_iter()
        .flat_map(|scale| [150., 260., 640., 1100.].map(move |width| (scale, width)))
    {
        let (_, output, layout) = scaled_frame(&ctx, width, source, vec![], scale);
        assert_eq!(layout.tables.len(), 1);
        assert_eq!(layout.copy(0..usize::MAX), expected);
        assert_eq!(output.galley.text(), expected);
        assert_eq!(
            output
                .galley
                .rows
                .iter()
                .map(|row| row.char_count_including_newline())
                .sum::<usize>(),
            expected.chars().count()
        );
        let table = &layout.tables[0];
        let first = output
            .galley
            .layout_from_cursor(egui::text::CCursor::new(table.rows[0].character))
            .row;
        for (index, row) in table.rows.iter().enumerate() {
            let at = output
                .galley
                .layout_from_cursor(egui::text::CCursor::new(row.character));
            assert_eq!(at.row, first + index);
            for (column, cell) in row
                .cells
                .iter()
                .enumerate()
                .filter(|(_, cell)| !cell.is_empty())
            {
                let (left, right) = edges(&output.galley, &layout.job.text, cell);
                assert!(left >= table.starts[column] - 1.);
                assert!(
                    right <= table.starts[column] + table.widths[column] + 1.,
                    "width={width}, row={index}, column={column}, cell={:?}, edges={left}..{right}, slot={}..{}",
                    &layout.job.text[cell.clone()],
                    table.starts[column],
                    table.starts[column] + table.widths[column]
                );
                let actual = match column {
                    0 => left,
                    1 => (left + right) / 2.,
                    _ => right,
                };
                let expected = match column {
                    0 => table.starts[column],
                    1 => table.starts[column] + table.widths[column] / 2.,
                    _ => table.starts[column] + table.widths[column],
                };
                assert!(
                    (actual - expected).abs() <= 1.,
                    "column {column} is not aligned: {actual} != {expected}"
                );
            }
        }
        for (index, row) in output.galley.rows.iter().enumerate() {
            if !(first..first + table.rows.len()).contains(&index) {
                assert!(
                    row.rect().right() <= width - 16. + 1.,
                    "prose escaped its viewport"
                );
            }
        }
        for rows in output.galley.rows.windows(2) {
            assert!(
                rows[1].rect().top() + 0.01 >= rows[0].rect().bottom(),
                "rows overlap"
            );
        }
    }
}

#[test]
fn multiple_tables_escaped_pipes_empty_cells_and_math_keep_copy_indices() {
    let ctx = egui::Context::default();
    let source = "Intro\n\n| A | B |\n|---|---|\n| escaped \\| pipe | \\(\\unsupported{x}\\) |\n| | last |\n\nBetween\n\n| C | D |\n|---|---|\n| **one** | [Docs](https://example.invalid) |\n\nEnd";
    let (_, output, layout) = frame(&ctx, 210., source, vec![]);
    assert_eq!(layout.tables.len(), 2);
    assert_eq!(
        layout.copy(0..usize::MAX),
        "Intro\n\nA | B\nescaped | pipe | \\(\\unsupported{x}\\)\n | last\n\nBetween\n\nC | D\none | Docs (https://example.invalid)\n\nEnd"
    );
    let mut previous = None;
    for table in &layout.tables {
        let at = output
            .galley
            .layout_from_cursor(egui::text::CCursor::new(table.rows[0].character));
        if let Some(last) = previous {
            assert!(at.row > last);
        }
        previous = Some(at.row + table.rows.len() - 1);
    }
    let (_, _, empty) = frame(&ctx, 180., "| |\n|---|\n| |", vec![]);
    assert_eq!(empty.tables.len(), 1);
}

#[test]
fn selection_copies_across_tables_and_survives_streamed_rows() {
    let ctx = egui::Context::default();
    let source = "Before\n\n| Name | Value |\n|---|---:|\n| α | 42 |\n\nAfter";
    let (_, mut partial, _) = frame(&ctx, 220., "Before\n\n| Name | Value |", vec![]);
    partial
        .state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(6),
        )));
    partial.state.store(&ctx, partial.response.id);
    partial.response.request_focus();
    let (full, completed, _) = frame(&ctx, 220., source, vec![egui::Event::Copy]);
    assert_eq!(
        completed.response.id, partial.response.id,
        "recognizing a streamed table replaced the text widget"
    );
    assert!(
        full.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "Before")
        )
    );
    let (_, mut output, layout) = frame(&ctx, 220., source, vec![]);
    let expected = layout.copy(2..layout.job.text.chars().count() - 2);
    output
        .state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(2),
            egui::text::CCursor::new(layout.job.text.chars().count() - 2),
        )));
    output.state.store(&ctx, output.response.id);
    output.response.request_focus();
    let (full, _, _) = frame(&ctx, 220., source, vec![egui::Event::Copy]);
    assert!(full.platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &expected)
    ));
    let (_, mut output, layout) = frame(&ctx, 220., source, vec![]);
    let start = layout.job.text[..layout.job.text.find('α').unwrap()]
        .chars()
        .count();
    output
        .state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(start),
            egui::text::CCursor::new(start + 1),
        )));
    output.state.store(&ctx, output.response.id);
    output.response.request_focus();
    let streamed = source.replace(
        "| α | 42 |",
        "| α | 42 |\n| a much wider new row | 123456789 |",
    );
    let (full, next, _) = frame(&ctx, 170., &streamed, vec![egui::Event::Copy]);
    assert_eq!(next.response.id, output.response.id);
    assert_eq!(
        next.cursor_range.unwrap().as_sorted_char_range(),
        start..start + 1
    );
    assert!(
        full.platform_output
            .commands
            .iter()
            .any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "α"))
    );
}

#[test]
fn horizontal_scroll_reveals_columns_while_prose_stays_at_the_viewport_edge() {
    let ctx = egui::Context::default();
    let source = "Before the table\n\n| First long column | Second long column | Last long column |\n|---|---|---|\n| Alpha WWWWWWWWWWW | Beta WWWWWWWWWWW | Omega WWWWWWWWWWW |\n\nAfter the table";
    frame(&ctx, 230., source, vec![]);
    let (_, before, layout) = frame(&ctx, 230., source, vec![]);
    assert!(before.galley.size().x > 230.);
    let prose_x = before.galley_pos.x
        + before
            .galley
            .pos_from_cursor(egui::text::CCursor::new(0))
            .left();
    let table_row = before
        .galley
        .layout_from_cursor(egui::text::CCursor::new(layout.tables[0].rows[0].character))
        .row;
    let pointer = egui::pos2(
        120.,
        before.galley_pos.y + before.galley.rows[table_row].rect().center().y,
    );
    let (_, mut after, _) = frame(
        &ctx,
        230.,
        source,
        vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-10000., 0.),
                modifiers: Default::default(),
            },
        ],
    );
    for _ in 0..20 {
        after = frame(&ctx, 230., source, vec![]).1;
    }
    assert!(
        after.galley_pos.x < before.galley_pos.x - 100.,
        "table did not scroll horizontally"
    );
    let after_prose_x = after.galley_pos.x
        + after
            .galley
            .pos_from_cursor(egui::text::CCursor::new(0))
            .left();
    assert!(
        (after_prose_x - prose_x).abs() <= 1.,
        "scrolling the table displaced prose"
    );
    assert!(
        after.galley_pos.x + after.galley.size().x <= 230. + 1.,
        "last column remains inaccessible"
    );
}

//! Check the real shell's PTY dimensions against its rendered terminal pane.
use eframe::egui::{self, FontId, Rect, Vec2};
use rode::{terminal::Terminal, theme::Palette};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

pub fn check_sizes(terminal: &mut Terminal, root: &Path) {
    let ctx = egui::Context::default();
    for (index, size) in [(1380., 900.), (850., 560.), (1600., 1050.)]
        .into_iter()
        .enumerate()
    {
        let mut expected = (0, 0);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size.0, size.1),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let available = ui.available_size();
                    let cell = ui.fonts_mut(|fonts| {
                        let font = FontId::monospace(14.);
                        Vec2::new(fonts.glyph_width(&font, 'M'), fonts.row_height(&font))
                    });
                    expected = (
                        ((available.y - 8.) / cell.y).floor() as u16,
                        ((available.x - 12.) / cell.x).floor() as u16,
                    );
                    assert!(terminal.ui(ui, 14., Palette::default()).is_none());
                });
            },
        );
        let marker = format!("terminal-size-{index}");
        let path = root.join(&marker);
        let _ = fs::remove_file(&path);
        terminal
            .send(format!("stty size > {marker}\n").into_bytes())
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while fs::read_to_string(&path).map_or(true, |s| s.trim().is_empty())
            && Instant::now() < until
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fs::read_to_string(&path).unwrap_or_default().trim(),
            format!("{} {}", expected.0, expected.1),
            "Shell dimensions differ from the rendered {size:?} pane: {}",
            terminal.contents()
        );
    }
}

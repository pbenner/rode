//! Check the real shell's PTY dimensions against its rendered terminal pane.
use eframe::egui::{self, FontId, Rect, Vec2};
use rode::{terminal::Terminal, theme::Palette};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

pub fn check_sizes(terminal: &mut Terminal, root: &Path) {
    if std::env::var_os("RODE_TEST_PROJECT_SHELL").is_some() {
        wait_for_prompt(terminal);
    }
    // Attachment creation precedes login-shell startup. A real command reply,
    // rather than PTY echo or a sleep, marks the shell ready for resize checks.
    let ready = root.join("terminal-ready");
    terminal
        .send(
            format!(
                "printf 'TERMINAL_%s\\n' READY > {}\n",
                rode::ssh::quote(&ready.to_string_lossy())
            )
            .into_bytes(),
        )
        .unwrap();
    assert_eq!(
        wait_for_line(terminal, &ready, Duration::from_secs(15)),
        "TERMINAL_READY"
    );
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
        let path = root.join(format!("terminal-size-{index}"));
        let _ = fs::remove_file(&path);
        terminal
            .send(
                format!(
                    "stty size > {}\n",
                    rode::ssh::quote(&path.to_string_lossy())
                )
                .into_bytes(),
            )
            .unwrap();
        assert_eq!(
            wait_for_line(terminal, &path, Duration::from_secs(5)),
            format!("{} {}", expected.0, expected.1),
            "Shell dimensions differ from the rendered {size:?} pane: {}",
            terminal.contents()
        );
    }
}

pub fn wait_for_prompt(terminal: &Terminal) {
    let until = Instant::now() + Duration::from_secs(15);
    while !terminal.contents().contains("RODE_TEST_SHELL_READY> ") {
        assert!(
            terminal.is_alive() && Instant::now() < until,
            "Fixture shell did not become ready: {}",
            terminal.contents()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_line(terminal: &Terminal, path: &Path, timeout: Duration) -> String {
    let until = Instant::now() + timeout;
    loop {
        let output = fs::read_to_string(path);
        if let Ok(line) = &output
            && line.ends_with('\n')
        {
            return line.trim().to_owned();
        }
        assert!(
            terminal.is_alive() && Instant::now() < until,
            "Shell did not publish a complete line to {}: {output:?}\n{}",
            path.display(),
            terminal.contents()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

//! Display-only contrast protection for terminal text, including truecolor prompts.
//!
//! Shells keep their original escape sequences and backgrounds. Adjusting the
//! rendered foreground avoids modifying PS1, remote environments or shared PTYs.
//! Cache a bounded set of color pairs per view; the sRGB lookup table is built
//! once, so repainting repeated text does not repeat gamma/contrast searches.
use eframe::egui::Color32;
use std::{collections::HashMap, sync::LazyLock};

const CACHE_LIMIT: usize = 256;
static LINEAR: LazyLock<[f32; 256]> = LazyLock::new(|| {
    std::array::from_fn(|i| {
        let s = i as f32 / 255.;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    })
});

fn luminance(c: Color32) -> f32 {
    0.2126 * LINEAR[c.r() as usize]
        + 0.7152 * LINEAR[c.g() as usize]
        + 0.0722 * LINEAR[c.b() as usize]
}

fn ratio(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn adjust(fg: Color32, bg: Color32, minimum: f32) -> Color32 {
    let background = luminance(bg);
    let foreground = luminance(fg);
    if ratio(foreground, background) >= minimum {
        return fg;
    }
    // Preserve the light/dark direction where possible, falling back to the
    // opposite pole if only it can meet the requested contrast. Some targets
    // are impossible against a midtone; then use the most readable pole.
    let light = ratio(1., background);
    let dark = ratio(0., background);
    let toward_white = if light >= minimum && dark >= minimum {
        foreground >= background
    } else {
        light >= dark
    };
    let target = if toward_white {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    if light.max(dark) < minimum {
        return target;
    }
    let mix = |amount: u16| {
        let channel = |a: u8, b: u8| {
            ((u32::from(a) * (255 - u32::from(amount)) + u32::from(b) * u32::from(amount) + 127)
                / 255) as u8
        };
        Color32::from_rgb(
            channel(fg.r(), target.r()),
            channel(fg.g(), target.g()),
            channel(fg.b(), target.b()),
        )
    };
    let (mut low, mut high) = (0, 255);
    while low < high {
        let mid = (low + high) / 2;
        if ratio(luminance(mix(mid)), background) >= minimum {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    mix(high)
}

/// Window-owned rendering cache; it never changes the parser or terminal input.
#[derive(Default)]
pub(super) struct Contrast {
    minimum: f32,
    pairs: HashMap<(Color32, Color32), Color32>,
}

impl Contrast {
    /// Protect text after inverse/cursor colors have resolved. Leave graphics
    /// (including Powerline separators and block-based images) and blanks exact.
    pub(super) fn foreground(
        &mut self,
        fg: Color32,
        bg: Color32,
        text: &str,
        minimum: f32,
    ) -> Color32 {
        if !minimum.is_finite() || minimum <= 1. {
            return fg;
        }
        if text.chars().all(|c| {
            c.is_whitespace() || matches!(c, '\u{2500}'..='\u{259f}' | '\u{e0b0}'..='\u{e0d7}')
        }) {
            return fg;
        }
        let minimum = minimum.min(21.);
        if self.minimum != minimum {
            self.pairs.clear();
            self.minimum = minimum;
        }
        if let Some(color) = self.pairs.get(&(fg, bg)) {
            return *color;
        }
        let color = adjust(fg, bg, minimum);
        if self.pairs.len() == CACHE_LIMIT {
            self.pairs.clear();
        }
        self.pairs.insert((fg, bg), color);
        color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_text_survives_and_dark_or_light_text_is_repaired() {
        for bg in [Color32::BLACK, Color32::WHITE, Color32::from_rgb(5, 20, 40)] {
            for fg in [
                Color32::BLACK,
                Color32::WHITE,
                Color32::from_rgb(10, 20, 60),
            ] {
                let corrected = adjust(fg, bg, 4.5);
                assert!(ratio(luminance(corrected), luminance(bg)) >= 4.5);
                if ratio(luminance(fg), luminance(bg)) >= 4.5 {
                    assert_eq!(corrected, fg);
                }
            }
        }
        let mid = Color32::from_gray(128);
        let best = adjust(mid, mid, 21.);
        assert_eq!(best, Color32::BLACK);
    }

    #[test]
    fn cache_is_bounded_and_tracks_both_colors_and_contrast() {
        let mut cache = Contrast::default();
        let fg = Color32::from_rgb(5, 10, 20);
        assert_eq!(cache.foreground(fg, fg, "text", 1.), fg);
        for text in [" ", "\u{e0b0}", "\u{2588}", "\u{2500}"] {
            assert_eq!(cache.foreground(fg, fg, text, 7.), fg);
        }
        for bg in [Color32::BLACK, Color32::WHITE] {
            for minimum in [7., 3., 4.5] {
                let c = cache.foreground(fg, bg, "prompt", minimum);
                assert_eq!(cache.foreground(fg, bg, "prompt", minimum), c);
                assert!(ratio(luminance(c), luminance(bg)) >= minimum);
            }
        }
        for i in 0..2048_u16 {
            let c = Color32::from_rgb(i as u8, (i >> 8) as u8, 20);
            cache.foreground(c, fg, "x", 4.5);
            assert!(cache.pairs.len() <= CACHE_LIMIT);
        }
    }
}

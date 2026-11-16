//! Embedded interface typography. Install once per UI context, retaining egui's
//! Unicode/emoji fallbacks and the existing monospace editor/terminal family.
//! Theme changes never reload the face or invalidate an unchanged font atlas.
use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

pub(super) fn install(ctx: &egui::Context) {
    let key = egui::Id::new("interface-fonts-installed");
    let first = ctx.data_mut(|data| {
        if data.get_temp::<bool>(key).unwrap_or(false) {
            false
        } else {
            data.insert_temp(key, true);
            true
        }
    });
    if !first {
        return;
    }
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".into(),
        FontData::from_static(include_bytes!("../../assets/fonts/Inter-Regular.ttf")).into(),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Inter".into());
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_interface_face_is_pinned_and_keeps_monospace_and_warm_layouts() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/fonts/manifest.json")).unwrap();
        assert_eq!(
            crate::files::digest(include_bytes!("../../assets/fonts/Inter-Regular.ttf")),
            manifest["files"]["Inter-Regular.ttf"].as_str().unwrap()
        );
        assert_eq!(
            crate::files::digest(include_bytes!("../../assets/fonts/Inter-LICENSE.txt")),
            manifest["files"]["Inter-LICENSE.txt"].as_str().unwrap()
        );
        let ctx = egui::Context::default();
        let mut previous = None;
        for _ in 0..2 {
            install(&ctx);
            let _ = ctx.run(Default::default(), |ctx| {
                ctx.fonts_mut(|fonts| {
                    let proportional = fonts.glyph_width(&egui::FontId::proportional(14.), 'i');
                    let monospace = fonts.glyph_width(&egui::FontId::monospace(14.), 'i');
                    assert!(proportional < monospace);
                    let galley = fonts.layout_no_wrap(
                        "Interface".into(),
                        egui::FontId::proportional(14.),
                        egui::Color32::WHITE,
                    );
                    if let Some(previous) = &previous {
                        assert!(std::sync::Arc::ptr_eq(previous, &galley));
                    }
                    previous = Some(galley);
                });
            });
        }
    }
}

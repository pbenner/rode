//! Embedded Inter interface and JetBrains Mono editor/terminal typography.
//! Install once per UI context, retaining egui's Unicode/emoji fallbacks.
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
    for (name, data, family) in [
        (
            "Inter",
            include_bytes!("../../assets/fonts/Inter-Regular.ttf").as_slice(),
            FontFamily::Proportional,
        ),
        (
            "JetBrains Mono",
            include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf").as_slice(),
            FontFamily::Monospace,
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), FontData::from_static(data).into());
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, name.into());
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_faces_are_pinned_and_keep_fallbacks_and_warm_layouts() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/fonts/manifest.json")).unwrap();
        let pinned: std::collections::BTreeMap<_, _> = manifest["fonts"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|font| font["files"].as_object().unwrap().iter())
            .map(|(name, digest)| (name.as_str(), digest.as_str().unwrap()))
            .collect();
        for (name, data) in [
            (
                "Inter-Regular.ttf",
                include_bytes!("../../assets/fonts/Inter-Regular.ttf").as_slice(),
            ),
            (
                "Inter-LICENSE.txt",
                include_bytes!("../../assets/fonts/Inter-LICENSE.txt").as_slice(),
            ),
            (
                "JetBrainsMono-Regular.ttf",
                include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf").as_slice(),
            ),
            (
                "JetBrainsMono-OFL.txt",
                include_bytes!("../../assets/fonts/JetBrainsMono-OFL.txt").as_slice(),
            ),
        ] {
            assert_eq!(crate::files::digest(data), pinned[name]);
        }
        assert_eq!(pinned.len(), 4);
        let ctx = egui::Context::default();
        let mut previous = None;
        for _ in 0..2 {
            install(&ctx);
            let _ = ctx.run(Default::default(), |ctx| {
                ctx.fonts_mut(|fonts| {
                    let families = &fonts.definitions().families;
                    assert_eq!(families[&FontFamily::Proportional][0], "Inter");
                    assert_eq!(families[&FontFamily::Monospace][0], "JetBrains Mono");
                    for fallback in [
                        "Hack",
                        "Ubuntu-Light",
                        "NotoEmoji-Regular",
                        "emoji-icon-font",
                    ] {
                        assert!(
                            families[&FontFamily::Monospace]
                                .iter()
                                .any(|name| name == fallback)
                        );
                    }
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

    #[test]
    fn monospace_glyphs_keep_fixed_cells_and_antialiased_edges_at_display_scales() {
        for theme in ["atelier-dark", "atelier-light"] {
            let mut previous_height = 0;
            for scale in [1., 1.25, 1.5, 2.] {
                let ctx = egui::Context::default();
                crate::theme::install(&ctx, crate::theme::Theme::builtin(theme).unwrap().palette);
                ctx.set_pixels_per_point(scale);
                let output = ctx.run(Default::default(), |ctx| {
                    assert!(ctx.tessellation_options(|options| options.round_text_to_pixels));
                    ctx.fonts_mut(|fonts| {
                        let id = egui::FontId::monospace(15.);
                        let width = fonts.glyph_width(&id, 'M');
                        for c in "il10O@éλ─".chars() {
                            assert!(fonts.has_glyph(&id, c));
                            assert!(
                                (fonts.glyph_width(&id, c) - width).abs() < 0.001,
                                "Unequal cell width for {c}"
                            );
                        }
                        let galley = fonts.layout_no_wrap("S".into(), id, egui::Color32::WHITE);
                        assert_eq!(galley.pixels_per_point, scale);
                        let vertices = &galley.rows[0].visuals.mesh.vertices;
                        // Galley UVs are atlas pixels until tessellation. Inspect
                        // this glyph alone, excluding the atlas's antialiased discs.
                        let min = vertices
                            .iter()
                            .fold(egui::pos2(f32::INFINITY, f32::INFINITY), |p, v| p.min(v.uv));
                        let max = vertices.iter().fold(egui::Pos2::ZERO, |p, v| p.max(v.uv));
                        let image = fonts.image();
                        let mut levels = std::collections::BTreeSet::new();
                        for y in min.y as usize..max.y as usize {
                            for x in min.x as usize..max.x as usize {
                                let alpha = image[(x, y)].a();
                                if alpha > 0 && alpha < 255 {
                                    levels.insert(alpha);
                                }
                            }
                        }
                        assert!(
                            levels.len() > 4,
                            "Missing grayscale glyph edges at {scale}x in {theme}"
                        );
                        let height = (max.y - min.y) as usize;
                        assert!(
                            height > previous_height,
                            "Glyphs must rerasterize at increasing DPI"
                        );
                        previous_height = height;
                    });
                });
                assert!(
                    output
                        .textures_delta
                        .set
                        .iter()
                        .any(|(id, delta)| *id == egui::TextureId::Managed(0)
                            && delta.options == egui::TextureOptions::LINEAR)
                );
            }
        }
    }
}

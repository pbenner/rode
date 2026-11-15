//! Native approximation of SynthWave's optional Neon Dreams text shadows.
//! Only visible colored glyphs contribute; selection, caret and text retain
//! TextEdit's geometry and paint order. There are no timers or new textures.
use eframe::egui::{self, Color32, Galley, Pos2, Shape, Vec2};
use std::sync::Arc;

// Foreground substitutions and halo hues from the pinned upstream theme_template.js.
const TOKENS: [(Color32, Color32, Color32); 5] = [
    (
        Color32::from_rgb(0xfe, 0x44, 0x50),
        Color32::from_rgb(0xff, 0xf5, 0xf6),
        Color32::from_rgb(0xfc, 0x1f, 0x2c),
    ),
    (
        Color32::from_rgb(0xff, 0x7e, 0xdb),
        Color32::from_rgb(0xf9, 0x2a, 0xad),
        Color32::from_rgb(0xdc, 0x07, 0x8e),
    ),
    (
        Color32::from_rgb(0xfe, 0xde, 0x5d),
        Color32::from_rgb(0xf4, 0xee, 0xe4),
        Color32::from_rgb(0xf3, 0x9f, 0x05),
    ),
    (
        Color32::from_rgb(0x72, 0xf1, 0xb8),
        Color32::from_rgb(0x72, 0xf1, 0xb8),
        Color32::from_rgb(0x25, 0x7c, 0x55),
    ),
    (
        Color32::from_rgb(0x36, 0xf9, 0xf6),
        Color32::from_rgb(0xfd, 0xfd, 0xfd),
        Color32::from_rgb(0x03, 0xed, 0xf9),
    ),
];

/// Match the upstream glow's bright core while keeping other tokens unchanged.
pub(super) fn foreground(color: Color32) -> Color32 {
    TOKENS
        .iter()
        .find(|(base, _, _)| *base == color)
        .map_or(color, |(_, core, _)| *core)
}

/// Reuse visible glyph meshes in paint-only text shapes. Let egui normalize
/// atlas coordinates during tessellation: later widgets can grow the font
/// texture after this editor has been laid out. Decoration has independent
/// limits; TextEdit always paints the complete source and owns its geometry.
pub(super) fn shape(ui: &egui::Ui, galley: &Galley, position: Pos2) -> Shape {
    if galley.rows.len() > 8192 {
        return Shape::Noop;
    }
    let clip = ui.clip_rect().expand(3.);
    let mut rows = Vec::new();
    let mut remaining = 4096;
    let mut row_budget = 16 * 1024;
    let mut bounds = egui::Rect::NOTHING;
    let mut vertices_count = 0;
    let mut indices_count = 0;
    for row in &galley.rows {
        let origin = position + row.pos.to_vec2();
        if !clip.intersects(row.visuals.mesh_bounds.translate(origin.to_vec2()))
            || row.glyphs.len() > row_budget
        {
            continue;
        }
        let mut mesh = egui::Mesh::with_texture(row.visuals.mesh.texture_id);
        for vertices in
            row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()].chunks_exact(4)
        {
            let Some((_, _, halo)) = TOKENS
                .iter()
                .find(|(_, core, _)| *core == vertices[0].color)
            else {
                continue;
            };
            let positions = [
                vertices[0].pos,
                vertices[1].pos,
                vertices[2].pos,
                vertices[3].pos,
            ]
            .map(|point| point + origin.to_vec2());
            if !clip.intersects(egui::Rect::from_points(&positions)) {
                continue;
            }
            if remaining == 0 {
                break;
            }
            remaining -= 1;
            let first = mesh.vertices.len() as u32;
            for vertex in vertices {
                let mut vertex = *vertex;
                vertex.color = *halo;
                mesh.vertices.push(vertex);
            }
            mesh.add_triangle(first, first + 1, first + 2);
            mesh.add_triangle(first + 2, first + 1, first + 3);
        }
        if !mesh.is_empty() {
            row_budget -= row.glyphs.len();
            let mut halo_row = row.row.as_ref().clone();
            halo_row.visuals.mesh_bounds = mesh.calc_bounds();
            bounds = bounds.union(halo_row.visuals.mesh_bounds.translate(row.pos.to_vec2()));
            vertices_count += mesh.vertices.len();
            indices_count += mesh.indices.len();
            halo_row.visuals.glyph_index_start = 0;
            halo_row.visuals.glyph_vertex_range = 0..mesh.vertices.len();
            halo_row.visuals.mesh = mesh;
            rows.push(egui::epaint::text::PlacedRow {
                pos: row.pos,
                row: Arc::new(halo_row),
            });
        }
        if remaining == 0 || row_budget == 0 {
            break;
        }
    }
    if rows.is_empty() {
        return Shape::Noop;
    }
    let mut halo = galley.clone();
    halo.rows = rows;
    halo.elided = true;
    halo.mesh_bounds = bounds;
    halo.num_vertices = vertices_count;
    halo.num_indices = indices_count;
    let halo = Arc::new(halo);
    let mut shapes = Vec::with_capacity(8);
    for (radius, opacity) in [(2.5, 0.045), (1.25, 0.12)] {
        for offset in [Vec2::X, Vec2::Y, -Vec2::X, -Vec2::Y] {
            let mut text = egui::epaint::TextShape::new(
                position + offset * radius,
                halo.clone(),
                Color32::TRANSPARENT,
            );
            text.opacity_factor = opacity;
            shapes.push(Shape::Text(text));
        }
    }
    Shape::Vec(shapes)
}

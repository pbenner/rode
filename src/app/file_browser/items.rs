//! Shared file-item visuals. Checked selection owns the fill; the keyboard
//! cursor uses an outline so deselected files never retain selection colors.
use super::*;

// File rows own their border gutter, independent of ordinary button styling.
const LIST_PADDING: egui::Margin = egui::Margin::symmetric(8, 6);

/// Transfer-browser presentation; changing it keeps the same cursor and paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum View {
    #[default]
    List,
    Icons,
}
impl View {
    pub(in crate::app) fn id(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Icons => "icons",
        }
    }
    pub(super) fn columns(self, width: f32) -> usize {
        match self {
            Self::List => 1,
            Self::Icons => ((width + 8.) / 120.).floor().max(1.) as usize,
        }
    }
    pub(super) fn height(self, ui: &egui::Ui) -> f32 {
        match self {
            Self::List => layout::control_height(ui)
                .max(34.)
                .max(ui.text_style_height(&egui::TextStyle::Button).max(18.) + LIST_PADDING.sum().y)
                .ceil(),
            Self::Icons => {
                84. + 2. * ui.text_style_height(&egui::TextStyle::Body)
                    + ui.text_style_height(&egui::TextStyle::Small)
            }
        }
    }
}

/// One accessible item and hit target, reused by both virtualized presentations.
pub(super) fn show(
    ui: &mut egui::Ui,
    entry: &Entry,
    selected: bool,
    focused: bool,
    size: egui::Vec2,
    sense: Sense,
    view: View,
) -> egui::Response {
    let response = match view {
        View::List => {
            let check_id = ui.next_auto_id().with("file-check");
            let icon_id = ui.next_auto_id().with("file-icon");
            let label = if selected {
                RichText::new(&entry.name).color(ui.visuals().text_color())
            } else {
                RichText::new(&entry.name)
            };
            // A frameless Button removes its padding, putting the check mark
            // on the outline. Lay out inset contents inside the same full-row
            // hit target, and include that padding in the virtual row height.
            let mut prepared = egui::AtomLayout::new((
                egui::Atom::custom(check_id, egui::Vec2::splat(18.)),
                egui::Atom::custom(icon_id, egui::Vec2::splat(18.)),
                label,
                egui::Atom::grow(),
            ))
            .frame(egui::Frame::new().inner_margin(LIST_PADDING))
            .fallback_font(egui::TextStyle::Button)
            .wrap_mode(egui::TextWrapMode::Truncate)
            .sense(sense)
            .min_size(size)
            .allocate(ui);
            let visuals = ui.style().interact_selectable(&prepared.response, selected);
            let fill = if selected {
                selection_fill(ui.visuals())
            } else if prepared.response.hovered() || prepared.response.is_pointer_button_down_on() {
                visuals.weak_bg_fill
            } else {
                egui::Color32::TRANSPARENT
            };
            prepared.frame = prepared.frame.fill(fill).corner_radius(6);
            prepared.fallback_text_color = visuals.text_color();
            let output = prepared.paint(ui);
            if let Some(rect) = output.rect(check_id) {
                selection_mark(ui, rect, selected);
            }
            if let Some(rect) = output.rect(icon_id) {
                icon(entry).paint(
                    ui.painter(),
                    rect,
                    if selected {
                        visuals.text_color()
                    } else {
                        ui.visuals().weak_text_color()
                    },
                );
            }
            output.response
        }
        View::Icons => tile(ui, entry, selected, size, sense),
    };
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            selected,
            &entry.name,
        )
    });
    if selected {
        ui.painter().rect_stroke(
            response.rect.shrink(0.5),
            6,
            egui::Stroke::new(2_f32, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    } else if focused {
        ui.painter().rect_stroke(
            response.rect.shrink(0.5),
            6,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    response
}

/// Use the theme accent at a stronger surface tint than text selection. The
/// filled check and border also distinguish selection without relying on tint.
fn selection_fill(visuals: &egui::Visuals) -> egui::Color32 {
    visuals.panel_fill.lerp_to_gamma(
        visuals.selection.stroke.color.to_opaque(),
        if visuals.dark_mode { 0.32 } else { 0.24 },
    )
}

fn check_color(fill: egui::Color32) -> egui::Color32 {
    let linear = egui::Rgba::from(fill.to_opaque());
    let luminance = 0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b();
    if luminance > 0.179 {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}

/// Paint within the existing item hit target; the mark never steals clicks or
/// drag events from the file. Unchecked items keep a neutral outlined box.
fn selection_mark(ui: &egui::Ui, rect: egui::Rect, selected: bool) {
    let rect = rect.shrink(0.5);
    if selected {
        let accent = ui.visuals().selection.stroke.color.to_opaque();
        ui.painter().rect_filled(rect, 4, accent);
        Icon::Check.paint(ui.painter(), rect.shrink(2.), check_color(accent));
    } else {
        ui.painter().rect_stroke(
            rect,
            4,
            egui::Stroke::new(1.2_f32, ui.visuals().weak_text_color()),
            egui::StrokeKind::Inside,
        );
    }
}

fn icon(entry: &Entry) -> Icon {
    if entry.dir { Icon::Folder } else { Icon::Files }
}

fn tile(
    ui: &mut egui::Ui,
    entry: &Entry,
    selected: bool,
    size: egui::Vec2,
    sense: Sense,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.style().interact_selectable(&response, selected);
    let hovered = response.hovered() || response.is_pointer_button_down_on();
    let fill = if selected {
        selection_fill(ui.visuals())
    } else if hovered {
        visuals.weak_bg_fill
    } else {
        ui.visuals().faint_bg_color
    };
    ui.painter().rect(
        rect.shrink(0.5),
        8,
        fill,
        egui::Stroke::NONE,
        egui::StrokeKind::Inside,
    );
    selection_mark(
        ui,
        egui::Rect::from_min_size(
            egui::pos2(rect.right() - 25., rect.top() + 7.),
            egui::Vec2::splat(18.),
        ),
        selected,
    );
    let text_color = if selected {
        ui.visuals().text_color()
    } else {
        visuals.text_color()
    };
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 34.),
        egui::Vec2::splat((rect.width() - 62.).clamp(24., 42.)),
    );
    icon(entry).paint(ui.painter(), icon_rect, text_color);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::simple(
        entry.name.clone(),
        font,
        text_color,
        (rect.width() - 16.).max(1.),
    );
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = true;
    job.halign = Align::Center;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let painter = ui.painter().with_clip_rect(rect.shrink(4.));
    painter.galley(
        egui::pos2(rect.center().x - galley.rect.center().x, rect.top() + 65.),
        galley,
        text_color,
    );
    let kind = if entry.dir {
        "Folder".into()
    } else {
        std::path::Path::new(&entry.name)
            .extension()
            .and_then(|ext| ext.to_str())
            .filter(|ext| ext.len() <= 8)
            .map(|ext| ext.to_uppercase())
            .unwrap_or_else(|| "File".into())
    };
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 12.),
        egui::Align2::CENTER_BOTTOM,
        kind,
        egui::TextStyle::Small.resolve(ui.style()),
        ui.visuals().weak_text_color(),
    );
    response
}

#[cfg(test)]
mod tests;

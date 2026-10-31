// SPDX-License-Identifier: MIT
//! A small native outline icon set. All glyphs use the same 24-unit grid and
//! scale with the UI; no icon font, image decoder, texture, or theme I/O is needed.
//! See docs/licensing.md for the source review and artwork policy. Importing or
//! tracing an upstream icon requires recording its own license and attribution;
//! implementing a shape in Rust does not by itself establish independent origin.
use eframe::egui::{
    self, Atom, Button, Color32, Painter, Pos2, Rect, Response, RichText, Shape, Stroke,
    StrokeKind, Ui, Vec2, Widget,
};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Files,
    Search,
    Remote,
    Terminal,
    Git,
    Monitor,
    Problems,
    Python,
    Settings,
    Extensions,
    Help,
    NewFile,
    Folder,
    Save,
    SaveAs,
    SaveAll,
    Exit,
    Format,
    Check,
    Fix,
    Play,
    Palette,
    Refresh,
    Close,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    More,
    Copy,
    Chat,
    Debug,
    Pause,
    Stop,
    StepOver,
    StepIn,
    StepOut,
}

impl Icon {
    pub(super) fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        let size = rect.width().min(rect.height());
        let canvas = Canvas {
            painter,
            origin: rect.center() - Vec2::splat(size / 2.),
            scale: size / 24.,
            stroke: Stroke::new((size / 15.).clamp(1.2, 1.8), color),
        };
        let c = &canvas;
        match self {
            Self::Debug => {
                c.rect([7., 8., 17., 20.], 4.);
                c.circle([12., 6.], 3.);
                for y in [10., 14., 18.] {
                    c.path(&[[3., y - 2.], [7., y]]);
                    c.path(&[[17., y], [21., y - 2.]]);
                }
                c.path(&[[12., 9.], [12., 20.]]);
            }
            Self::Pause => {
                c.path(&[[8., 5.], [8., 19.]]);
                c.path(&[[16., 5.], [16., 19.]]);
            }
            Self::Stop => {
                c.rect([5., 5., 19., 19.], 1.);
            }
            Self::StepOver => {
                c.path(&[
                    [4., 14.],
                    [4., 7.],
                    [8., 4.],
                    [16., 4.],
                    [20., 7.],
                    [20., 14.],
                    [16., 10.],
                ]);
                c.path(&[[20., 14.], [23., 10.]]);
                c.dot([12., 19.]);
            }
            Self::StepIn => {
                c.path(&[[12., 3.], [12., 15.], [7., 10.]]);
                c.path(&[[12., 15.], [17., 10.]]);
                c.path(&[[5., 20.], [19., 20.]]);
            }
            Self::StepOut => {
                c.path(&[[12., 16.], [12., 4.], [7., 9.]]);
                c.path(&[[12., 4.], [17., 9.]]);
                c.path(&[[5., 21.], [19., 21.]]);
            }

            Self::Chat => {
                c.path(&[
                    [5., 4.],
                    [19., 4.],
                    [21., 6.],
                    [21., 16.],
                    [19., 18.],
                    [9., 18.],
                    [4., 22.],
                    [4., 18.],
                    [2., 16.],
                    [2., 7.],
                    [5., 4.],
                ]);
                for x in [7., 12., 17.] {
                    c.dot([x, 11.]);
                }
            }
            Self::Files | Self::Copy => {
                c.rect([8., 7., 21., 21.], 2.);
                c.path(&[[16., 7.], [16., 3.], [3., 3.], [3., 16.], [8., 16.]]);
            }
            Self::Search => {
                c.circle([10.5, 10.5], 6.5);
                c.path(&[[15.5, 15.5], [21., 21.]]);
            }
            Self::Remote => {
                c.rect([3., 3., 21., 11.], 2.);
                c.dot([7., 7.]);
                c.path(&[[12., 7.], [17., 7.]]);
                c.path(&[[3., 16.], [3., 19.], [21., 19.], [21., 16.]]);
                c.path(&[[9., 15.], [12., 18.], [15., 15.]]);
                c.path(&[[12., 12.], [12., 18.]]);
            }
            Self::Terminal => {
                c.rect([2.5, 4., 21.5, 20.], 2.);
                c.path(&[[6., 9.], [9., 12.], [6., 15.]]);
                c.path(&[[12., 15.], [17., 15.]]);
            }
            Self::Git => {
                c.path(&[[6., 8.], [6., 16.]]);
                c.path(&[[6., 14.], [12., 14.], [18., 8.]]);
                c.circle([6., 5.], 2.5);
                c.circle([6., 19.], 2.5);
                c.circle([18., 5.], 2.5);
            }
            Self::Monitor => {
                c.rect([2., 3., 22., 18.], 2.);
                c.path(&[
                    [5., 11.],
                    [8., 11.],
                    [10., 7.],
                    [13., 14.],
                    [15., 10.],
                    [19., 10.],
                ]);
                c.path(&[[8., 22.], [16., 22.]]);
                c.path(&[[12., 18.], [12., 22.]]);
            }
            Self::Problems => {
                c.path(&[[12., 3.], [22., 20.], [2., 20.], [12., 3.]]);
                c.path(&[[12., 9.], [12., 13.]]);
                c.dot([12., 16.5]);
            }
            Self::Python => {
                c.path(&[
                    [12., 3.],
                    [6., 3.],
                    [6., 10.],
                    [3., 10.],
                    [3., 17.],
                    [8., 17.],
                ]);
                c.path(&[
                    [12., 21.],
                    [18., 21.],
                    [18., 14.],
                    [21., 14.],
                    [21., 7.],
                    [16., 7.],
                ]);
                c.path(&[
                    [6., 7.],
                    [16., 7.],
                    [16., 12.],
                    [8., 12.],
                    [8., 17.],
                    [18., 17.],
                ]);
                c.dot([10., 4.]);
                c.dot([14., 20.]);
            }
            Self::Settings => {
                for (x, knob) in [(5., 8.), (12., 16.), (19., 8.)] {
                    c.path(&[[x, 3.], [x, knob - 2.5]]);
                    c.path(&[[x, knob + 2.5], [x, 21.]]);
                    c.circle([x, knob], 2.5);
                }
            }
            Self::Extensions => {
                for rect in [
                    [3., 3., 10., 10.],
                    [3., 13., 10., 20.],
                    [13., 13., 20., 20.],
                ] {
                    c.rect(rect, 0.5);
                }
                c.path(&[[18., 1.5], [23., 6.5], [18., 11.5], [13., 6.5], [18., 1.5]]);
            }
            Self::Help => {
                c.circle([12., 12.], 9.);
                c.path(&[
                    [9., 9.],
                    [10., 7.],
                    [14., 7.],
                    [15., 9.],
                    [14., 11.],
                    [12., 12.],
                    [12., 14.],
                ]);
                c.dot([12., 17.]);
            }
            Self::NewFile => {
                c.path(&[
                    [13., 3.],
                    [5., 3.],
                    [5., 21.],
                    [19., 21.],
                    [19., 9.],
                    [13., 3.],
                    [13., 9.],
                    [19., 9.],
                ]);
                c.path(&[[9., 15.], [15., 15.]]);
                c.path(&[[12., 12.], [12., 18.]]);
            }
            Self::Folder => {
                c.path(&[
                    [3., 7.],
                    [3., 4.],
                    [9., 4.],
                    [12., 7.],
                    [21., 7.],
                    [21., 10.],
                ]);
                c.path(&[
                    [3., 7.],
                    [3., 20.],
                    [19., 20.],
                    [22., 10.],
                    [7., 10.],
                    [3., 20.],
                ]);
            }
            Self::Save | Self::SaveAll => {
                if matches!(self, Self::SaveAll) {
                    c.path(&[[2., 7.], [2., 22.], [17., 22.]]);
                }
                c.path(&[
                    [5., 3.],
                    [17., 3.],
                    [21., 7.],
                    [21., 20.],
                    [5., 20.],
                    [5., 3.],
                ]);
                c.path(&[[9., 3.], [9., 9.], [16., 9.], [16., 3.]]);
                c.path(&[[9., 20.], [9., 14.], [17., 14.], [17., 20.]]);
            }
            Self::SaveAs => {
                c.path(&[
                    [12., 20.],
                    [4., 20.],
                    [4., 3.],
                    [17., 3.],
                    [21., 7.],
                    [21., 10.],
                ]);
                c.path(&[[8., 3.], [8., 9.], [16., 9.], [16., 3.]]);
                c.path(&[[8., 20.], [8., 14.], [12., 14.]]);
                c.path(&[
                    [14., 22.],
                    [15., 18.],
                    [20., 13.],
                    [23., 16.],
                    [18., 21.],
                    [14., 22.],
                ]);
            }
            Self::Exit => {
                c.path(&[[10., 3.], [4., 3.], [4., 21.], [10., 21.]]);
                c.path(&[[10., 12.], [21., 12.]]);
                c.path(&[[17., 8.], [21., 12.], [17., 16.]]);
            }
            Self::Format => {
                c.path(&[[7., 5.], [3., 9.], [7., 13.]]);
                c.path(&[[17., 5.], [21., 9.], [17., 13.]]);
                c.path(&[[14., 3.], [10., 15.]]);
                c.path(&[[5., 18.], [19., 18.]]);
                c.path(&[[8., 22.], [16., 22.]]);
            }
            Self::Check => {
                c.path(&[[4., 12.], [9., 17.], [20., 6.]]);
            }
            Self::Fix => {
                c.path(&[
                    [14., 3.],
                    [10., 7.],
                    [11., 12.],
                    [3., 20.],
                    [6., 23.],
                    [14., 15.],
                    [19., 16.],
                    [23., 12.],
                    [18., 12.],
                    [16., 10.],
                    [16., 5.],
                    [14., 3.],
                ]);
            }
            Self::Play => c.path(&[[7., 3.], [21., 12.], [7., 21.], [7., 3.]]),
            Self::Palette => {
                c.path(&[
                    [21., 13.],
                    [20., 7.],
                    [16., 3.],
                    [9., 3.],
                    [4., 7.],
                    [3., 13.],
                    [6., 19.],
                    [12., 21.],
                    [15., 20.],
                    [15., 16.],
                    [17., 14.],
                    [21., 13.],
                ]);
                c.dot([8., 9.]);
                c.dot([13., 7.]);
                c.dot([17., 10.]);
                c.dot([7., 14.]);
            }
            Self::Refresh => {
                c.path(&[
                    [20., 9.],
                    [18., 5.],
                    [13., 3.],
                    [8., 4.],
                    [4., 8.],
                    [3., 13.],
                    [5., 18.],
                    [10., 21.],
                    [16., 20.],
                    [20., 16.],
                ]);
                c.path(&[[20., 3.], [20., 9.], [14., 9.]]);
            }
            Self::Close => {
                c.path(&[[6., 6.], [18., 18.]]);
                c.path(&[[18., 6.], [6., 18.]]);
            }
            Self::ChevronLeft => c.path(&[[15., 5.], [8., 12.], [15., 19.]]),
            Self::ChevronRight => c.path(&[[9., 5.], [16., 12.], [9., 19.]]),
            Self::ChevronUp => c.path(&[[5., 15.], [12., 8.], [19., 15.]]),
            Self::ChevronDown => c.path(&[[5., 9.], [12., 16.], [19., 9.]]),
            Self::More => {
                for x in [5., 12., 19.] {
                    c.dot([x, 12.]);
                }
            }
        }
    }
}

struct Canvas<'a> {
    painter: &'a Painter,
    origin: Pos2,
    scale: f32,
    stroke: Stroke,
}
impl Canvas<'_> {
    fn point(&self, [x, y]: [f32; 2]) -> Pos2 {
        self.origin + egui::vec2(x, y) * self.scale
    }
    fn path(&self, points: &[[f32; 2]]) {
        self.painter.add(Shape::line(
            points.iter().map(|p| self.point(*p)).collect(),
            self.stroke,
        ));
        // Small round end caps keep every stroke consistent at fractional DPI.
        for point in [points.first(), points.last()].into_iter().flatten() {
            self.painter.circle_filled(
                self.point(*point),
                self.stroke.width / 2.,
                self.stroke.color,
            );
        }
    }
    fn rect(&self, [x1, y1, x2, y2]: [f32; 4], radius: f32) {
        self.painter.rect_stroke(
            Rect::from_min_max(self.point([x1, y1]), self.point([x2, y2])),
            radius * self.scale,
            self.stroke,
            StrokeKind::Middle,
        );
    }
    fn circle(&self, center: [f32; 2], radius: f32) {
        self.painter
            .circle_stroke(self.point(center), radius * self.scale, self.stroke);
    }
    fn dot(&self, center: [f32; 2]) {
        self.painter.circle_filled(
            self.point(center),
            self.stroke.width * 0.8,
            self.stroke.color,
        );
    }
}

/// Paint icons inside ordinary egui buttons so focus, menus, shortcuts, and
/// disabled states keep egui's existing behavior and accessibility metadata.
pub(super) struct IconButton<'a> {
    icon: Icon,
    label: &'a str,
    shortcut: &'a str,
    icon_only: bool,
    selected: bool,
    truncate: bool,
    tint: Option<Color32>,
    size: f32,
    min_size: Vec2,
}
impl<'a> IconButton<'a> {
    pub(super) fn new(icon: Icon, label: &'a str) -> Self {
        Self {
            icon,
            label,
            shortcut: "",
            icon_only: false,
            selected: false,
            truncate: false,
            tint: None,
            size: 16.,
            min_size: Vec2::ZERO,
        }
    }
    /// The accessible label also supplies the tooltip; callers should not add another one.
    pub(super) fn icon(icon: Icon, label: &'a str) -> Self {
        Self {
            icon_only: true,
            ..Self::new(icon, label)
        }
    }
    pub(super) fn shortcut(mut self, shortcut: &'a str) -> Self {
        self.shortcut = shortcut;
        self
    }
    pub(super) fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub(super) fn color(mut self, color: Color32) -> Self {
        self.tint = Some(color);
        self
    }
    pub(super) fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }
    pub(super) fn icon_size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    pub(super) fn min_size(mut self, size: Vec2) -> Self {
        self.min_size = size;
        self
    }
}
impl Widget for IconButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let id = ui.next_auto_id().with("outline-icon");
        let icon = Atom::custom(id, Vec2::splat(self.size));
        let mut button = if self.icon_only {
            Button::new(icon).frame(false).small()
        } else {
            let mut text = RichText::new(self.label);
            if let Some(color) = self.tint {
                text = text.color(color);
            }
            Button::new((icon, text))
        }
        .selected(self.selected)
        .min_size(self.min_size);
        if self.truncate {
            button = button.truncate();
        }
        if !self.shortcut.is_empty() {
            button = button.shortcut_text(self.shortcut);
        }
        let output = button.atom_ui(ui);
        let icon_rect = output.rect(id);
        let response = output.response;
        if ui.is_rect_visible(response.rect) {
            let visuals = ui.style().interact_selectable(&response, self.selected);
            if self.icon_only && (response.hovered() || response.has_focus() || self.selected) {
                ui.painter()
                    .rect_filled(response.rect, 4., visuals.weak_bg_fill);
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        response.rect,
                        4.,
                        visuals.bg_stroke,
                        StrokeKind::Inside,
                    );
                }
            }
            let rect = if self.icon_only {
                Some(Rect::from_center_size(
                    response.rect.center(),
                    Vec2::splat(self.size),
                ))
            } else {
                icon_rect
            };
            if let Some(rect) = rect {
                let color = self.tint.unwrap_or_else(|| {
                    if self.selected {
                        ui.visuals().selection.stroke.color
                    } else if response.hovered() {
                        visuals.text_color()
                    } else {
                        ui.visuals().weak_text_color()
                    }
                });
                self.icon.paint(ui.painter(), rect, color);
            }
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), self.label)
        });
        if self.icon_only {
            response.on_hover_text(self.label)
        } else {
            response
        }
    }
}

pub(super) fn small(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    ui.add_sized([26., 18.], IconButton::icon(icon, label).icon_size(14.))
}

pub(super) fn submenu<R>(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    contents: impl FnOnce(&mut Ui) -> R,
) {
    let id = ui.next_auto_id().with("submenu-icon");
    let arrow_id = id.with("arrow");
    let output = Button::new((Atom::custom(id, Vec2::splat(16.)), label))
        .right_text(Atom::custom(arrow_id, Vec2::splat(14.)))
        .atom_ui(ui);
    let color = ui.visuals().weak_text_color();
    if let Some(rect) = output.rect(id) {
        icon.paint(ui.painter(), rect, color);
    }
    if let Some(rect) = output.rect(arrow_id) {
        Icon::ChevronRight.paint(ui.painter(), rect, color);
    }
    egui::containers::menu::SubMenu::new().show(ui, &output.response, contents);
}

pub(super) fn overflow(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) -> Response {
    let id = ui.next_auto_id().with("editors-menu-icon");
    let (response, _) = egui::containers::menu::MenuButton::from_button(
        Button::new(Atom::custom(id, Vec2::splat(16.)))
            .min_size(egui::vec2(32., 36.))
            .frame(false),
    )
    .ui(ui, contents);
    Icon::More.paint(
        ui.painter(),
        Rect::from_center_size(response.rect.center(), Vec2::splat(16.)),
        ui.visuals().weak_text_color(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            "All open editors",
        )
    });
    response
}

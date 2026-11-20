//! Named native controls and button-only focus groups. Widget behavior stays with egui.
use super::*;

/// Discoverable native disclosure header, including collapsed sections whose
/// child controls are not rendered yet.
pub(crate) struct CollapsingHeader {
    inner: egui::CollapsingHeader,
    label: String,
}
impl CollapsingHeader {
    pub(crate) fn new(text: impl Into<WidgetText>) -> Self {
        let text = text.into();
        Self {
            label: text.text().into(),
            inner: egui::CollapsingHeader::new(text),
        }
    }
    pub(crate) fn id_salt(mut self, id: impl std::hash::Hash) -> Self {
        self.inner = self.inner.id_salt(id);
        self
    }
    pub(crate) fn default_open(mut self, open: bool) -> Self {
        self.inner = self.inner.default_open(open);
        self
    }
    pub(crate) fn open(mut self, open: Option<bool>) -> Self {
        self.inner = self.inner.open(open);
        self
    }
    pub(crate) fn show<R>(
        self,
        ui: &mut egui::Ui,
        add: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::CollapsingResponse<R> {
        let result = self.inner.show(ui, add);
        target(ui, &self.label, &result.header_response);
        result
    }
}

/// Match Ui::collapsing while publishing the native disclosure control.
pub(crate) fn collapsing<R>(
    ui: &mut egui::Ui,
    text: impl Into<WidgetText>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::CollapsingResponse<R> {
    CollapsingHeader::new(text).show(ui, add)
}

/// Named native menu button. Popup ownership and keyboard activation stay in egui.
pub(crate) struct MenuButton<'a> {
    inner: egui::containers::menu::MenuButton<'a>,
    label: String,
}
impl<'a> MenuButton<'a> {
    pub(crate) fn new(label: &str) -> Self {
        Self {
            inner: egui::containers::menu::MenuButton::new(label.to_owned()),
            label: format!("{label} menu"),
        }
    }
    pub(crate) fn from_button(button: egui::Button<'a>, label: &str) -> Self {
        Self {
            inner: egui::containers::menu::MenuButton::from_button(
                button.stroke(egui::Stroke::NONE),
            ),
            label: label.into(),
        }
    }
    pub(crate) fn config(mut self, config: egui::containers::menu::MenuConfig) -> Self {
        self.inner = self.inner.config(config);
        self
    }
    /// Apply an explicit frame choice after the shared menu-button treatment.
    pub(crate) fn frame(mut self, frame: bool) -> Self {
        self.inner.button = self.inner.button.frame(frame);
        self
    }
    pub(crate) fn ui<R>(
        self,
        ui: &mut egui::Ui,
        add: impl FnOnce(&mut egui::Ui) -> R,
    ) -> (Response, Option<egui::InnerResponse<R>>) {
        let result = self.inner.ui(ui, add);
        crate::theme::button_focus(ui, &result.0, None);
        target(ui, &self.label, &result.0);
        result
    }
}

/// Wrap an existing native widget without changing its input or activation path.
pub(crate) fn named<'a>(label: &'a str, widget: impl Widget + 'a) -> impl Widget + 'a {
    move |ui: &mut egui::Ui| {
        let response = widget.ui(ui);
        field_target(ui, label, &response);
        response
    }
}
pub(crate) fn editor_named<'a>(label: &'a str, widget: impl Widget + 'a) -> impl Widget + 'a {
    move |ui: &mut egui::Ui| {
        let response = widget.ui(ui);
        editor_target(ui, label, &response);
        response
    }
}

/// Native button with production discovery metadata. Its builders delegate to
/// egui, preserving existing painting, focus and activation behavior.
pub(crate) struct Button<'a> {
    inner: egui::Button<'a>,
    label: String,
    stroke_override: bool,
    fill: Option<egui::Color32>,
}
impl<'a> Button<'a> {
    pub(crate) fn selectable(selected: bool, text: impl Into<WidgetText>) -> Self {
        let text = text.into();
        Self {
            label: text.text().into(),
            inner: egui::Button::selectable(selected, text).stroke(egui::Stroke::NONE),
            stroke_override: false,
            fill: None,
        }
    }
    pub(crate) fn new(text: impl Into<WidgetText>) -> Self {
        let text = text.into();
        Self {
            label: text.text().into(),
            inner: egui::Button::new(text).stroke(egui::Stroke::NONE),
            stroke_override: false,
            fill: None,
        }
    }
    pub(crate) fn selected(mut self, value: bool) -> Self {
        self.inner = self.inner.selected(value);
        self
    }
    pub(crate) fn small(mut self) -> Self {
        self.inner = self.inner.small();
        self
    }
    pub(crate) fn frame(mut self, value: bool) -> Self {
        self.inner = self.inner.frame(value);
        self
    }
    /// Put shortcuts in a trailing column, independently of the visible label.
    pub(crate) fn shortcut_text(mut self, shortcut: &str) -> Self {
        self.inner = self.inner.shortcut_text(shortcut.to_owned());
        self
    }
    pub(crate) fn truncate(mut self) -> Self {
        self.inner = self.inner.truncate();
        self
    }
    pub(crate) fn wrap(mut self) -> Self {
        self.inner = self.inner.wrap();
        self
    }
    pub(crate) fn min_size(mut self, size: egui::Vec2) -> Self {
        self.inner = self.inner.min_size(size);
        self
    }
    pub(crate) fn fill(mut self, color: egui::Color32) -> Self {
        self.fill = Some(color);
        self.inner = self.inner.fill(color);
        self
    }
    pub(crate) fn stroke(mut self, stroke: impl Into<egui::Stroke>) -> Self {
        self.stroke_override = true;
        self.inner = self.inner.stroke(stroke);
        self
    }
}
impl Widget for Button<'_> {
    fn ui(self, ui: &mut egui::Ui) -> Response {
        let response = self
            .inner
            .sense(button_sense(ui, &self.label, Sense::click()))
            .ui(ui);
        if !self.stroke_override {
            crate::theme::button_focus(ui, &response, self.fill);
        }
        target(ui, &self.label, &response);
        response
    }
}
/// Render a standard native button and publish its visible name.
pub(crate) fn button(ui: &mut egui::Ui, text: impl Into<WidgetText>) -> Response {
    ui.add(Button::new(text))
}

/// Consume a native button's keyboard activation before its action transfers
/// focus to a text field rendered later in the same pass. Keep later input in
/// order; Space's adjacent text event belongs to the activation as well.
pub(crate) fn consume_activation(response: &Response) {
    if !response.clicked() || response.clicked_by(egui::PointerButton::Primary) {
        return;
    }
    response.ctx.input_mut(|input| {
        let Some(index) = input.events.iter().position(|event| {
            matches!(
                event,
                egui::Event::Key {
                    key: Key::Enter | Key::Space,
                    pressed: true,
                    ..
                }
            )
        }) else {
            return;
        };
        let space = matches!(
            input.events.remove(index),
            egui::Event::Key {
                key: Key::Space,
                ..
            }
        );
        if space && matches!(input.events.get(index), Some(egui::Event::Text(text)) if text == " ")
        {
            input.events.remove(index);
        }
    });
}

/// Render a compact native button and publish its visible name.
pub(crate) fn small_button(ui: &mut egui::Ui, text: impl Into<WidgetText>) -> Response {
    ui.add(Button::new(text).small())
}
/// Match Ui::menu_button without changing its popup layout or return value.
pub(crate) fn menu_button<R>(
    ui: &mut egui::Ui,
    text: impl Into<WidgetText>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let text = text.into();
    let label = format!("{} menu", text.text());
    let result = ui.menu_button(text, add);
    target(ui, &label, &result.response);
    result
}
/// Publish native toggle metadata without changing value or keyboard semantics.
pub(crate) fn checkbox(
    ui: &mut egui::Ui,
    value: &mut bool,
    text: impl Into<WidgetText>,
) -> Response {
    let text = text.into();
    let label = text.text().to_owned();
    let response = ui.checkbox(value, text);
    target(ui, &label, &response);
    response
}
pub(crate) fn selectable_value<T: PartialEq>(
    ui: &mut egui::Ui,
    current: &mut T,
    value: T,
    text: impl Into<WidgetText>,
) -> Response {
    let text = text.into();
    let label = text.text().to_owned();
    let response = ui.selectable_value(current, value, text);
    target(ui, &label, &response);
    response
}
pub(crate) fn selectable_label(
    ui: &mut egui::Ui,
    selected: bool,
    text: impl Into<WidgetText>,
) -> Response {
    ui.add(Button::selectable(selected, text))
}

pub(crate) fn button_sense(ui: &egui::Ui, label: &str, sense: Sense) -> Sense {
    let Some(catalog) = catalog(ui.ctx()) else {
        return sense;
    };
    let mut windows = catalog.0.lock().unwrap();
    let window = windows.entry(ui.ctx().viewport_id()).or_default();
    if let Some(group) = window.group
        && group.layer == ui.layer_id()
        && ui.is_enabled()
    {
        let member = ui.id().with(label);
        let active = window.groups.entry(group.id).or_insert(member);
        if *active != member {
            return sense.difference(Sense::focusable_noninteractive());
        }
    }
    sense
}

/// One Tab stop for a button-only toolbar or tab strip. Arrow keys move native
/// focus without activating commands; text fields/selectors stay outside groups.
pub(crate) fn group<R>(ui: &mut egui::Ui, key: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let Some(catalog) = catalog(ui.ctx()) else {
        return add(ui);
    };
    // Scope metadata without introducing a child UI or changing wrapping/layout.
    let group = ui.id().with(key);
    let old = {
        let mut windows = catalog.0.lock().unwrap();
        let window = windows.entry(ui.ctx().viewport_id()).or_default();
        let members: Vec<_> = window
            .published
            .as_ref()
            .map(|s| {
                s.targets
                    .iter()
                    .filter(|t| t.group == Some(group))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !members
            .iter()
            .any(|t| window.groups.get(&group) == Some(&t.member))
        {
            window.groups.remove(&group);
        }
        if let Some(index) = members
            .iter()
            .position(|t| ui.memory(|m| m.has_focus(t.widget)))
            && !egui::Popup::is_any_open(ui.ctx())
        {
            let mut next = index;
            for (key, step) in [(Key::ArrowLeft, -1_isize), (Key::ArrowRight, 1)] {
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                    ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
                    next = (index as isize + step).rem_euclid(members.len() as isize) as usize;
                }
            }
            if next != index {
                window.groups.insert(group, members[next].member);
                ui.memory_mut(|m| m.request_focus(members[next].widget));
            }
        }
        window.group.replace(Group {
            id: group,
            layer: ui.layer_id(),
        })
    };
    let result = add(ui);
    catalog
        .0
        .lock()
        .unwrap()
        .entry(ui.ctx().viewport_id())
        .or_default()
        .group = old;
    result
}
pub(crate) fn radio_value<T: PartialEq>(
    ui: &mut egui::Ui,
    current: &mut T,
    value: T,
    text: impl Into<WidgetText>,
) -> Response {
    let text = text.into();
    let label = text.text().to_owned();
    let response = ui.radio_value(current, value, text);
    target(ui, &label, &response);
    response
}

/// Named native selector; opening and selection remain egui operations.
pub(crate) struct ComboBox {
    inner: egui::ComboBox,
    label: String,
}
impl ComboBox {
    pub(crate) fn new(id: impl std::hash::Hash, label: &str) -> Self {
        Self {
            inner: egui::ComboBox::from_id_salt(id),
            label: label.into(),
        }
    }
    pub(crate) fn selected_text(mut self, text: impl Into<WidgetText>) -> Self {
        self.inner = self.inner.selected_text(text);
        self
    }
    pub(crate) fn width(mut self, value: f32) -> Self {
        self.inner = self.inner.width(value);
        self
    }
    pub(crate) fn truncate(mut self) -> Self {
        self.inner = self.inner.truncate();
        self
    }
    pub(crate) fn show_ui<R>(
        self,
        ui: &mut egui::Ui,
        add: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<Option<R>> {
        let result = self.inner.show_ui(ui, add);
        target(ui, &self.label, &result.response);
        result
    }
}

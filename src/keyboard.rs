//! Window-local, focus-first control discovery. Controls publish names and identities,
//! never callbacks or domain state. Native widgets retain activation and editing.
//!
//! Only accepted layout passes publish targets. Hint assignments retain identities
//! while their geometry follows layout. A focus handoff holds later keyboard events
//! until the destination acknowledges focus; no hint letters reach source or PTYs.
use eframe::egui;
use egui::{Align, Key, Sense};
use egui::{Id, Rect, Response, Widget, WidgetText};
use std::sync::Mutex;
use std::{collections::HashMap, sync::Arc};

mod view;
mod widgets;
pub(super) use view::finish;
pub(crate) use widgets::{
    Button, CollapsingHeader, ComboBox, MenuButton, button, button_sense, checkbox, collapsing,
    editor_named, group, menu_button, named, radio_value, selectable_label, selectable_value,
    small_button,
};

#[derive(Clone)]
struct Target {
    key: Id,
    widget: Id,
    label: String,
    rect: Rect,
    layer: egui::LayerId,
    enabled: bool,
    group: Option<Id>,
    member: Id,
    input: InputKind,
}
#[derive(Clone, Copy)]
enum InputKind {
    Control,
    Field,
    Editor,
}
#[derive(Clone)]
struct Snapshot {
    scope: Id,
    targets: Vec<Target>,
}
struct Choice {
    code: String,
    target: Target,
}
#[derive(Clone, Copy)]
struct Group {
    id: Id,
    layer: egui::LayerId,
}
#[derive(Default)]
enum Mode {
    #[default]
    Idle,
    Arm {
        search: bool,
        skip: bool,
        grace: u8,
    },
    Hints {
        choices: Vec<Choice>,
        prefix: String,
    },
    Search {
        targets: Vec<Target>,
        query: String,
        selected: usize,
    },
    Focus {
        target: Target,
        applied: bool,
        grace: u8,
    },
}
#[derive(Default)]
struct Window {
    scope: Option<Id>,
    draft: Vec<Target>,
    published: Option<Snapshot>,
    mode: Mode,
    origin: Option<Id>,
    held: Vec<egui::Event>,
    deferred: Vec<egui::Event>,
    deferred_ready: bool,
    group: Option<Group>,
    groups: HashMap<Id, Id>,
    notice: Option<String>,
}
const MAX_EVENTS: usize = 512;
const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;
fn event_bytes(event: &egui::Event) -> usize {
    match event {
        egui::Event::Text(value) | egui::Event::Paste(value) => value.len(),
        egui::Event::Ime(egui::ImeEvent::Preedit(value) | egui::ImeEvent::Commit(value)) => {
            value.len()
        }
        _ => 0,
    }
}
impl Window {
    fn restore_origin(&self, grace: u8) -> Mode {
        self.published
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .targets
                    .iter()
                    .find(|target| Some(target.widget) == self.origin)
            })
            .map(|target| Mode::Focus {
                target: target.clone(),
                applied: false,
                grace,
            })
            .unwrap_or(Mode::Idle)
    }
    fn hold(&mut self, event: egui::Event) {
        if self.notice.is_some() {
            return;
        }
        if self.held.len() < MAX_EVENTS
            && self
                .held
                .iter()
                .map(event_bytes)
                .sum::<usize>()
                .saturating_add(event_bytes(&event))
                <= MAX_EVENT_BYTES
        {
            self.held.push(event);
        } else {
            self.mode = Mode::Idle;
            self.held.clear();
            self.notice = Some(
                "Keyboard navigation input exceeded its limit; the pending input was discarded."
                    .into(),
            );
        }
    }
}
#[derive(Clone, Default)]
struct Catalog(Arc<Mutex<HashMap<egui::ViewportId, Window>>>);
fn catalog(ctx: &egui::Context) -> Option<Catalog> {
    ctx.data(|data| data.get_temp(Id::new("keyboard-controls")))
}

/// Install production metadata collection independently of fixture observation.
pub(super) fn install(ctx: &egui::Context) {
    let state = Catalog::default();
    ctx.data_mut(|data| data.insert_temp(Id::new("keyboard-controls"), state.clone()));
    let begin = state.clone();
    ctx.on_begin_pass(
        "keyboard-controls",
        Arc::new(move |ctx| {
            begin
                .0
                .lock()
                .unwrap()
                .entry(ctx.viewport_id())
                .or_default()
                .draft
                .clear();
        }),
    );
    ctx.on_end_pass(
        "keyboard-controls",
        Arc::new(move |ctx| {
            if ctx.will_discard() {
                return;
            }
            let mut windows = state.0.lock().unwrap();
            let window = windows.entry(ctx.viewport_id()).or_default();
            let Some(scope) = window.scope else {
                return;
            };
            window.deferred_ready = !window.deferred.is_empty();
            let mut targets: Vec<_> = window
                .draft
                .iter()
                .filter(|target| {
                    target.enabled && ctx.memory(|memory| memory.allows_interaction(target.layer))
                })
                .cloned()
                .collect();
            if egui::Popup::is_any_open(ctx)
                && let Some(layer) =
                    ctx.memory(|memory| memory.areas().top_layer_id(egui::Order::Foreground))
            {
                targets.retain(|target| target.layer == layer);
            }
            let mode = std::mem::take(&mut window.mode);
            window.mode = match mode {
                Mode::Arm {
                    search,
                    skip: true,
                    grace,
                } => Mode::Arm {
                    search,
                    skip: false,
                    grace,
                },
                Mode::Arm {
                    search,
                    skip: false,
                    ..
                } if !targets.is_empty() => {
                    window.origin = ctx
                        .memory(|m| m.focused())
                        .filter(|id| targets.iter().any(|t| t.widget == *id));
                    if search {
                        Mode::Search {
                            targets: targets.clone(),
                            query: String::new(),
                            selected: 0,
                        }
                    } else {
                        hints(&targets)
                    }
                }
                Mode::Arm {
                    search,
                    skip: false,
                    grace,
                } if grace > 0 => {
                    // egui retains a closing modal's interaction barrier across
                    // its closing/reflow passes. Wait for accepted layout only,
                    // with a finite budget for hosts that remain disabled.
                    Mode::Arm {
                        search,
                        skip: false,
                        grace: grace - 1,
                    }
                }
                Mode::Arm { skip: false, .. } => {
                    // A disabled/empty host cannot acknowledge a discovery
                    // target. Stop repainting and discard its queued input.
                    window.held.clear();
                    window.notice =
                        Some("No keyboard controls are available in this window.".into());
                    ctx.request_repaint();
                    Mode::Idle
                }
                Mode::Hints { choices, prefix } => {
                    // Keep codes attached to their original controls while
                    // following accepted geometry and dropping hidden targets.
                    if choices.iter().any(|choice| {
                        targets
                            .iter()
                            .find(|target| target.key == choice.target.key)
                            .is_none_or(|target| target.rect != choice.target.rect)
                    }) {
                        ctx.request_repaint();
                    }
                    let choices: Vec<_> = choices
                        .into_iter()
                        .filter_map(|choice| {
                            targets
                                .iter()
                                .find(|t| t.key == choice.target.key)
                                .map(|t| Choice {
                                    code: choice.code,
                                    target: t.clone(),
                                })
                        })
                        .collect();
                    if choices.is_empty() {
                        window.held.clear();
                        Mode::Idle
                    } else {
                        Mode::Hints { choices, prefix }
                    }
                }
                Mode::Focus {
                    target,
                    applied,
                    grace,
                } => {
                    if let Some(current) = targets.iter().find(|t| t.key == target.key) {
                        if let Some(group) = current.group {
                            window.groups.insert(group, current.member);
                        }
                        ctx.memory_mut(|m| m.request_focus(current.widget));
                        Mode::Focus {
                            target: current.clone(),
                            applied: true,
                            grace,
                        }
                    } else if !applied && grace > 0 {
                        ctx.request_repaint();
                        Mode::Focus {
                            target,
                            applied,
                            grace: grace - 1,
                        }
                    } else {
                        window.held.clear();
                        Mode::Idle
                    }
                }
                other => other,
            };
            window.published = Some(Snapshot { scope, targets });
            if matches!(window.mode, Mode::Focus { .. } | Mode::Arm { .. }) || window.deferred_ready
            {
                ctx.request_repaint();
            }
        }),
    );
}

/// Register an explicit production label. Widget IDs remain native; semantic keys
/// additionally bind the label and parent context so changing rows cannot reuse a hint.
pub(crate) fn target(ui: &egui::Ui, name: &str, response: &Response) {
    target_kind(ui, name, response, InputKind::Control);
}
pub(crate) fn editor_target(ui: &egui::Ui, name: &str, response: &Response) {
    target_kind(ui, name, response, InputKind::Editor);
}
pub(crate) fn field_target(ui: &egui::Ui, name: &str, response: &Response) {
    target_kind(ui, name, response, InputKind::Field);
}
fn target_kind(ui: &egui::Ui, name: &str, response: &Response, input: InputKind) {
    if name.trim().is_empty() || ui.is_sizing_pass() {
        return;
    }
    let Some(catalog) = catalog(ui.ctx()) else {
        return;
    };
    let rect = response.interact_rect.intersect(ui.clip_rect());
    if !rect.is_positive() || !rect.is_finite() {
        return;
    }
    let target = Target {
        key: ui.id().with((name, response.id)),
        widget: response.id,
        label: name.chars().take(160).collect(),
        rect,
        layer: response.layer_id,
        enabled: response.enabled(),
        group: None,
        member: ui.id().with(name),
        input,
    };
    let mut windows = catalog.0.lock().unwrap();
    let window = windows.entry(ui.ctx().viewport_id()).or_default();
    let group = window
        .group
        .filter(|group| group.layer == response.layer_id);
    if !response.sense.is_focusable() && !(group.is_some() && response.sense.senses_click()) {
        return;
    }
    let mut target = target;
    target.group = group.map(|group| group.id);
    if let Some(group) = target.group
        && (response.has_focus() || response.clicked())
    {
        window.groups.insert(group, target.member);
    }
    if let Some(old) = window.draft.iter_mut().find(|t| t.widget == target.widget) {
        *old = target;
    } else {
        window.draft.push(target);
    }
}

fn hints(targets: &[Target]) -> Mode {
    const ALPHABET: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";
    let mut width = 1;
    let mut capacity = ALPHABET.len();
    while capacity < targets.len() {
        width += 1;
        capacity *= ALPHABET.len();
    }
    let mut ordered = targets.to_vec();
    ordered.sort_by(|a, b| {
        a.rect
            .top()
            .total_cmp(&b.rect.top())
            .then(a.rect.left().total_cmp(&b.rect.left()))
    });
    let choices = ordered
        .into_iter()
        .enumerate()
        .map(|(mut n, target)| {
            let mut code = vec![b'a'; width];
            for digit in code.iter_mut().rev() {
                *digit = ALPHABET[n % ALPHABET.len()];
                n /= ALPHABET.len();
            }
            Choice {
                code: String::from_utf8(code).unwrap(),
                target,
            }
        })
        .collect();
    Mode::Hints {
        choices,
        prefix: String::new(),
    }
}

/// Open an existing contextual menu with Shift+F10 at the focused control.
/// Pointer invocation retains egui's native context-menu behavior.
pub(crate) fn context_menu<R>(
    response: &Response,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<egui::InnerResponse<R>> {
    context_menu_for(response, response.has_focus(), add)
}
pub(super) fn context_menu_for<R>(
    response: &Response,
    focused: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<egui::InnerResponse<R>> {
    let keyboard = focused
        && response.enabled()
        && response
            .ctx
            .input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::F10));
    let anchor = response.id.with("keyboard-context-menu");
    let popup_id = egui::Popup::default_response_id(response);
    if keyboard {
        response.ctx.data_mut(|data| data.insert_temp(anchor, true));
    } else if response.secondary_clicked() || !egui::Popup::is_id_open(&response.ctx, popup_id) {
        response.ctx.data_mut(|data| data.remove::<bool>(anchor));
    }
    let mut popup = egui::Popup::context_menu(response);
    if response.ctx.data(|data| data.get_temp::<bool>(anchor)) == Some(true) {
        // Retain the widget anchor for the entire keyboard-opened menu lifetime;
        // native pointer invocation continues to use its fixed pointer anchor.
        popup = popup.at_position(response.rect.left_bottom());
    }
    if keyboard {
        popup
            .open_memory(Some(egui::SetOpenCommand::Bool(true)))
            .show(add)
    } else {
        popup.show(add)
    }
}

fn keyboard_event(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { .. }
            | egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Ime(_)
    )
}

/// Enter discovery on the current native host. If a palette just closed, wait
/// for its originating UI to publish an interactive layout before collecting targets.
pub(super) fn start(ctx: &egui::Context, search: bool) {
    let Some(catalog) = catalog(ctx) else {
        return;
    };
    let mut windows = catalog.0.lock().unwrap();
    let window = windows.entry(ctx.viewport_id()).or_default();
    window.origin = ctx.memory(|m| m.focused());
    window.mode = match &window.published {
        Some(snapshot) if Some(snapshot.scope) == window.scope && !snapshot.targets.is_empty() => {
            if search {
                Mode::Search {
                    targets: snapshot.targets.clone(),
                    query: String::new(),
                    selected: 0,
                }
            } else {
                hints(&snapshot.targets)
            }
        }
        _ => Mode::Arm {
            search,
            skip: true,
            grace: 2,
        },
    };
    ctx.request_repaint();
}

/// Bind discovery to the actual workspace/document/modal identity. Other native
/// viewports cannot consume this host's mode or buffered input.
pub(super) fn prepare(ctx: &egui::Context, scope: Id) {
    let Some(catalog) = catalog(ctx) else {
        return;
    };
    let mut windows = catalog.0.lock().unwrap();
    let window = windows.entry(ctx.viewport_id()).or_default();
    if window.scope != Some(scope) || !ctx.input(|i| i.focused) {
        if !matches!(window.mode, Mode::Idle) || !window.held.is_empty() {
            ctx.input_mut(|i| i.events.retain(|event| !keyboard_event(event)));
        }
        window.mode = Mode::Idle;
        window.held.clear();
        window.deferred.clear();
    }
    window.scope = Some(scope);
    if !window.deferred_ready && !window.deferred.is_empty() {
        ctx.input_mut(|input| {
            let (keys, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut input.events)
                .into_iter()
                .partition(keyboard_event);
            input.events = rest;
            window.deferred.extend(keys);
        });
        if window.deferred.len() > MAX_EVENTS
            || window.deferred.iter().map(event_bytes).sum::<usize>() > MAX_EVENT_BYTES
        {
            window.deferred.clear();
            window.notice = Some(
                "Keyboard navigation input exceeded its limit; the pending input was discarded."
                    .into(),
            );
        }
        ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    }
    if window.deferred_ready {
        let mut events = std::mem::take(&mut window.deferred);
        ctx.input_mut(|i| {
            events.append(&mut i.events);
            i.events = events;
        });
        window.deferred_ready = false;
    }
}

/// Native form input preceding a navigation prefix must render before discovery
/// changes focus. Source input uses the existing document acknowledgement fence.
pub(super) fn fence(ctx: &egui::Context, is_start: impl Fn(&egui::Event) -> bool) {
    let Some(catalog) = catalog(ctx) else {
        return;
    };
    let mut windows = catalog.0.lock().unwrap();
    let window = windows.entry(ctx.viewport_id()).or_default();
    if !matches!(window.mode, Mode::Idle) {
        return;
    }
    ctx.input_mut(|i| {
        if let Some(index) = i.events.iter().position(is_start)
            && i.events[..index].iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Key { pressed: true, .. }
                )
            })
        {
            let events = i.events.split_off(index);
            if events.len() <= MAX_EVENTS && events.iter().map(event_bytes).sum::<usize>() <= MAX_EVENT_BYTES {
                window.deferred.extend(events);
            } else {
                window.deferred.clear();
                window.notice = Some("Keyboard navigation input exceeded its limit; the pending input was discarded.".into());
            }
        }
    });
}

/// Capture discovery input before native widgets. Return true while those widgets
/// must not see keyboard events. Later input resumes only after focus acknowledgement.
pub(super) fn capture(ctx: &egui::Context) -> bool {
    resume(ctx);
    let Some(catalog) = catalog(ctx) else {
        return false;
    };
    let mut windows = catalog.0.lock().unwrap();
    let window = windows.entry(ctx.viewport_id()).or_default();
    if matches!(window.mode, Mode::Idle) {
        return false;
    }
    // egui computes directional focus before app routing. Discovery owns these
    // keys until it hands input to a destination, so prevent background movement.
    ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    if matches!(window.mode, Mode::Search { .. }) {
        ctx.memory_mut(|m| {
            m.set_modal_layer(egui::LayerId::new(
                egui::Order::Tooltip,
                Id::new("keyboard-control-search"),
            ))
        });
    }
    if !matches!(window.mode, Mode::Search { .. }) && ctx.input(|i| i.pointer.any_pressed()) {
        window.mode = Mode::Idle;
        window.held.clear();
        return false;
    }
    let events = ctx.input_mut(|i| {
        if matches!(window.mode, Mode::Hints { .. }) && !window.held.is_empty() {
            let mut held = std::mem::take(&mut window.held);
            held.append(&mut i.events);
            i.events = held;
        }
        let (keys, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut i.events)
            .into_iter()
            .partition(keyboard_event);
        i.events = rest;
        keys
    });
    if !events.is_empty() {
        ctx.request_repaint();
    }
    for event in events {
        if !matches!(window.mode, Mode::Search { .. })
            && matches!(
                event,
                egui::Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    ..
                }
            )
        {
            window.held.clear();
            window.mode = window.restore_origin(0);
            if let Some(origin) = window.origin {
                ctx.memory_mut(|m| m.request_focus(origin));
            }
            continue;
        }
        match &mut window.mode {
            Mode::Hints { choices, prefix } => match event {
                egui::Event::Key {
                    key: Key::Backspace,
                    pressed: true,
                    ..
                } => {
                    prefix.pop();
                }
                egui::Event::Text(text) => {
                    let mut selected = None;
                    let mut tail = String::new();
                    for original in text.chars() {
                        let c = original.to_ascii_lowercase();
                        if selected.is_some() {
                            tail.push(original);
                            continue;
                        }
                        prefix.push(c);
                        if let Some(choice) = choices.iter().find(|c| c.code == *prefix) {
                            selected = Some(choice.target.clone());
                        } else if !choices.iter().any(|c| c.code.starts_with(prefix.as_str())) {
                            prefix.clear();
                        }
                    }
                    if let Some(target) = selected {
                        window.mode = Mode::Focus {
                            target,
                            applied: false,
                            grace: 0,
                        };
                        if !tail.is_empty() {
                            window.hold(egui::Event::Text(tail));
                        }
                    }
                }
                _ => {}
            },
            Mode::Search { .. } | Mode::Arm { .. } | Mode::Focus { .. } => {
                window.hold(event);
            }
            Mode::Idle => {} // Cancellation never replays hint input.
        }
    }
    true
}

/// Release a completed focus handoff before the source router fences native
/// events. This preserves edit-before-Save ordering even for one batched packet.
pub(super) fn resume(ctx: &egui::Context) {
    let Some(catalog) = catalog(ctx) else {
        return;
    };
    let mut windows = catalog.0.lock().unwrap();
    let Some(window) = windows.get_mut(&ctx.viewport_id()) else {
        return;
    };
    if let Mode::Focus {
        target,
        applied: true,
        ..
    } = &window.mode
        && !ctx.input(|i| i.pointer.any_pressed())
    {
        ctx.memory_mut(|m| m.request_focus(target.widget));
        let mut held = std::mem::take(&mut window.held);
        for event in &held {
            if let egui::Event::Key {
                key,
                modifiers,
                pressed: true,
                ..
            } = event
            {
                let direction = match key {
                    Key::Tab
                        if !matches!(target.input, InputKind::Editor)
                            && !modifiers.ctrl
                            && !modifiers.alt
                            && !modifiers.command =>
                    {
                        Some(if modifiers.shift {
                            egui::FocusDirection::Previous
                        } else {
                            egui::FocusDirection::Next
                        })
                    }
                    Key::ArrowLeft
                        if target.group.is_none()
                            && matches!(target.input, InputKind::Control)
                            && modifiers.is_none() =>
                    {
                        Some(egui::FocusDirection::Left)
                    }
                    Key::ArrowRight
                        if target.group.is_none()
                            && matches!(target.input, InputKind::Control)
                            && modifiers.is_none() =>
                    {
                        Some(egui::FocusDirection::Right)
                    }
                    Key::ArrowUp
                        if target.group.is_none()
                            && matches!(target.input, InputKind::Control)
                            && modifiers.is_none() =>
                    {
                        Some(egui::FocusDirection::Up)
                    }
                    Key::ArrowDown
                        if target.group.is_none()
                            && matches!(target.input, InputKind::Control)
                            && modifiers.is_none() =>
                    {
                        Some(egui::FocusDirection::Down)
                    }
                    _ => None,
                };
                if let Some(direction) = direction {
                    ctx.memory_mut(|m| m.move_focus(direction));
                }
            }
        }
        ctx.input_mut(|i| {
            held.append(&mut i.events);
            i.events = held;
        });
        window.mode = Mode::Idle;
    }
}

/// Stable subsequence ranking shared by command and visible-control discovery.
pub(super) fn rank(query: &str, label: &str) -> Option<usize> {
    let query = query.to_lowercase();
    let label = label.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if query == label {
        return Some(0);
    }
    if let Some(index) = label.find(&query) {
        return Some(1 + index);
    }
    let mut chars = label.chars().enumerate();
    let mut score = label.len();
    let mut previous = 0;
    for wanted in query.chars().filter(|c| !c.is_whitespace()) {
        let (index, _) = chars.find(|(_, c)| *c == wanted)?;
        score += index.saturating_sub(previous);
        previous = index;
    }
    Some(100 + score)
}

/// Read-only fixture projection of names and codes; no field values are exported.
pub(super) fn snapshot(ctx: &egui::Context) -> serde_json::Value {
    let Some(catalog) = catalog(ctx) else {
        return serde_json::Value::Null;
    };
    let windows = catalog.0.lock().unwrap();
    let Some(window) = windows.get(&ctx.viewport_id()) else {
        return serde_json::Value::Null;
    };
    let mode = match window.mode {
        Mode::Idle => "idle",
        Mode::Arm { .. } => "arming",
        Mode::Hints { .. } => "hints",
        Mode::Search { .. } => "search",
        Mode::Focus { .. } => "focus",
    };
    let hints: Vec<_> = match &window.mode {
        Mode::Hints { choices, .. } => choices
            .iter()
            .map(|c| serde_json::json!({"code":c.code,"label":c.target.label}))
            .collect(),
        _ => vec![],
    };
    serde_json::json!({"mode":mode,"hints":hints})
}

/// Release a native host's transient metadata when its window closes.
pub(super) fn forget(ctx: &egui::Context, viewport: egui::ViewportId) {
    if let Some(catalog) = catalog(ctx) {
        catalog.0.lock().unwrap().remove(&viewport);
    }
}
pub(super) fn take_notice(ctx: &egui::Context) -> Option<String> {
    catalog(ctx)?
        .0
        .lock()
        .unwrap()
        .get_mut(&ctx.viewport_id())?
        .notice
        .take()
}

#[cfg(test)]
mod tests;

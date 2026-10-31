//! Bounded, per-action preferences. Scalar keys merge safely between windows.
//!
//! Validate leader/direct binding conflicts as a complete draft before activation.
//! Standard/Emacs/Vim customizations have independent scalar keys, allowing concurrent
//! windows to merge unrelated edits safely.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Action {
    Left,
    Down,
    Up,
    Right,
    Next,
    Previous,
    Editor,
    Explorer,
    Terminal,
    Tools,
    Menus,
    Narrower,
    Wider,
    Shorter,
    Taller,
    Zoom,
    Hide,
    NewTerminal,
    PreviousTerminal,
    NextTerminal,
    Persistence,
    OpenFile,
    OpenFolder,
    Commands,
    Help,
    Save,
    SaveAs,
    CloseFile,
    NextFile,
    PreviousFile,
    Find,
    NextMatch,
    PreviousMatch,
}
pub(super) const ACTIONS: &[(Action, &str, &str, &str)] = &[
    (
        Action::Left,
        "left",
        "Focus pane left",
        "Leader+h, Leader+Left",
    ),
    (
        Action::Down,
        "down",
        "Focus pane below",
        "Leader+j, Leader+Down",
    ),
    (Action::Up, "up", "Focus pane above", "Leader+k, Leader+Up"),
    (
        Action::Right,
        "right",
        "Focus pane right",
        "Leader+l, Leader+Right",
    ),
    (Action::Next, "next", "Next pane", "Leader+Tab"),
    (
        Action::Previous,
        "previous",
        "Previous pane",
        "Leader+Shift+Tab",
    ),
    (Action::Editor, "editor", "Focus editor", "Leader+e"),
    (Action::Explorer, "explorer", "Focus explorer", "Leader+f"),
    (Action::Terminal, "terminal", "Focus terminal", "Leader+t"),
    (Action::Tools, "tools", "Focus project tools", "Leader+p"),
    (Action::Menus, "menus", "Focus application menu", "Leader+u"),
    (
        Action::Narrower,
        "narrower",
        "Shrink explorer / detached tools width",
        "Leader+Shift+H",
    ),
    (
        Action::Wider,
        "wider",
        "Grow explorer / detached tools width",
        "Leader+Shift+L",
    ),
    (
        Action::Shorter,
        "shorter",
        "Shrink detached tools height",
        "Leader+Shift+J",
    ),
    (
        Action::Taller,
        "taller",
        "Grow detached tools height",
        "Leader+Shift+K",
    ),
    (Action::Zoom, "zoom", "Maximize / restore pane", "Leader+m"),
    (Action::Hide, "hide", "Hide focused side pane", "Leader+q"),
    (
        Action::NewTerminal,
        "new_terminal",
        "New terminal",
        "Leader+n",
    ),
    (
        Action::PreviousTerminal,
        "previous_terminal",
        "Previous terminal",
        "Leader+[",
    ),
    (
        Action::NextTerminal,
        "next_terminal",
        "Next terminal",
        "Leader+]",
    ),
    (
        Action::Persistence,
        "persistence",
        "Toggle terminal persistence",
        "Leader+s",
    ),
    (Action::OpenFile, "open_file", "Open file", "Leader+o"),
    (
        Action::OpenFolder,
        "open_folder",
        "Open project folder",
        "Leader+r",
    ),
    (Action::Commands, "commands", "Command palette", "Leader+c"),
    (
        Action::Help,
        "help",
        "Keyboard shortcuts / bindings",
        "Leader+Slash",
    ),
    (Action::Save, "save", "Save document", ""),
    (Action::SaveAs, "save_as", "Save document as", ""),
    (Action::CloseFile, "close_file", "Close document", ""),
    (Action::NextFile, "next_file", "Next document", ""),
    (
        Action::PreviousFile,
        "previous_file",
        "Previous document",
        "",
    ),
    (Action::Find, "find", "Find in document", ""),
    (Action::NextMatch, "next_match", "Next search match", ""),
    (
        Action::PreviousMatch,
        "previous_match",
        "Previous search match",
        "",
    ),
];

const DEFAULT_LEADER: &str = "Ctrl+Space";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Style {
    #[default]
    Standard,
    Emacs,
    Vim,
}
impl Style {
    pub const ALL: [Self; 3] = [Self::Standard, Self::Emacs, Self::Vim];
    pub fn name(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Emacs => "Emacs",
            Self::Vim => "Vim",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Emacs => "emacs",
            Self::Vim => "vim",
        }
    }
    fn key(self, name: &str) -> String {
        if self == Self::Standard {
            format!("keyboard.{name}")
        } else {
            format!("keyboard.{}.{name}", self.id())
        }
    }
    fn leader(self) -> &'static str {
        match self {
            Self::Standard => DEFAULT_LEADER,
            Self::Emacs => "Ctrl+X",
            Self::Vim => "Ctrl+W",
        }
    }
    fn binding(self, action: Action, standard: &'static str) -> &'static str {
        use self::Action::*;
        match (self, action) {
            (Self::Emacs, Left) => "Leader+Left, Leader+Ctrl+B",
            (Self::Emacs, Down) => "Leader+Down, Leader+Ctrl+N",
            (Self::Emacs, Up) => "Leader+Up, Leader+Ctrl+P",
            (Self::Emacs, Right) => "Leader+Right",
            (Self::Emacs, Next) => "Leader+o",
            (Self::Emacs, Previous) => "Leader+Shift+O",
            (Self::Emacs, OpenFile) => "Leader+Ctrl+F",
            (Self::Emacs, OpenFolder) => "Leader+Ctrl+D",
            (Self::Emacs, Commands) => "Leader+x",
            (Self::Emacs, Save) => "Leader+Ctrl+S",
            (Self::Emacs, SaveAs) => "Leader+Ctrl+W",
            (Self::Emacs, CloseFile) => "Leader+k",
            (Self::Emacs, NextFile) => "Leader+b",
            (Self::Emacs, PreviousFile) => "Leader+Shift+B",
            (Self::Vim, Next) => "Leader+w",
            (Self::Vim, Previous) => "Leader+Shift+W",
            (Self::Vim, Zoom) => "Leader+o",
            (Self::Vim, OpenFile) => "Leader+Ctrl+F",
            (Self::Vim, Commands) => "Leader+Shift+P",
            (Self::Vim, Save) => "Leader+Ctrl+S",
            (Self::Vim, SaveAs) => "Leader+Ctrl+Shift+S",
            (Self::Vim, CloseFile) => "Leader+c",
            (Self::Vim, NextFile) => "Leader+Ctrl+N",
            (Self::Vim, PreviousFile) => "Leader+Ctrl+P",
            _ => standard,
        }
    }
}
pub(in crate::app) struct Bindings {
    profiles: [Profile; 3],
    style: Style,
    draft: Style,
    pending: bool,
}
impl std::ops::Deref for Bindings {
    type Target = Profile;
    fn deref(&self) -> &Profile {
        &self.profiles[self.style as usize]
    }
}
impl std::ops::DerefMut for Bindings {
    fn deref_mut(&mut self) -> &mut Profile {
        &mut self.profiles[self.style as usize]
    }
}
impl Bindings {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let style = storage
            .and_then(|s| s.get_string("keyboard.style"))
            .and_then(|id| Style::ALL.into_iter().find(|style| style.id() == id))
            .unwrap_or_default();
        Self {
            profiles: Style::ALL.map(|style| Profile::load(storage, style)),
            style,
            draft: style,
            pending: false,
        }
    }
    pub fn style(&self) -> Style {
        self.style
    }
    pub fn select_style(&mut self, style: Style) {
        self.pending |= self.style != style;
        self.style = style;
        self.draft = style;
    }
    pub fn prefix(&self, terminal: bool) -> Chord {
        if terminal && self.style != Style::Standard {
            Chord {
                key: Key::Space,
                modifiers: egui::Modifiers::CTRL,
                leader: false,
            }
        } else {
            self.leader
        }
    }
    pub fn prefix_label(&self, terminal: bool) -> &str {
        if terminal && self.style != Style::Standard {
            DEFAULT_LEADER
        } else {
            self.hint()
        }
    }
    pub fn store(&mut self, storage: &mut dyn eframe::Storage) {
        for profile in &mut self.profiles {
            profile.store(storage);
        }
        if std::mem::take(&mut self.pending) {
            storage.set_string("keyboard.style", self.style.id().into());
            storage.flush();
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, focus: bool) {
        ui.horizontal(|ui| {
            ui.label("Shortcut style");
            egui::ComboBox::from_id_salt("keyboard-style")
                .selected_text(self.draft.name())
                .show_ui(ui, |ui| {
                    for style in Style::ALL {
                        ui.selectable_value(&mut self.draft, style, style.name());
                    }
                });
        });
        ui.label(match self.draft {Style::Standard=>"Standard editing with configurable pane commands.",Style::Emacs=>"Emacs source editing and Ctrl+X commands. In terminals, use Ctrl+Space for pane commands.",Style::Vim=>"Vim normal/insert/visual editing and Ctrl+W pane commands. In terminals, use Ctrl+Space for pane commands."});
        if self.profiles[self.draft as usize].ui(ui, focus) {
            self.select_style(self.draft);
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct Chord {
    pub key: Key,
    pub modifiers: egui::Modifiers,
    pub leader: bool,
}
impl Chord {
    pub fn matches(&self, key: Key, modifiers: egui::Modifiers, leader: bool) -> bool {
        self.key == key && self.leader == leader && modifiers.matches_exact(self.modifiers)
    }
}
fn parse(text: &str, allow_leader: bool) -> Result<Chord, String> {
    let parts: Vec<_> = text.trim().split('+').map(str::trim).collect();
    let mut modifiers = egui::Modifiers::NONE;
    let mut leader = false;
    let (name, prefix) = parts.split_last().ok_or("Choose a key")?;
    for part in prefix {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" => modifiers.alt = true,
            "cmd" | "command" => modifiers.command = true,
            "leader" if allow_leader && !leader => leader = true,
            _ => return Err(format!("Unknown modifier: {part}")),
        }
    }
    let key = Key::from_name(name)
        .or_else(|| {
            Key::ALL
                .iter()
                .copied()
                .find(|key| key.name().eq_ignore_ascii_case(name))
        })
        .ok_or_else(|| format!("Unknown key: {name}. Use names such as Space, Left or Slash."))?;
    if !leader
        && !modifiers.ctrl
        && !modifiers.command
        && !modifiers.alt
        && !matches!(
            key,
            Key::F1
                | Key::F2
                | Key::F3
                | Key::F4
                | Key::F5
                | Key::F6
                | Key::F7
                | Key::F8
                | Key::F9
                | Key::F10
                | Key::F11
                | Key::F12
        )
    {
        return Err("Direct shortcuts need Ctrl, Cmd or Alt, or a function key. Plain typing keys belong after Leader.".into());
    }
    if leader && key == Key::Escape {
        return Err("Escape is reserved for cancelling navigation".into());
    }
    Ok(Chord {
        key,
        modifiers,
        leader,
    })
}
struct Entry {
    action: Action,
    value: String,
    draft: String,
    chords: Vec<Chord>,
    pending: bool,
    repair: bool,
}
pub(in crate::app) struct Profile {
    style: Style,
    pub leader: Chord,
    leader_value: String,
    leader_draft: String,
    leader_pending: bool,
    leader_repair: bool,
    entries: Vec<Entry>,
    error: String,
}
impl Profile {
    fn load(storage: Option<&dyn eframe::Storage>, style: Style) -> Self {
        let saved_leader = storage.and_then(|s| s.get_string(&style.key("leader")));
        let invalid_leader = saved_leader
            .as_ref()
            .is_some_and(|text| text.len() > 80 || parse(text, false).is_err());
        let leader_value = saved_leader
            .filter(|text| text.len() <= 80 && parse(text, false).is_ok())
            .unwrap_or_else(|| style.leader().into());
        let mut result = Self {
            style,
            leader: parse(&leader_value, false).unwrap(),
            leader_draft: leader_value.clone(),
            leader_value,
            leader_pending: false,
            leader_repair: invalid_leader,
            entries: ACTIONS
                .iter()
                .map(|&(action, id, _, default)| {
                    let saved = storage.and_then(|s| s.get_string(&style.key(id)));
                    let oversized = saved.as_ref().is_some_and(|text| text.len() > 256);
                    let value = saved
                        .filter(|text| text.len() <= 256)
                        .unwrap_or_else(|| style.binding(action, default).into());
                    Entry {
                        action,
                        draft: value.clone(),
                        value,
                        chords: vec![],
                        pending: false,
                        repair: oversized,
                    }
                })
                .collect(),
            error: if invalid_leader {
                "Saved leader was invalid; the default leader is active. Apply to save the corrected binding.".into()
            } else {
                String::new()
            },
        };
        if let Err(error) = result.compile() {
            // Invalid saved data must not trap keyboard users. Keep the error
            // visible in settings, with the documented safe defaults active.
            result.error = format!("Saved bindings were invalid ({error}); defaults are active.");
            result.leader_repair |= result.leader_value != style.leader();
            result.leader_value = style.leader().into();
            result.leader_draft.clone_from(&result.leader_value);
            for (entry, spec) in result.entries.iter_mut().zip(ACTIONS) {
                let default = style.binding(entry.action, spec.3);
                entry.repair |= entry.value != default;
                entry.value = default.into();
                entry.draft.clone_from(&entry.value);
            }
            result.compile().expect("valid default navigation bindings");
        }
        result
    }
    fn compile(&mut self) -> Result<(), String> {
        if self.leader_draft.len() > 80 {
            return Err("Leader exceeds 80 characters".into());
        }
        let leader = parse(&self.leader_draft, false)?;
        let mut used = vec![leader];
        let mut compiled = vec![];
        for (entry, spec) in self.entries.iter().zip(ACTIONS) {
            if entry.draft.len() > 256 {
                return Err("A binding exceeds 256 characters".into());
            }
            let mut chords = vec![];
            for text in entry
                .draft
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if chords.len() >= 4 {
                    return Err("Use at most four alternatives per action".into());
                }
                let chord = parse(text, true)?;
                let collision = used.iter().any(|old| {
                    old.key == chord.key
                        && old.leader == chord.leader
                        && (0..32).any(|bits| {
                            let modifiers = egui::Modifiers {
                                alt: bits & 1 != 0,
                                ctrl: bits & 2 != 0,
                                shift: bits & 4 != 0,
                                mac_cmd: bits & 8 != 0,
                                command: bits & 16 != 0,
                            };
                            modifiers.matches_exact(old.modifiers)
                                && modifiers.matches_exact(chord.modifiers)
                        })
                });
                if collision {
                    return Err(format!("Duplicate shortcut {text} ({})", spec.2));
                }
                used.push(chord);
                chords.push(chord);
            }
            compiled.push(chords);
        }
        self.leader = leader;
        for (entry, chords) in self.entries.iter_mut().zip(compiled) {
            entry.chords = chords;
        }
        Ok(())
    }
    pub fn action(&self, key: Key, modifiers: egui::Modifiers, leader: bool) -> Option<Action> {
        self.entries
            .iter()
            .find(|entry| {
                entry
                    .chords
                    .iter()
                    .any(|chord| chord.matches(key, modifiers, leader))
            })
            .map(|entry| entry.action)
    }
    pub fn hint(&self) -> &str {
        &self.leader_value
    }
    pub fn commands(&self) -> impl Iterator<Item = (Action, &str, &str)> {
        self.entries
            .iter()
            .zip(ACTIONS)
            .map(|(entry, spec)| (entry.action, spec.2, entry.value.as_str()))
    }
    #[cfg(test)]
    pub fn help(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .zip(ACTIONS)
            .filter(|(entry, _)| !entry.value.is_empty())
            .map(|(entry, spec)| (spec.2, entry.value.as_str()))
    }
    pub fn store(&mut self, storage: &mut dyn eframe::Storage) {
        let mut changed = false;
        if self.leader_pending {
            storage.set_string(&self.style.key("leader"), self.leader_value.clone());
            self.leader_pending = false;
            changed = true;
        }
        for (entry, spec) in self.entries.iter_mut().zip(ACTIONS) {
            if entry.pending {
                storage.set_string(&self.style.key(spec.1), entry.value.clone());
                entry.pending = false;
                changed = true;
            }
        }
        if changed {
            storage.flush();
        }
    }
    fn apply(&mut self) -> Result<(), String> {
        self.compile()?;
        self.leader_pending |= self.leader_repair || self.leader_value != self.leader_draft;
        self.leader_repair = false;
        self.leader_value.clone_from(&self.leader_draft);
        for entry in &mut self.entries {
            entry.pending |= entry.repair || entry.value != entry.draft;
            entry.repair = false;
            entry.value.clone_from(&entry.draft);
        }
        Ok(())
    }
    fn ui(&mut self, ui: &mut egui::Ui, focus: bool) -> bool {
        let mut applied = false;
        let apply_key = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter));
        ui.label("Navigation bindings use no Alt keys by default. Existing editor shortcuts and shell keys stay available.");
        ui.label("Use Leader+h, direct Ctrl+Shift+Left, or comma-separated alternatives. Empty fields disable an action. Release the leader modifiers before its next key.");
        ui.horizontal(|ui| {
            ui.label("Leader key");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.leader_draft)
                    .char_limit(80)
                    .desired_width(240.),
            );
            if focus {
                response.request_focus();
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("navigation-bindings")
            .auto_shrink([false, false])
            .max_height((ui.ctx().content_rect().height() - 470.).clamp(90., 300.))
            .show(ui, |ui| {
                let label_width = (ui.available_width() * 0.42).clamp(150., 230.);
                for (index, (entry, spec)) in self.entries.iter_mut().zip(ACTIONS).enumerate() {
                    let fill = if index % 2 == 0 {
                        ui.visuals().faint_bg_color
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    egui::Frame::new().fill(fill).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [label_width, 24.],
                                egui::Label::new(spec.2).truncate().halign(Align::Min),
                            )
                            .on_hover_text(spec.2);
                            let response = ui.add_sized(
                                [ui.available_width(), 24.],
                                egui::TextEdit::singleline(&mut entry.draft).char_limit(256),
                            );
                            if response.gained_focus() {
                                response.scroll_to_me(Some(Align::Center));
                            }
                        });
                    });
                }
            });
        if !self.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &self.error);
        }
        ui.horizontal(|ui| {
            if ui.button("Apply bindings · Ctrl+Enter").clicked() || apply_key {
                match self.apply() {
                    Ok(()) => {
                        self.error.clear();
                        applied = true;
                    }
                    Err(error) => self.error = error,
                }
            }
            if ui.button("Restore preset bindings").clicked() {
                self.leader_draft = self.style.leader().into();
                for (entry, spec) in self.entries.iter_mut().zip(ACTIONS) {
                    entry.draft = self.style.binding(entry.action, spec.3).into();
                }
                self.error.clear();
            }
        });
        ui.small("Apply saves the style and changed bindings immediately. Other windows load changes on restart. Direct bindings override existing shortcuts; window-manager grabs take priority.");
        applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    pub(super) struct Storage(pub(super) std::collections::BTreeMap<String, String>);
    impl eframe::Storage for Storage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.into(), value);
        }
        fn flush(&mut self) {}
    }
    #[test]
    fn invalid_or_colliding_edits_leave_active_bindings_and_storage_unchanged() {
        let mut bindings = Profile::load(None, Style::Standard);
        let mut storage = Storage::default();
        for invalid in ["h", "Leader+Escape", "Leader+j", DEFAULT_LEADER] {
            bindings.entries[0].draft = invalid.into();
            assert!(bindings.apply().is_err(), "{invalid}");
            assert_eq!(
                bindings.action(Key::H, egui::Modifiers::NONE, true),
                Some(Action::Left)
            );
            bindings.store(&mut storage);
            assert!(storage.0.is_empty());
        }
    }
    #[test]
    fn remapping_and_disabling_save_only_changed_fields_and_survive_reload() {
        let mut bindings = Profile::load(None, Style::Standard);
        bindings.leader_draft = "Ctrl+Shift+F12".into();
        bindings.entries[0].draft = "Ctrl+Shift+Left".into();
        bindings.entries[1].draft.clear();
        bindings.apply().unwrap();
        let mut storage = Storage::default();
        bindings.store(&mut storage);
        assert_eq!(storage.0.len(), 3);
        let loaded = Profile::load(Some(&storage), Style::Standard);
        assert_eq!(loaded.hint(), "Ctrl+Shift+F12");
        assert_eq!(
            loaded.action(
                Key::ArrowLeft,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                false
            ),
            Some(Action::Left)
        );
        assert_eq!(loaded.action(Key::J, egui::Modifiers::NONE, true), None);
        storage.0.clear();
        bindings.store(&mut storage);
        assert!(storage.0.is_empty());
    }
    #[test]
    fn malformed_saved_shortcuts_recover_to_accessible_defaults_without_overwriting_them() {
        let mut storage = Storage::default();
        storage.0.insert("keyboard.left".into(), "Typo+h".into());
        let mut bindings = Profile::load(Some(&storage), Style::Standard);
        assert!(bindings.error.contains("invalid"));
        assert_eq!(
            bindings.action(Key::H, egui::Modifiers::NONE, true),
            Some(Action::Left)
        );
        bindings.store(&mut storage);
        assert_eq!(storage.0["keyboard.left"], "Typo+h");
        bindings.apply().unwrap();
        bindings.store(&mut storage);
        assert_eq!(storage.0.len(), 1);
        assert_eq!(storage.0["keyboard.left"], ACTIONS[0].3);
    }
    #[test]
    fn concurrent_windows_preserve_edits_to_different_bindings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.ron");
        let open = || crate::persistence::Preferences::open(path.clone(), Arc::new(|| {}));
        let mut a = open();
        let mut b = open();
        let mut first = Profile::load(Some(&a), Style::Standard);
        let mut second = Profile::load(Some(&b), Style::Standard);
        first.entries[0].draft = "Ctrl+Shift+Left".into();
        first.apply().unwrap();
        first.store(&mut a);
        drop(a);
        second.entries[1].draft = "Ctrl+Shift+Down".into();
        second.apply().unwrap();
        second.store(&mut b);
        drop(b);
        let saved = open();
        let bindings = Profile::load(Some(&saved), Style::Standard);
        assert_eq!(
            bindings.action(
                Key::ArrowLeft,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                false
            ),
            Some(Action::Left)
        );
        assert_eq!(
            bindings.action(
                Key::ArrowDown,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                false
            ),
            Some(Action::Down)
        );
    }
}

#[cfg(test)]
mod preset_tests {
    use super::tests::Storage;
    use super::*;
    #[test]
    fn presets_are_valid_and_customizations_are_scoped_to_their_style() {
        let mut bindings = Bindings::load(None);
        assert_eq!(bindings.style(), Style::Standard);
        bindings.profiles[0].leader_draft = "Ctrl+Shift+F12".into();
        bindings.profiles[0].apply().unwrap();
        bindings.select_style(Style::Emacs);
        assert_eq!(bindings.hint(), "Ctrl+X");
        assert_eq!(
            bindings.action(Key::S, egui::Modifiers::CTRL, true),
            Some(Action::Save)
        );
        bindings.profiles[1].leader_draft = "F12".into();
        bindings.profiles[1].apply().unwrap();
        let mut storage = Storage::default();
        bindings.store(&mut storage);
        let mut loaded = Bindings::load(Some(&storage));
        assert_eq!(loaded.style(), Style::Emacs);
        assert_eq!(loaded.hint(), "F12");
        loaded.select_style(Style::Vim);
        assert_eq!(loaded.hint(), "Ctrl+W");
        loaded.select_style(Style::Standard);
        assert_eq!(loaded.hint(), "Ctrl+Shift+F12");
        assert_eq!(storage.0["keyboard.emacs.leader"], "F12");
    }
    #[test]
    fn terminal_prefix_preserves_readline_control_x_and_control_w() {
        for style in [Style::Emacs, Style::Vim] {
            let mut bindings = Bindings::load(None);
            bindings.select_style(style);
            assert!(
                bindings
                    .prefix(true)
                    .matches(Key::Space, egui::Modifiers::CTRL, false)
            );
            assert_ne!(bindings.prefix(true).key, bindings.prefix(false).key);
        }
    }
}

//! Rendering borrows inspection data. Mutations and protocol requests are
//! returned to the controller, after all UI borrows have ended.
use super::*;
use std::collections::BTreeSet;

pub(super) enum Action {
    Start,
    Stop,
    Control(&'static str),
    ToggleBreakpoint,
    Thread(i64),
    Frame(i64),
    Evaluate(String),
}
#[derive(Default)]
pub(super) struct Actions {
    pub commands: Vec<Action>,
    pub variables: BTreeSet<i64>,
    pub settings_changed: bool,
    pub config_saved: bool,
    pub watches_changed: bool,
    pub breakpoints: BTreeSet<String>,
    pub errors: Vec<String>,
}
pub(super) struct Context<'a> {
    pub host: &'a str,
    pub ready: bool,
    pub sandbox: bool,
    pub palette: theme::Palette,
}
pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    settings: &mut debugger::Settings,
    view: Context<'_>,
) -> Actions {
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    let mut actions = Actions::default();
    ui.add(egui::Label::new(view.palette.tag(view.host)).truncate())
        .on_hover_text(view.host);
    let phase = state.client.as_ref().map_or(Phase::Ended, |c| c.phase);
    let live = phase != Phase::Ended;
    let collapse_config = live && !state.was_live;
    state.was_live = live;
    controls(ui, phase, view.ready, &mut actions);
    if let Some(client) = &state.client {
        ui.label(&client.status);
        if !client.error.is_empty() {
            ui.colored_label(view.palette.error, &client.error);
        }
    }
    if !view.ready {
        ui.label("Open a connected project to debug its programs.");
    }
    egui::ScrollArea::vertical()
        .id_salt("debug-content")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            configuration_ui(
                ui,
                &mut state.config,
                settings,
                !live,
                collapse_config,
                view.sandbox,
                &mut actions,
            );
            if let Some(client) = &state.client {
                stack_ui(ui, client, &mut actions);
                scopes_ui(ui, client, &mut actions);
            }
            watches_ui(
                ui,
                &mut state.watch,
                settings,
                state.client.as_ref(),
                view.palette,
                &mut actions,
            );
            breakpoints_ui(ui, settings, state.client.as_ref(), &mut actions);
            if let Some(client) = &state.client {
                exceptions_ui(ui, settings, client, &mut actions);
                console_ui(
                    ui,
                    client,
                    &mut state.expression,
                    view.palette,
                    &mut actions,
                );
            }
        });
    actions
}
fn controls(ui: &mut egui::Ui, phase: Phase, ready: bool, actions: &mut Actions) {
    let paused = phase == Phase::Paused;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                ready && phase == Phase::Ended,
                IconButton::icon(Icon::Debug, "Start debugging · F5"),
            )
            .clicked()
        {
            actions.commands.push(Action::Start);
        }
        for (icon, command, label, enabled) in [
            (Icon::Play, "continue", "Continue · F5", paused),
            (Icon::Pause, "pause", "Pause · F6", phase == Phase::Running),
            (Icon::StepOver, "next", "Step over · F10", paused),
            (Icon::StepIn, "stepIn", "Step into · F11", paused),
            (Icon::StepOut, "stepOut", "Step out · Shift+F11", paused),
        ] {
            if ui
                .add_enabled(enabled, IconButton::icon(icon, label))
                .clicked()
            {
                actions.commands.push(Action::Control(command));
            }
        }
        if ui
            .add_enabled(
                phase != Phase::Ended,
                IconButton::icon(
                    Icon::Stop,
                    "Stop launched program; detach attached process · Shift+F5",
                ),
            )
            .clicked()
        {
            actions.commands.push(Action::Stop);
        }
    });
}
fn configuration_ui(
    ui: &mut egui::Ui,
    draft: &mut configuration::Draft,
    settings: &mut debugger::Settings,
    editable: bool,
    collapse: bool,
    sandbox: bool,
    actions: &mut Actions,
) {
    egui::CollapsingHeader::new("Launch / attach configuration")
        .default_open(editable).open(if collapse{Some(false)}else{None})
        .show(ui,|ui|{
            ui.small("Installed project-host adapters: Python (debugpy), C/C++/Rust (lldb-dap), or custom stdio DAP. No automatic tool installation.");
            ui.add_enabled_ui(editable,|ui|{
                if let Some(config)=&mut draft.parsed {
                    let mut changed=false;
                    egui::ComboBox::from_id_salt("debug-backend").selected_text(config.backend.label()).show_ui(ui,|ui|{
                        for backend in [Backend::Python,Backend::Lldb,Backend::Custom]{changed|=ui.selectable_value(&mut config.backend,backend,backend.label()).changed();}
                    });
                    ui.label("Adapter executable (blank: preset default)");
                    changed|=ui.add(egui::TextEdit::singleline(&mut config.adapter).desired_width(f32::INFINITY).char_limit(4096)).changed();
                    ui.label("Program (native code: compiled executable)");
                    changed|=ui.add(egui::TextEdit::singleline(&mut config.program).desired_width(f32::INFINITY).char_limit(4096)).changed();
                    ui.horizontal(|ui|{
                        changed|=ui.checkbox(&mut config.stop_on_entry,"Stop on entry").changed();
                        changed|=ui.checkbox(&mut config.attach,"Attach").changed();
                    });
                    if changed{draft.reformat();}
                }
                egui::CollapsingHeader::new("Arguments, adapter and advanced JSON").show(ui,|ui|{
                    ui.small("Arguments are arrays of literal strings. Placeholders: {root}, {file}, {dir}, {stem}, {python}. Extra DAP arguments override presets. Attach needs backend-specific extra arguments (e.g. processId).");
                    if ui.add(egui::TextEdit::multiline(&mut draft.text).code_editor().desired_rows(12).desired_width(f32::INFINITY).char_limit(64*1024)).changed(){draft.parse();}
                });
                if ui.button("Save configuration").clicked(){
                    match draft.validated(){
                        Ok(config)=>{settings.config=config.clone();actions.settings_changed=true;actions.config_saved=true;}
                        Err(error)=>actions.errors.push(error.to_string()),
                    }
                }
            });
            if sandbox{ui.small("Programs launch inside this sandbox. Attach to an external process requires normal mode.");}
        });
}
fn stack_ui(ui: &mut egui::Ui, client: &Client, actions: &mut Actions) {
    egui::CollapsingHeader::new("Threads & call stack")
        .default_open(true)
        .show(ui, |ui| {
            ui.add_enabled_ui(client.phase == Phase::Paused, |ui| {
                for thread in &client.threads {
                    if let Some(id) = thread["id"].as_i64()
                        && ui
                            .selectable_label(
                                client.thread == Some(id),
                                thread["name"].as_str().unwrap_or("Thread"),
                            )
                            .clicked()
                    {
                        actions.commands.push(Action::Thread(id));
                    }
                }
                for frame in &client.frames {
                    if let Some(id) = frame["id"].as_i64() {
                        let label = format!(
                            "{} :{}",
                            frame["name"].as_str().unwrap_or("Frame"),
                            frame["line"]
                        );
                        if ui
                            .add(
                                egui::Button::selectable(
                                    client.frame == Some(id),
                                    egui::RichText::new(label).monospace(),
                                )
                                .truncate(),
                            )
                            .on_hover_text(
                                frame["source"]["path"]
                                    .as_str()
                                    .unwrap_or("Source unavailable"),
                            )
                            .clicked()
                        {
                            actions.commands.push(Action::Frame(id));
                        }
                    }
                }
            });
        });
}
fn scopes_ui(ui: &mut egui::Ui, client: &Client, actions: &mut Actions) {
    egui::CollapsingHeader::new("Variables")
        .default_open(true)
        .show(ui, |ui| {
            for scope in &client.scopes {
                if let Some(id) = scope["variablesReference"].as_i64() {
                    egui::CollapsingHeader::new(scope["name"].as_str().unwrap_or("Scope"))
                        .id_salt(("debug-scope", id))
                        .default_open(scope["expensive"] != true)
                        .show(ui, |ui| variables_ui(ui, client, id, 0, actions));
                }
            }
            if client.phase != Phase::Paused {
                ui.small("Pause at a breakpoint to inspect variables.");
            }
        });
}
fn variables_ui(ui: &mut egui::Ui, client: &Client, id: i64, depth: usize, actions: &mut Actions) {
    if depth >= 8 {
        ui.small("Expansion depth limit");
        return;
    }
    let Some(values) = client.variables.get(&id) else {
        if id > 0 {
            actions.variables.insert(id);
            ui.small("Loading…");
        }
        return;
    };
    for (index, value) in values.iter().enumerate() {
        let label = format!(
            "{} = {}",
            value["name"].as_str().unwrap_or("?"),
            value["value"].as_str().unwrap_or("")
        );
        let reference = value["variablesReference"].as_i64().unwrap_or(0);
        if reference > 0 {
            egui::CollapsingHeader::new(egui::RichText::new(&label).monospace())
                .id_salt(("debug-var", id, index))
                .show(ui, |ui| {
                    variables_ui(ui, client, reference, depth + 1, actions)
                });
        } else {
            ui.add(egui::Label::new(egui::RichText::new(&label).monospace()).truncate())
                .on_hover_ui(|ui| {
                    ui.label(value["type"].as_str().unwrap_or(""));
                    ui.monospace(&label);
                });
        }
    }
    if values.len() >= 200 {
        ui.small("First 200 children. Evaluate a narrower expression for more.");
    }
}
fn watches_ui(
    ui: &mut egui::Ui,
    expression: &mut String,
    settings: &mut debugger::Settings,
    client: Option<&Client>,
    palette: theme::Palette,
    actions: &mut Actions,
) {
    egui::CollapsingHeader::new("Watches")
        .default_open(true)
        .show(ui, |ui| {
            let mut remove = None;
            for (index, watch) in settings.watches.iter().enumerate() {
                ui.horizontal(|ui| {
                    if icons::small(ui, Icon::Close, "Remove watch").clicked() {
                        remove = Some(index);
                    }
                    ui.add(egui::Label::new(egui::RichText::new(watch).monospace()).truncate())
                        .on_hover_text(watch);
                });
                if let Some(result) = client.and_then(|c| c.watch_values.get(index)) {
                    ui.add(
                        egui::Label::new(egui::RichText::new(result).color(palette.accent))
                            .truncate(),
                    )
                    .on_hover_text(result);
                }
            }
            if let Some(index) = remove {
                settings.watches.remove(index);
                actions.settings_changed = true;
                actions.watches_changed = true;
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(expression)
                        .hint_text("Watch expression")
                        .char_limit(4096)
                        .desired_width((ui.available_width() - 72.).max(60.)),
                );
                if ui
                    .add_sized([48., 24.], egui::Button::new("Add").truncate())
                    .clicked()
                    && !expression.trim().is_empty()
                    && settings.watches.len() < 32
                {
                    settings.watches.push(std::mem::take(expression));
                    actions.settings_changed = true;
                    actions.watches_changed = true;
                }
            });
        });
}
fn breakpoints_ui(
    ui: &mut egui::Ui,
    settings: &mut debugger::Settings,
    client: Option<&Client>,
    actions: &mut Actions,
) {
    egui::CollapsingHeader::new("Breakpoints").default_open(true).show(ui,|ui|{
        if ui.button("Toggle at cursor · F9").clicked(){actions.commands.push(Action::ToggleBreakpoint);}
        ui.small("Click the left gutter to toggle. Right-click a breakpoint below for conditions and logpoints.");
        let mut remove=None;
        for(path,items)in &mut settings.breakpoints {
            if items.is_empty(){continue;}
            egui::CollapsingHeader::new(ssh::display_name(path)).id_salt(("debug-source",path)).show(ui,|ui|{
                ui.small(path);let mut enabled_index=0;
                for(index,bp)in items.iter_mut().enumerate(){
                    ui.horizontal(|ui|{
                        let was_enabled=bp.enabled;
                        let response=ui.checkbox(&mut bp.enabled,format!("Line {}",bp.line));
                        let mut changed=response.changed();
                        if was_enabled&&let Some(client)=client.filter(|c|c.phase!=Phase::Ended){
                            let verified=client.breakpoints.get(path).and_then(|b|b.get(enabled_index));
                            ui.add(egui::Label::new(egui::RichText::new(if verified.is_some_and(|b|b.verified){"verified"}else{"pending"}).small()).truncate()).on_hover_ui(|ui|{
                                ui.label(verified.map(|b|if b.verified{format!("Verified at line {}",b.line)}else{format!("Unverified: {}",b.message)}).unwrap_or_else(||"Pending verification".into()));
                            });
                        }
                        if was_enabled{enabled_index+=1;}
                        response.context_menu(|ui|{
                            for(label,value)in[("Condition",&mut bp.condition),("Hit condition",&mut bp.hit_condition),("Log message (logpoint)",&mut bp.log_message)]{
                                ui.label(label);changed|=ui.add(egui::TextEdit::singleline(value).char_limit(1024)).changed();
                            }
                        });
                        if changed{actions.settings_changed=true;actions.breakpoints.insert(path.clone());}
                        if icons::small(ui,Icon::Close,"Remove breakpoint").clicked(){remove=Some((path.clone(),index));}
                    });
                }
            });
        }
        if let Some((path,index))=remove{settings.breakpoints.get_mut(&path).unwrap().remove(index);actions.breakpoints.insert(path);actions.settings_changed=true;}
    });
}
fn exceptions_ui(
    ui: &mut egui::Ui,
    settings: &mut debugger::Settings,
    client: &Client,
    actions: &mut Actions,
) {
    let Some(filters) = client.capabilities["exceptionBreakpointFilters"]
        .as_array()
        .filter(|f| !f.is_empty())
    else {
        return;
    };
    egui::CollapsingHeader::new("Exceptions (next launch)").show(ui, |ui| {
        for filter in filters.iter().take(32) {
            if let Some(id) = filter["filter"].as_str() {
                let mut selected = settings.exceptions.iter().any(|s| s == id);
                if ui
                    .checkbox(&mut selected, filter["label"].as_str().unwrap_or(id))
                    .changed()
                {
                    settings.exceptions.retain(|s| s != id);
                    if selected {
                        settings.exceptions.push(id.into());
                    }
                    actions.settings_changed = true;
                }
            }
        }
    });
}
fn console_ui(
    ui: &mut egui::Ui,
    client: &Client,
    expression: &mut String,
    palette: theme::Palette,
    actions: &mut Actions,
) {
    ui.separator();
    ui.label(palette.tag("DEBUG CONSOLE"));
    egui::ScrollArea::vertical()
        .id_salt("debug-console")
        .max_height(220.)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(&client.output).monospace())
                    .selectable(true)
                    .wrap(),
            );
        });
    ui.horizontal(|ui| {
        let paused = client.phase == Phase::Paused;
        let response = ui.add_enabled(
            paused,
            egui::TextEdit::singleline(expression)
                .hint_text("Evaluate in selected frame")
                .char_limit(4096)
                .desired_width((ui.available_width() - 72.).max(60.)),
        );
        if ui
            .add_enabled(
                paused,
                egui::Button::new("Eval")
                    .min_size(egui::vec2(48., 24.))
                    .truncate(),
            )
            .clicked()
            || response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
        {
            actions
                .commands
                .push(Action::Evaluate(std::mem::take(expression)));
        }
    });
}

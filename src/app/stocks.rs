//! Bundled desktop market viewer. One lazy worker serializes bounded HTTPS jobs;
//! the UI keeps only the newest pending intent and rejects stale completions.
//! No workspace, terminal, chat, host helper or project sandbox is involved.
use super::*;
use crate::extensions::{Feature, Registry};
use std::collections::VecDeque;
mod chart;
mod data;
use data::{Instrument, Range, Selection, Series};

const FEATURE: Feature = Feature::Stocks;
const CACHE_TTL: Duration = Duration::from_secs(300);
const CACHE_LIMIT: usize = 12;
const WATCH_LIMIT: usize = 32;

#[derive(Clone)]
enum Request {
    Search(String),
    Chart(Selection),
}
enum Content {
    Search(Vec<Instrument>),
    Chart(Arc<Series>),
}
struct Task {
    generation: u64,
    request: Request,
    viewport: egui::ViewportId,
}
struct Completion {
    task: Task,
    result: Result<Content, String>,
}
struct Worker {
    send: mpsc::SyncSender<Task>,
    receive: mpsc::Receiver<Completion>,
}
impl Worker {
    fn start(ctx: &egui::Context) -> std::io::Result<Self> {
        let (send, requests) = mpsc::sync_channel::<Task>(1);
        let (replies, receive) = mpsc::sync_channel(1);
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("stock-data".into())
            .spawn(move || {
                let client = data::client();
                while let Ok(task) = requests.recv() {
                    let result = match &task.request {
                        Request::Search(query) => data::search(&client, query).map(Content::Search),
                        Request::Chart(selection) => data::history(&client, selection)
                            .map(|series| Content::Chart(Arc::new(series))),
                    }
                    .map_err(|e| format!("{e:#}"));
                    let viewport = task.viewport;
                    if replies.send(Completion { task, result }).is_err() {
                        break;
                    }
                    // The root owns completion admission even if this request
                    // originated in an independently detached Stocks window.
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                    ctx.request_repaint_of(viewport);
                }
            })?;
        Ok(Self { send, receive })
    }
}
#[derive(Clone)]
struct Loaded {
    selection: Selection,
    series: Arc<Series>,
    fetched: Instant,
}

/// Window-owned chart/search/cache state; only symbols and display choices are persisted.
/// At most one request runs and one newer intent waits, including while disabled/hidden.
pub(super) struct State {
    initialized: bool,
    query: String,
    symbol: String,
    range: Range,
    candles: bool,
    volume: bool,
    results: Vec<Instrument>,
    searched: bool,
    error: Option<String>,
    loaded: Option<Loaded>,
    cache: VecDeque<Loaded>,
    worker: Option<Worker>,
    in_flight: bool,
    pending: Option<Task>,
    generation: u64,
    view: chart::View,
}
impl Default for State {
    fn default() -> Self {
        Self {
            initialized: false,
            query: String::new(),
            symbol: "AAPL".into(),
            range: Range::default(),
            candles: false,
            volume: true,
            results: vec![],
            searched: false,
            error: None,
            loaded: None,
            cache: VecDeque::new(),
            worker: None,
            in_flight: false,
            pending: None,
            generation: 0,
            view: chart::View::default(),
        }
    }
}
impl State {
    fn selection(&self) -> Selection {
        Selection {
            symbol: self.symbol.clone(),
            range: self.range,
        }
    }
    /// Stop admission and invalidate active results without blocking on DNS/TLS or
    /// spawning replacement workers. The bounded in-flight request may finish.
    fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
    }
    /// Re-read saved preferences on the next activation while retaining the one
    /// worker and cached charts. Disabled or edited settings reject active results.
    pub fn invalidate(&mut self) {
        self.cancel();
        self.initialized = false;
    }
    fn queue(&mut self, request: Request, ctx: &egui::Context) {
        self.cancel();
        self.error = None;
        self.pending = Some(Task {
            generation: self.generation,
            request,
            viewport: ctx.viewport_id(),
        });
        self.dispatch(ctx);
    }
    fn dispatch(&mut self, ctx: &egui::Context) {
        if self.in_flight || self.pending.is_none() {
            return;
        }
        if self.worker.is_none() {
            match Worker::start(ctx) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.error = Some(format!("Cannot start stock-data worker: {error}"));
                    self.pending = None;
                    return;
                }
            }
        }
        let task = self.pending.take().unwrap();
        if self.worker.as_ref().unwrap().send.try_send(task).is_ok() {
            self.in_flight = true;
        } else {
            self.worker = None;
            self.error = Some("Stock-data worker ended. Try Refresh.".into());
        }
    }
    /// Drain a completion even when the panel is hidden, then dispatch only the
    /// latest queued request. Detached viewports share this same controller.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let completion = match self.worker.as_ref().map(|worker| worker.receive.try_recv()) {
            Some(Ok(completion)) => Some(completion),
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.worker = None;
                self.in_flight = false;
                self.pending = None;
                self.error = Some("Stock-data worker ended. Try Refresh.".into());
                None
            }
            _ => None,
        };
        if let Some(completion) = completion {
            self.in_flight = false;
            self.accept(completion, ctx);
            self.dispatch(ctx);
        }
    }
    fn accept(&mut self, completion: Completion, ctx: &egui::Context) {
        if completion.task.generation != self.generation {
            return;
        }
        match completion.result {
            Ok(Content::Search(results)) => {
                self.results = results;
                self.searched = true;
            }
            Ok(Content::Chart(series)) => {
                if let Request::Chart(selection) = completion.task.request {
                    let loaded = Loaded {
                        selection,
                        series,
                        fetched: Instant::now(),
                    };
                    self.cache
                        .retain(|entry| entry.selection != loaded.selection);
                    self.cache.push_back(loaded.clone());
                    while self.cache.len() > CACHE_LIMIT {
                        self.cache.pop_front();
                    }
                    self.loaded = Some(loaded);
                }
            }
            Err(error) => self.error = Some(error),
        }
        ctx.request_repaint_of(completion.task.viewport);
    }
    fn load(&mut self, force: bool, ctx: &egui::Context) {
        let selection = self.selection();
        if self
            .loaded
            .as_ref()
            .is_none_or(|loaded| loaded.selection != selection)
        {
            self.view = chart::View::default();
        }
        if !force
            && let Some(loaded) = self
                .cache
                .iter()
                .find(|entry| entry.selection == selection && entry.fetched.elapsed() < CACHE_TTL)
                .cloned()
        {
            self.cancel();
            self.error = None;
            self.loaded = Some(loaded);
            return;
        }
        self.queue(Request::Chart(selection), ctx);
    }
    fn choose(&mut self, symbol: String, ctx: &egui::Context, registry: &mut Registry) {
        self.symbol = symbol;
        self.results.clear();
        self.searched = false;
        settings(registry).insert("symbol".into(), self.symbol.clone().into());
        registry.pending = true;
        self.load(false, ctx);
    }
    fn initialize(&mut self, registry: &Registry) {
        self.symbol = "AAPL".into();
        self.range = Range::default();
        self.candles = false;
        self.volume = true;
        self.results.clear();
        self.searched = false;
        if let Some(saved) = registry.preferences.get(FEATURE.id()) {
            if let Some(symbol) = saved
                .settings
                .get("symbol")
                .and_then(|v| v.as_str())
                .and_then(|v| data::symbol(v).ok())
            {
                self.symbol = symbol;
            }
            if let Some(range) = saved
                .settings
                .get("range")
                .and_then(|v| v.as_str())
                .and_then(|v| Range::ALL.into_iter().find(|r| r.label() == v))
            {
                self.range = range;
            }
            self.candles = saved
                .settings
                .get("candles")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            self.volume = saved
                .settings
                .get("volume")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
        }
        self.initialized = true;
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        registry: &mut Registry,
        palette: theme::Palette,
    ) {
        if !self.initialized {
            self.initialize(registry);
            self.load(false, ctx);
        }
        // The scroller itself has unbounded height. Measure controls relative to
        // its content origin so scrolling cannot resize the plot.
        let available_height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt("stocks-content")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let content_top = ui.cursor().top();
                layout::heading(
                    ui,
                    "Stocks",
                    "Yahoo Finance · Data may be delayed · Times in UTC",
                    palette,
                );
                ui.horizontal_wrapped(|ui| {
                    let input = ui.add(keyboard::named(
                        "Stock company or ticker",
                        egui::TextEdit::singleline(&mut self.query)
                            .id_salt("stocks-search")
                            .desired_width(250.)
                            .char_limit(120)
                            .hint_text("Company or symbol, e.g. Siemens or SIE.DE"),
                    ));
                    ui_probe::button(ui, "stocks-search", &input);
                    let enter = input.has_focus()
                        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
                    if button(ui, "Search", "stocks-search-submit") || enter {
                        if self.query.trim().is_empty() {
                            self.error = Some("Enter a company name or ticker.".into());
                        } else {
                            self.results.clear();
                            self.searched = false;
                            self.queue(Request::Search(self.query.clone()), ctx);
                        }
                    }
                    if button(ui, "Open ticker", "stocks-open") {
                        match data::symbol(&self.query) {
                            Ok(symbol) => self.choose(symbol, ctx, registry),
                            Err(error) => self.error = Some(error.to_string()),
                        }
                    }
                    let busy = self.in_flight || self.pending.is_some();
                    let refresh = ui.add_enabled(!busy, keyboard::Button::new("Refresh"));
                    ui_probe::button(ui, "stocks-refresh", &refresh);
                    if refresh.clicked() {
                        self.load(true, ctx);
                    }
                    if busy {
                        ui.spinner();
                        ui.label("Loading…");
                    }
                });
                if self.searched && self.results.is_empty() {
                    ui.label(
                        "No matching symbols. Try the exact exchange ticker with Open ticker.",
                    );
                }
                let mut selected = None;
                egui::ScrollArea::vertical()
                    .id_salt("stock-results")
                    .max_height(165.)
                    .show(ui, |ui| {
                        for result in &self.results {
                            let label = format!(
                                "{} · {} · {} · {}",
                                result.symbol, result.name, result.exchange, result.kind
                            );
                            let response = ui
                                .add_sized(
                                    [ui.available_width(), 30.],
                                    keyboard::Button::selectable(false, &label).truncate(),
                                )
                                .on_hover_text(&label);
                            ui_probe::button(
                                ui,
                                &format!("stocks-result:{}", result.symbol),
                                &response,
                            );
                            if response.clicked() {
                                selected = Some(result.symbol.clone());
                            }
                        }
                    });
                if let Some(symbol) = selected {
                    self.choose(symbol, ctx, registry);
                }
                let watchlist = watchlist(registry);
                ui.horizontal_wrapped(|ui| {
                    ui.weak("Watchlist");
                    for symbol in &watchlist {
                        let response =
                            keyboard::selectable_label(ui, self.symbol == *symbol, symbol);
                        ui_probe::button(ui, &format!("stocks-watch:{symbol}"), &response);
                        if response.clicked() {
                            self.choose(symbol.clone(), ctx, registry);
                        }
                    }
                    let pinned = watchlist.contains(&self.symbol);
                    let pin = ui.add_enabled(
                        pinned || watchlist.len() < WATCH_LIMIT,
                        keyboard::Button::new(if pinned {
                            "Remove from watchlist"
                        } else {
                            "Add to watchlist"
                        }),
                    );
                    ui_probe::button(ui, "stocks-pin", &pin);
                    if pin.clicked() {
                        match edit_watchlist(registry, &self.symbol, !pinned) {
                            Ok(()) => {}
                            Err(error) => self.error = Some(error.to_string()),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.weak("Range");
                    for range in Range::ALL {
                        let response =
                            keyboard::selectable_label(ui, self.range == range, range.label());
                        ui_probe::button(ui, &format!("stocks-range:{}", range.label()), &response);
                        if response.clicked() {
                            self.range = range;
                            settings(registry).insert("range".into(), range.label().into());
                            registry.pending = true;
                            self.load(false, ctx);
                        }
                    }
                    let line = keyboard::selectable_value(ui, &mut self.candles, false, "Line");
                    let candles =
                        keyboard::selectable_value(ui, &mut self.candles, true, "Candles");
                    ui_probe::button(ui, "stocks-line", &line);
                    ui_probe::button(ui, "stocks-candles", &candles);
                    if line.changed() || candles.changed() {
                        settings(registry).insert("candles".into(), self.candles.into());
                        registry.pending = true;
                    }
                    if keyboard::checkbox(ui, &mut self.volume, "Volume").changed() {
                        settings(registry).insert("volume".into(), self.volume.into());
                        registry.pending = true;
                    }
                });
                if let Some(error) = &self.error {
                    ui.colored_label(palette.error, error);
                }
                ui.separator();
                let loaded = self
                    .loaded
                    .as_ref()
                    .filter(|loaded| loaded.selection == self.selection())
                    .cloned();
                if let Some(loaded) = loaded {
                    let series = &loaded.series;
                    ui.horizontal_wrapped(|ui| {
                        let heading = ui.heading(&self.symbol);
                        ui_probe::button(ui, "stocks-series-ready", &heading);
                        ui_probe::value(ui, "stocks-symbol", &heading, &self.symbol);
                        ui.label(&series.name);
                        ui.weak(format!("{} · {}", series.exchange, series.currency));
                    });
                    if let Some(last) = series.bars.last() {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(format!("{} {}", chart::price(last.close), series.currency));
                            if let Some(change) = series.change_percent() {
                                ui.colored_label(
                                    if change >= 0. {
                                        palette.success
                                    } else {
                                        palette.error
                                    },
                                    format!("{change:+.2}% over selected range"),
                                );
                            }
                            ui.weak(format!("Last bar: {} UTC", chart::date(last.at, true)));
                        });
                    }
                    ui.horizontal_wrapped(|ui| {
                        if button(ui, "−", "stocks-zoom-out") {
                            self.view.zoom(1.4, 0.5, series.bars.len());
                        }
                        if button(ui, "+", "stocks-zoom-in") {
                            self.view.zoom(1. / 1.4, 0.5, series.bars.len());
                        }
                        if button(ui, "←", "stocks-pan-left") {
                            self.view.pan(-0.2);
                        }
                        if button(ui, "→", "stocks-pan-right") {
                            self.view.pan(0.2);
                        }
                        if button(ui, "Reset view", "stocks-reset") {
                            self.view = chart::View::default();
                        }
                        ui.weak("Wheel: zoom · Drag: pan · Focus chart: + / −, ← / →, Home");
                    });
                    let controls_height = ui.cursor().top() - content_top;
                    let chart_height = (available_height - controls_height - 62.).clamp(180., 520.);
                    chart::show(
                        ui,
                        series,
                        &mut self.view,
                        self.candles,
                        self.volume,
                        palette,
                        chart_height,
                    );
                    ui.small(format!(
                        "{} observations · Provider OHLC · Cached {}s ago",
                        series.bars.len(),
                        loaded.fetched.elapsed().as_secs()
                    ));
                } else {
                    layout::empty(
                        ui,
                        &self.symbol,
                        if self.in_flight {
                            "Loading price history…"
                        } else {
                            "No price history loaded. Choose a ticker or press Refresh."
                        },
                        palette,
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    ui.hyperlink_to(
                        "View source",
                        format!(
                            "https://finance.yahoo.com/quote/{}/",
                            data::encode(&self.symbol)
                        ),
                    );
                });
            });
    }
}

fn settings(registry: &mut Registry) -> &mut std::collections::BTreeMap<String, serde_json::Value> {
    &mut registry
        .preferences
        .entry(FEATURE.id().into())
        .or_default()
        .settings
}
fn watchlist(registry: &Registry) -> Vec<String> {
    registry
        .preferences
        .get(FEATURE.id())
        .and_then(|p| p.settings.get("watchlist"))
        .and_then(|v| v.as_object())
        .into_iter()
        .flat_map(|map| map.iter())
        .filter(|(_, v)| v.as_bool() == Some(true))
        .filter_map(|(key, _)| data::symbol(key).ok())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .take(WATCH_LIMIT)
        .collect()
}
/// Edit a single symbol in a map so the shared preference writer can merge
/// independent watchlist changes from different editor processes.
fn edit_watchlist(registry: &mut Registry, symbol: &str, pinned: bool) -> anyhow::Result<()> {
    let symbol = data::symbol(symbol)?;
    let mut preferences = registry.preferences.clone();
    let raw = preferences
        .entry(FEATURE.id().into())
        .or_default()
        .settings
        .entry("watchlist".into())
        .or_insert_with(|| serde_json::json!({}));
    let map = raw.as_object_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "Stock watchlist settings must be an object; existing settings were preserved."
        )
    })?;
    // Settings can be edited manually. Treat exchange symbols case-insensitively
    // so aliases cannot create duplicate buttons or an entry that cannot be removed.
    map.retain(|key, _| data::symbol(key).ok().as_ref() != Some(&symbol));
    if pinned {
        anyhow::ensure!(
            map.len() < WATCH_LIMIT,
            "The watchlist is limited to 32 symbols."
        );
        map.insert(symbol, true.into());
    }
    crate::extensions::preferences(&serde_json::to_string(&preferences)?)?;
    registry.preferences = preferences;
    registry.pending = true;
    Ok(())
}
fn button(ui: &mut egui::Ui, label: &str, id: &str) -> bool {
    let response = keyboard::button(ui, label);
    ui_probe::button(ui, id, &response);
    response.clicked()
}

impl EditorApp {
    /// Render the desktop-only native extension using shared workbench/focus chrome.
    pub(super) fn stocks_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.stocks.ui(
            ui,
            ctx,
            &mut self.extensions.registry,
            self.themes.active().palette,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_settings_replace_display_choices_and_removed_fields_restore_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = Registry::load(temp.path(), Default::default());
        settings(&mut registry).extend([
            ("symbol".into(), "SIE.DE".into()),
            ("range".into(), "1M".into()),
            ("candles".into(), true.into()),
            ("volume".into(), false.into()),
        ]);
        let mut state = State::default();
        state.initialize(&registry);
        assert_eq!(state.symbol, "SIE.DE");
        assert_eq!(state.range, Range::Month);
        assert!(state.candles && !state.volume);
        state.invalidate();
        assert!(!state.initialized);
        settings(&mut registry).clear();
        state.initialize(&registry);
        assert_eq!(state.symbol, "AAPL");
        assert_eq!(state.range, Range::Year);
        assert!(!state.candles && state.volume);
        assert!(state.worker.is_none());
    }

    fn sample() -> Arc<Series> {
        Arc::new(Series {
            name: "Sample instrument".into(),
            exchange: "TEST".into(),
            currency: "USD".into(),
            bars: (0..200)
                .map(|i| data::Bar {
                    at: 1_700_000_000 + i * 86400,
                    open: 10.,
                    high: 12.,
                    low: 9.,
                    close: 11.,
                    volume: 100.,
                })
                .collect(),
        })
    }

    #[test]
    fn recent_cache_changes_selection_without_starting_a_worker_and_old_results_cannot_replace_it()
    {
        let mut state = State::default();
        let ctx = egui::Context::default();
        let cached = Loaded {
            selection: state.selection(),
            series: sample(),
            fetched: Instant::now(),
        };
        state.cache.push_back(cached);
        state.in_flight = true;
        let task = Task {
            generation: state.generation,
            request: Request::Chart(Selection {
                symbol: "MSFT".into(),
                range: Range::Year,
            }),
            viewport: egui::ViewportId::ROOT,
        };
        state.load(false, &ctx);
        assert!(state.loaded.is_some());
        assert!(state.worker.is_none() && state.pending.is_none());
        state.accept(
            Completion {
                task,
                result: Ok(Content::Chart(sample())),
            },
            &ctx,
        );
        assert_eq!(state.loaded.as_ref().unwrap().selection.symbol, "AAPL");
    }

    #[test]
    fn stock_panel_works_without_a_workspace_and_disabling_it_rejects_active_work() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        assert!(app.workspace.as_ref().is_none());
        app.toggle_panel(Panel::Stocks, &ctx);
        assert_eq!(app.areas.active, areas::Area::Stocks);
        assert_eq!(app.workbench.panel, None);
        let generation = app.stocks.generation;
        app.set_extension(FEATURE.id(), false);
        assert!(!app.panel_enabled(Panel::Stocks));
        assert_ne!(app.stocks.generation, generation);
        assert!(app.stocks.worker.is_none());
        assert_eq!(app.workbench.panel, None);
    }

    #[test]
    fn stock_controls_and_plot_stay_inside_small_and_scaled_panels() {
        for size in [egui::vec2(460., 580.), egui::vec2(1200., 900.)] {
            for scale in [1., 1.5] {
                let temp = tempfile::tempdir().unwrap();
                let probe = temp.path().join("state.json");
                let ctx = egui::Context::default();
                keyboard::install(&ctx);
                ui_probe::install_at(&ctx, probe.clone());
                let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                let app = EditorApp::new(&cc, Startup::default());
                ctx.set_zoom_factor(scale);
                let palette = app.themes.active().palette;
                let mut state = State {
                    initialized: true,
                    ..State::default()
                };
                state.loaded = Some(Loaded {
                    selection: state.selection(),
                    series: sample(),
                    fetched: Instant::now(),
                });
                let mut registry = Registry::load(temp.path(), Default::default());
                for _ in 0..3 {
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default()
                                .show(ctx, |ui| state.ui(ui, ctx, &mut registry, palette));
                        },
                    );
                    assert!(
                        output
                            .shapes
                            .iter()
                            .all(|shape| shape.clip_rect.is_finite())
                    );
                }
                let snapshot: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&probe).unwrap()).unwrap();
                let buttons = snapshot["buttons"].as_array().unwrap();
                for name in [
                    "stocks-search",
                    "stocks-pin",
                    "stocks-range:1D",
                    "stocks-chart",
                ] {
                    let button = buttons
                        .iter()
                        .find(|button| button["name"] == name)
                        .unwrap_or_else(|| panic!("Missing {name}: {snapshot}"));
                    let rect = button["rect"].as_array().unwrap();
                    assert!(
                        rect[0].as_f64().unwrap() >= 0.
                            && rect[2].as_f64().unwrap() <= size.x as f64,
                        "{name} escaped {size:?}: {rect:?}"
                    );
                }
                assert!(state.worker.is_none());
            }
        }
    }
    #[test]
    fn cancellation_rejects_old_results_and_watchlist_edits_preserve_other_settings() {
        let mut state = State::default();
        let ctx = egui::Context::default();
        let task = Task {
            generation: state.generation,
            request: Request::Search("old".into()),
            viewport: egui::ViewportId::ROOT,
        };
        state.cancel();
        state.accept(
            Completion {
                task,
                result: Ok(Content::Search(vec![])),
            },
            &ctx,
        );
        assert!(!state.searched);
        assert!(state.worker.is_none());
        let mut registry = Registry::load(
            &PathBuf::from("/nonexistent-rode-stock-extensions"),
            Default::default(),
        );
        settings(&mut registry).insert("other".into(), true.into());
        edit_watchlist(&mut registry, "SIE.DE", true).unwrap();
        edit_watchlist(&mut registry, "7203.T", true).unwrap();
        edit_watchlist(&mut registry, "SIE.DE", false).unwrap();
        assert_eq!(watchlist(&registry), vec!["7203.T"]);
        assert_eq!(settings(&mut registry)["other"], true);
        settings(&mut registry).insert(
            "watchlist".into(),
            serde_json::json!({"sie.de":true,"SIE.DE":true}),
        );
        assert_eq!(watchlist(&registry), vec!["SIE.DE"]);
        edit_watchlist(&mut registry, "SIE.DE", false).unwrap();
        assert!(watchlist(&registry).is_empty());
        settings(&mut registry).insert("watchlist".into(), false.into());
        assert!(edit_watchlist(&mut registry, "AAPL", true).is_err());
        assert_eq!(settings(&mut registry)["watchlist"], false);
    }
}

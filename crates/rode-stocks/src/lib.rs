//! Independent market-data provider. No editor, egui, SSH or project dependency.
//! The host owns rendering and field-merged settings; the provider owns HTTP,
//! request validation, search, watchlists and the bounded price-history cache.
mod data;
use anyhow::{Result, bail, ensure};
use data::{Instrument, Range, Selection, Series};
use rode_extension_api::panel::*;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

type Settings = BTreeMap<String, serde_json::Value>;
const WATCH_LIMIT: usize = 32;
const CACHE_LIMIT: usize = 12;
const CACHE_TTL: Duration = Duration::from_secs(300);
struct Loaded {
    selection: Selection,
    series: Arc<Series>,
    fetched: Instant,
}
/// Network boundary is substituted only by deterministic local tests.
trait Market {
    fn search(&self, query: &str) -> Result<Vec<Instrument>>;
    fn history(&self, selection: &Selection) -> Result<Series>;
}
struct Yahoo(ureq::Agent);
impl Market for Yahoo {
    fn search(&self, query: &str) -> Result<Vec<Instrument>> {
        data::search(&self.0, query)
    }
    fn history(&self, selection: &Selection) -> Result<Series> {
        data::history(&self.0, selection)
    }
}
pub struct Provider {
    market: Box<dyn Market>,
    demo: bool,
    settings: Settings,
    query: String,
    symbol: String,
    range: Range,
    candles: bool,
    volume: bool,
    results: Vec<Instrument>,
    searched: bool,
    cache: VecDeque<Loaded>,
    selected: Option<Selection>,
}
impl Default for Provider {
    fn default() -> Self {
        Self {
            market: Box::new(Yahoo(data::client())),
            demo: false,
            settings: Settings::new(),
            query: String::new(),
            symbol: "AAPL".into(),
            range: Range::default(),
            candles: false,
            volume: true,
            results: vec![],
            searched: false,
            cache: VecDeque::new(),
            selected: None,
        }
    }
}
impl Provider {
    fn apply_settings(&mut self, settings: &Settings) {
        self.settings = settings.clone();
        self.symbol = settings
            .get("symbol")
            .and_then(|v| v.as_str())
            .and_then(|s| data::symbol(s).ok())
            .unwrap_or_else(|| "AAPL".into());
        self.range = settings
            .get("range")
            .and_then(|v| v.as_str())
            .and_then(|label| Range::ALL.into_iter().find(|r| r.label() == label))
            .unwrap_or_default();
        self.candles = settings
            .get("candles")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        self.volume = settings
            .get("volume")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
    }
    fn selection(&self) -> Selection {
        Selection {
            symbol: self.symbol.clone(),
            range: self.range,
        }
    }
    fn load(&mut self, force: bool) -> Result<()> {
        let selection = self.selection();
        if !force
            && self
                .cache
                .iter()
                .any(|item| item.selection == selection && item.fetched.elapsed() < CACHE_TTL)
        {
            self.selected = Some(selection);
            return Ok(());
        }
        let series = self.market.history(&selection)?;
        series.validate()?;
        self.cache.retain(|item| item.selection != selection);
        self.cache.push_back(Loaded {
            selection: selection.clone(),
            series: Arc::new(series),
            fetched: Instant::now(),
        });
        while self.cache.len() > CACHE_LIMIT {
            self.cache.pop_front();
        }
        self.selected = Some(selection);
        Ok(())
    }
    fn choose(&mut self, symbol: &str) -> Result<()> {
        self.symbol = data::symbol(symbol)?;
        self.settings
            .insert("symbol".into(), self.symbol.clone().into());
        self.results.clear();
        self.searched = false;
        self.load(false)
    }
    fn watchlist(&self) -> Vec<String> {
        self.settings
            .get("watchlist")
            .and_then(|v| v.as_object())
            .into_iter()
            .flat_map(|o| o.iter())
            .filter(|(_, v)| v.as_bool() == Some(true))
            .filter_map(|(s, _)| data::symbol(s).ok())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .take(WATCH_LIMIT)
            .collect()
    }
    fn action(&mut self, request: &Request) -> Result<bool> {
        self.apply_settings(&request.settings);
        if let Some(query) = request.values.get("query") {
            ensure!(query.len() <= 120, "Search is limited to 120 bytes");
            self.query = query.clone();
        }
        let action = request.action.as_str();
        match action {
            "open" | "settings" => {
                self.load(false)?;
                Ok(false)
            }
            "refresh" => {
                self.load(true)?;
                Ok(false)
            }
            "search" => {
                self.results = self.market.search(&self.query)?;
                self.searched = true;
                Ok(false)
            }
            "open-ticker" => {
                let symbol = self.query.clone();
                self.choose(&symbol)?;
                Ok(true)
            }
            "pin" => {
                let pinned = self.watchlist().contains(&self.symbol);
                ensure!(
                    pinned || self.watchlist().len() < WATCH_LIMIT,
                    "Watchlist is full (32 symbols)"
                );
                let raw = self
                    .settings
                    .entry("watchlist".into())
                    .or_insert_with(|| serde_json::json!({}));
                let rows = raw
                    .as_object_mut()
                    .ok_or_else(|| anyhow::anyhow!("Watchlist settings must be an object"))?;
                let matches: Vec<_> = rows
                    .keys()
                    .filter(|key| data::symbol(key).ok().as_ref() == Some(&self.symbol))
                    .cloned()
                    .collect();
                for key in matches {
                    rows.insert(key, false.into());
                }
                rows.insert(self.symbol.clone(), (!pinned).into());
                Ok(true)
            }
            "style:line" | "style:candles" => {
                self.candles = action == "style:candles";
                self.settings.insert("candles".into(), self.candles.into());
                Ok(true)
            }
            "volume" => {
                self.volume = !self.volume;
                self.settings.insert("volume".into(), self.volume.into());
                Ok(true)
            }
            action if action.starts_with("watch:") => {
                self.choose(&action[6..])?;
                Ok(true)
            }
            action if action.starts_with("choose:") => {
                self.choose(&action[7..])?;
                Ok(true)
            }
            action if action.starts_with("range:") => {
                self.range = Range::ALL
                    .into_iter()
                    .find(|r| r.label() == &action[6..])
                    .ok_or_else(|| anyhow::anyhow!("Unknown market range"))?;
                self.settings
                    .insert("range".into(), self.range.label().into());
                self.load(false)?;
                Ok(true)
            }
            _ => bail!("Unknown Stocks action"),
        }
    }
    pub fn handle(&mut self, request: Request) -> Reply {
        // Settings changed by explicit selection are retained even when a market
        // request fails; the previous chart never impersonates the new symbol.
        let before = request.settings.clone();
        let result = self.action(&request);
        let changed = self.settings != before;
        let mut view = self.view();
        if let Err(error) = result {
            view.error = true;
            view.message = format!("{error:#}");
        }
        Reply {
            api: request.api,
            id: request.id,
            view,
            settings: changed.then(|| self.settings.clone()),
        }
    }
    fn view(&self) -> View {
        let pinned = self.watchlist().contains(&self.symbol);
        let mut view = View {
            title: "Stocks".into(),
            status: if self.demo {
                "Demo · synthetic market data · No network requests"
            } else {
                "Yahoo Finance · Data may be delayed · Times in UTC"
            }
            .into(),
            fields: vec![Field {
                key: "query".into(),
                label: "Company or ticker".into(),
                value: self.query.clone(),
                choices: vec![],
                multiline: false,
                inline: true,
                submit: Some("search".into()),
            }],
            actions: vec![
                Action::new("search", "Search", true),
                Action::new("open-ticker", "Open ticker", true),
                Action::new("refresh", "Refresh", true),
                Action::new(
                    "pin",
                    if pinned {
                        "Remove from watchlist"
                    } else {
                        "Add to watchlist"
                    },
                    pinned || self.watchlist().len() < WATCH_LIMIT,
                ),
            ],
            ..View::default()
        };
        if self.searched && self.results.is_empty() {
            view.message =
                "No matching symbols. Try Open ticker with an exact exchange ticker.".into();
        }
        if !self.results.is_empty() {
            view.action_groups.push(ActionGroup {
                key: String::new(),
                title: "Search results".into(),
                actions: self
                    .results
                    .iter()
                    .map(|r| {
                        Action::new(
                            &format!("choose:{}", r.symbol),
                            &format!("{} · {} · {} · {}", r.symbol, r.name, r.exchange, r.kind),
                            true,
                        )
                    })
                    .collect(),
            });
        }
        let watchlist = self.watchlist();
        if !watchlist.is_empty() {
            view.action_groups.push(ActionGroup {
                key: String::new(),
                title: "Watchlist".into(),
                actions: watchlist
                    .iter()
                    .map(|s| {
                        let mut a = Action::new(&format!("watch:{s}"), s, true);
                        a.selected = *s == self.symbol;
                        a
                    })
                    .collect(),
            });
        }
        view.action_groups.push(ActionGroup {
            key: String::new(),
            title: "Range".into(),
            actions: Range::ALL
                .iter()
                .map(|r| {
                    let mut a = Action::new(&format!("range:{}", r.label()), r.label(), true);
                    a.selected = *r == self.range;
                    a
                })
                .collect(),
        });
        let mut line = Action::new("style:line", "Line", true);
        line.selected = !self.candles;
        let mut candles = Action::new("style:candles", "Candles", true);
        candles.selected = self.candles;
        let mut volume = Action::new("volume", "Volume", true);
        volume.selected = self.volume;
        let controls = view.action_groups.last_mut().expect("range controls exist");
        controls.title = "Range & display".into();
        controls.actions.extend([line, candles, volume]);
        if let Some(loaded) = self.cache.iter().find(|item| {
            Some(&item.selection) == self.selected.as_ref() && item.selection == self.selection()
        }) {
            let series = &loaded.series;
            if let Some(last) = series.bars.last() {
                view.metrics.push(Metric {
                    key: String::new(),
                    label: format!("{} · {}", self.symbol, series.currency),
                    value: format_price(last.close),
                    progress: None,
                });
                if let Some(change) = series.change_percent() {
                    view.metrics.push(Metric {
                        key: String::new(),
                        label: "Selected range".into(),
                        value: format!("{change:+.2}%"),
                        progress: None,
                    });
                }
            }
            view.charts.push(Chart {
                key: format!("{}:{}", self.symbol, self.range.label()),
                title: format!("{} · {}", self.symbol, series.name),
                subtitle: format!(
                    "{} · {} · {} observations · Cached {}s ago",
                    series.exchange,
                    series.currency,
                    series.bars.len(),
                    loaded.fetched.elapsed().as_secs()
                ),
                series: (**series).clone(),
                candles: self.candles,
                volume: self.volume,
            });
        } else {
            view.message = if view.message.is_empty() {
                format!(
                    "No price history loaded for {}. Press Refresh to retry.",
                    self.symbol
                )
            } else {
                view.message
            };
        }
        view.links.push(Link {
            key: String::new(),
            label: "View source".into(),
            url: format!(
                "https://finance.yahoo.com/quote/{}/",
                data::encode(&self.symbol)
            ),
        });
        view
    }
}
/// Explicit offline data source for UI demonstrations and native smoke tests.
/// It is never selected by discovery, installation or the included manifest.
struct Demo;
impl Market for Demo {
    fn search(&self, query: &str) -> Result<Vec<Instrument>> {
        ensure!(!query.trim().is_empty(), "Enter a company name or ticker");
        Ok(vec![Instrument {
            symbol: "SIE.DE".into(),
            name: "Synthetic instrument".into(),
            exchange: "DEMO".into(),
            kind: "Equity".into(),
        }])
    }
    fn history(&self, selection: &Selection) -> Result<Series> {
        let symbol = data::symbol(&selection.symbol)?;
        Ok(Series {
            name: format!("{symbol} synthetic demonstration"),
            currency: "USD".into(),
            exchange: "DEMO".into(),
            bars: (0..200)
                .map(|i| {
                    let close = 100. + (i as f64 / 9.).sin() * 8. + i as f64 * 0.1;
                    Bar {
                        at: 1_700_000_000 + i * 86400,
                        open: close - 0.5,
                        high: close + 2.,
                        low: close - 2.,
                        close,
                        volume: 10_000. + (i % 13) as f64 * 1000.,
                    }
                })
                .collect(),
        })
    }
}

/// Running without --stdio is inert, so discovery/installation cannot fetch data.
pub fn run() -> Result<()> {
    if !std::env::args().skip(1).any(|a| a == "--stdio") {
        println!("RODE Stocks provider. Use --stdio with a compatible extension host.");
        return Ok(());
    }
    let mut provider = Provider::default();
    if std::env::args().skip(1).any(|a| a == "--demo") {
        provider.market = Box::new(Demo);
        provider.demo = true;
    }
    rode_extension_api::server::serve(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        |request| provider.handle(request),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Fake {
        calls: Arc<AtomicUsize>,
    }
    impl Market for Fake {
        fn search(&self, _: &str) -> Result<Vec<Instrument>> {
            Ok(vec![Instrument {
                symbol: "SIE.DE".into(),
                name: "Siemens".into(),
                exchange: "Test".into(),
                kind: "Equity".into(),
            }])
        }
        fn history(&self, selection: &Selection) -> Result<Series> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ensure!(selection.symbol != "FAIL", "Synthetic provider outage");
            Ok(Series {
                name: selection.symbol.clone(),
                currency: "EUR".into(),
                exchange: "TEST".into(),
                bars: (0..12)
                    .map(|i| Bar {
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
    }
    fn fixture() -> (Provider, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Provider {
                market: Box::new(Fake {
                    calls: calls.clone(),
                }),
                ..Provider::default()
            },
            calls,
        )
    }
    fn action(provider: &mut Provider, action: &str, query: &str) -> Reply {
        let reply = provider.handle(Request {
            api: 2,
            id: 1,
            action: action.into(),
            values: BTreeMap::from([("query".into(), query.into())]),
            settings: provider.settings.clone(),
        });
        reply.view.validate().unwrap();
        reply
    }
    #[test]
    fn stock_domain_is_independent_and_native_style_changes_do_not_refetch_prices() {
        let (mut provider, calls) = fixture();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let view = action(&mut provider, "open", "");
        assert_eq!(view.view.charts.len(), 1);
        assert!(
            action(&mut provider, "search", "Siemens")
                .view
                .action_groups
                .iter()
                .any(|g| g.title == "Search results")
        );
        action(&mut provider, "choose:SIE.DE", "");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(action(&mut provider, "style:candles", "").view.charts[0].candles);
        assert!(!action(&mut provider, "volume", "").view.charts[0].volume);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        action(&mut provider, "range:1M", "");
        action(&mut provider, "range:1Y", "");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "Returning to cached data should not fetch it again"
        );
        let failed = action(&mut provider, "choose:FAIL", "");
        assert!(failed.view.error);
        assert!(
            failed.view.charts.is_empty(),
            "A stale chart must not impersonate the failed symbol"
        );
    }
    #[test]
    fn watchlist_edits_keep_unrelated_preferences_and_normalize_aliases() {
        let (mut provider, _) = fixture();
        provider.settings.insert("other".into(), true.into());
        provider.settings.insert("symbol".into(), "SIE.DE".into());
        provider.settings.insert(
            "watchlist".into(),
            serde_json::json!({"sie.de":true,"SIE.DE":true}),
        );
        let reply = action(&mut provider, "pin", "");
        assert_eq!(reply.settings.unwrap()["other"], true);
        assert!(provider.watchlist().is_empty());
        action(&mut provider, "pin", "");
        assert_eq!(provider.watchlist(), ["SIE.DE"]);
        provider.settings.insert("watchlist".into(), false.into());
        assert!(action(&mut provider, "pin", "").view.error);
        assert_eq!(provider.settings["watchlist"], false);
    }
}

//! Bounded, read-only Yahoo Finance requests. No project host or credentials are used.
//! These public endpoints are unofficial: callers retain cached data on failure and
//! display the provider and observation dates instead of promising live quotes.
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{collections::BTreeMap, io::Read, time::Duration};

const RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const BAR_LIMIT: usize = 25_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum Range {
    Day,
    Week,
    Month,
    Quarter,
    HalfYear,
    #[default]
    Year,
    FiveYears,
    All,
}
impl Range {
    pub const ALL: [Self; 8] = [
        Self::Day,
        Self::Week,
        Self::Month,
        Self::Quarter,
        Self::HalfYear,
        Self::Year,
        Self::FiveYears,
        Self::All,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "1D",
            Self::Week => "5D",
            Self::Month => "1M",
            Self::Quarter => "3M",
            Self::HalfYear => "6M",
            Self::Year => "1Y",
            Self::FiveYears => "5Y",
            Self::All => "All",
        }
    }
    fn query(self) -> (&'static str, &'static str) {
        match self {
            Self::Day => ("1d", "5m"),
            Self::Week => ("5d", "15m"),
            Self::Month => ("1mo", "1d"),
            Self::Quarter => ("3mo", "1d"),
            Self::HalfYear => ("6mo", "1d"),
            Self::Year => ("1y", "1d"),
            Self::FiveYears => ("5y", "1wk"),
            Self::All => ("max", "1mo"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Selection {
    pub symbol: String,
    pub range: Range,
}
#[derive(Clone, Debug)]
pub(super) struct Instrument {
    pub symbol: String,
    pub name: String,
    pub exchange: String,
    pub kind: String,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Bar {
    pub at: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}
#[derive(Debug)]
pub(super) struct Series {
    pub name: String,
    pub currency: String,
    pub exchange: String,
    pub bars: Vec<Bar>,
}
impl Series {
    /// A first-to-last close change is meaningful only with a positive starting
    /// price and a finite result; negative-price instruments have no such return.
    pub fn change_percent(&self) -> Option<f64> {
        let first = self.bars.first()?.close;
        let last = self.bars.last()?.close;
        if first <= 0. {
            return None;
        }
        let change = (last / first - 1.) * 100.;
        change.is_finite().then_some(change)
    }
}

/// A small allowlist admits international suffixes, indices and FX/crypto symbols.
/// Symbols are still percent-encoded before entering the fixed HTTPS URL.
pub(super) fn symbol(value: &str) -> Result<String> {
    let value = value.trim();
    ensure!(
        !value.is_empty()
            && value.len() <= 40
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".^-=_".contains(&b)),
        "Enter a ticker such as AAPL, SIE.DE or 7203.T."
    );
    Ok(value.to_ascii_uppercase())
}
pub(super) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// A single lazy worker reuses TLS connections; all timeouts also apply to tests.
pub(super) fn client() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(20)))
        .timeout_resolve(Some(Duration::from_secs(5)))
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_per_read(Some(Duration::from_secs(5)))
        .build()
        .new_agent()
}
fn get(client: &ureq::Agent, url: &str) -> Result<Value> {
    let response = client.get(url).header("User-Agent", "RODE/0.1 stock viewer").header("Accept", "application/json").call()
        .map_err(|e| anyhow::anyhow!("Yahoo Finance request failed: {e}. The service may be unavailable or rate limited; try again later."))?;
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take((RESPONSE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .context("Cannot read market data")?;
    ensure!(
        bytes.len() <= RESPONSE_LIMIT,
        "Market data response exceeds its size limit"
    );
    serde_json::from_slice(&bytes).context("Yahoo Finance returned an invalid response")
}
pub(super) fn search(client: &ureq::Agent, query: &str) -> Result<Vec<Instrument>> {
    ensure!(
        !query.trim().is_empty() && query.len() <= 120,
        "Enter a company name or ticker (up to 120 bytes)."
    );
    let value = get(
        client,
        &format!(
            "https://query1.finance.yahoo.com/v1/finance/search?q={}&quotesCount=12&newsCount=0&enableFuzzyQuery=false",
            encode(query.trim())
        ),
    )?;
    parse_search(&value)
}
fn field(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(160)
        .collect()
}
fn parse_search(value: &Value) -> Result<Vec<Instrument>> {
    let rows = value["quotes"]
        .as_array()
        .context("Yahoo Finance returned no symbol list")?;
    let mut seen = std::collections::HashSet::new();
    Ok(rows
        .iter()
        .filter_map(|row| {
            let ticker = symbol(row["symbol"].as_str()?).ok()?;
            if !seen.insert(ticker.clone()) {
                return None;
            }
            Some(Instrument {
                symbol: ticker,
                name: if row["shortname"].is_string() {
                    field(row, "shortname")
                } else {
                    field(row, "longname")
                },
                exchange: field(row, "exchDisp"),
                kind: field(row, "quoteType"),
            })
        })
        .take(12)
        .collect())
}
pub(super) fn history(client: &ureq::Agent, selection: &Selection) -> Result<Series> {
    let ticker = symbol(&selection.symbol)?;
    let (range, interval) = selection.range.query();
    let value = get(
        client,
        &format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{}?range={range}&interval={interval}&includePrePost=false",
            encode(&ticker)
        ),
    )?;
    parse_history(&value, &ticker)
}
/// Skip null/malformed observations, sort timestamps and deduplicate bars. All array
/// accesses are checked because suspended/illiquid instruments often contain nulls.
fn parse_history(value: &Value, ticker: &str) -> Result<Series> {
    if !value["chart"]["error"].is_null() {
        bail!(
            "Yahoo Finance: {}",
            field(&value["chart"]["error"], "description")
        );
    }
    let result = value["chart"]["result"]
        .get(0)
        .context("No chart is available for this symbol")?;
    let meta = &result["meta"];
    ensure!(
        symbol(meta["symbol"].as_str().unwrap_or(ticker))? == ticker,
        "Market data belongs to a different symbol"
    );
    let timestamps = result["timestamp"]
        .as_array()
        .context("No prices are available for this time range")?;
    ensure!(
        timestamps.len() <= BAR_LIMIT,
        "Market data contains too many observations"
    );
    let quote = &result["indicators"]["quote"][0];
    let mut bars = BTreeMap::new();
    for (i, at) in timestamps.iter().enumerate() {
        let Some(at) = at
            .as_i64()
            .filter(|&at| (-62_135_596_800..=253_402_300_799).contains(&at))
        else {
            continue;
        };
        let number = |key: &str| {
            quote[key]
                .get(i)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && v.abs() <= 1e100)
        };
        let (Some(open), Some(high), Some(low), Some(close)) = (
            number("open"),
            number("high"),
            number("low"),
            number("close"),
        ) else {
            continue;
        };
        if low > high || open < low || open > high || close < low || close > high {
            continue;
        }
        bars.insert(
            at,
            Bar {
                at,
                open,
                high,
                low,
                close,
                volume: number("volume").unwrap_or(0.).max(0.),
            },
        );
    }
    ensure!(
        !bars.is_empty(),
        "No valid prices are available for this symbol and time range"
    );
    Ok(Series {
        name: if meta["longName"].is_string() {
            field(meta, "longName")
        } else {
            field(meta, "shortName")
        },
        currency: field(meta, "currency"),
        exchange: field(meta, "exchangeName"),
        bars: bars.into_values().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn queries_encode_input_and_admit_international_symbols_only() {
        assert_eq!(symbol(" sie.de ").unwrap(), "SIE.DE");
        assert!(symbol("AAPL?range=max").is_err());
        assert!(symbol("../AAPL").is_err());
        assert_eq!(
            encode("トヨタ & Co"),
            "%E3%83%88%E3%83%A8%E3%82%BF%20%26%20Co"
        );
        for range in Range::ALL {
            let (period, interval) = range.query();
            assert!(!period.is_empty() && !interval.is_empty());
        }
    }

    #[test]
    fn charts_handle_missing_observations_duplicates_and_provider_errors() {
        let value = json!({"chart":{"result":[{"meta":{"symbol":"AAPL","currency":"USD"},"timestamp":[20,10,10,30],"indicators":{"quote":[{"open":[2,1,1,null],"high":[3,2,2,4],"low":[1,0,0,2],"close":[2,1,1,3],"volume":[100,null,20,30]}]}}],"error":null}});
        let series = parse_history(&value, "AAPL").unwrap();
        assert_eq!(series.bars.len(), 2);
        assert_eq!(series.bars[0].at, 10);
        assert_eq!(series.bars[0].volume, 20.);
        assert!(parse_history(&value, "MSFT").is_err());
        assert!(
            parse_history(
                &json!({"chart":{"result":null,"error":{"description":"Not found"}}}),
                "BAD"
            )
            .unwrap_err()
            .to_string()
            .contains("Not found")
        );
        assert!(parse_history(&json!({"chart":{"result":[]}}), "AAPL").is_err());
    }

    #[test]
    fn changes_omit_nonpositive_baselines_and_overflow_instead_of_displaying_invalid_returns() {
        let mut series = Series {
            name: String::new(),
            currency: String::new(),
            exchange: String::new(),
            bars: vec![
                Bar {
                    at: -1,
                    open: 10.,
                    high: 10.,
                    low: 10.,
                    close: 10.,
                    volume: 0.
                };
                2
            ],
        };
        series.bars[1].close = 15.;
        assert_eq!(series.change_percent(), Some(50.));
        for baseline in [0., -10., f64::MIN_POSITIVE] {
            series.bars[0].close = baseline;
            series.bars[1].close = 1e100;
            assert_eq!(series.change_percent(), None);
        }
    }

    #[test]
    fn search_keeps_valid_unique_symbols_and_bounds_results() {
        let rows = json!({"quotes":[{"symbol":"SIE.DE","shortname":"Siemens"},{"symbol":"SIE.DE"},{"symbol":"bad?"},{"symbol":"7203.T"}]});
        let results = parse_search(&rows).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "Siemens");
        assert!(parse_search(&Value::Null).is_err());
    }
}

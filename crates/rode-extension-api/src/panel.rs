//! Panel protocols 1, 2 and 3 contain data only. Providers cannot send markup, executable
//! UI code, paths to load into the editor, or commands for the host to execute.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
/// Wire budgets include the trailing newline; both host and SDK use these limits.
pub const REQUEST_LIMIT: usize = 1024 * 1024;
pub const REPLY_LIMIT: usize = 4 * 1024 * 1024;
pub use crate::SETTINGS_LIMIT;
/// Lifecycle requests are read-only and never submit form values.
pub fn lifecycle_action(action: &str) -> bool {
    matches!(action, "open" | "tick" | "settings")
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    /// Protocol 3: complete ordered layout, referencing controls by stable key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<Content>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<PanelLayout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub error: bool,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub metrics: Vec<Metric>,
    #[serde(default)]
    pub tables: Vec<Table>,
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub refresh_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub action_groups: Vec<ActionGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub charts: Vec<Chart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
}
/// Declarative preferences; the host adapts columns to available width and
/// owns fonts, colors, spacing, accessibility and equal-height metric cards.
#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PanelLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric_columns: Option<u8>,
    #[serde(default, skip_serializing_if = "MetricDensity::is_auto")]
    pub metric_density: MetricDensity,
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricDensity {
    /// Let the host choose density for the available content.
    #[default]
    Auto,
    Comfortable,
    Compact,
}
impl MetricDensity {
    fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}
/// Native section in one of two mutually exclusive layout modes:
/// ordered layouts use `content`; automatic layouts use the legacy reference
/// lists. Only automatic layouts implicitly place unreferenced controls.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<Content>,
    /// Stable identity required for collapsible sections, independent of ordering.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsible: bool,
    /// Initial state only; subsequent user toggles are owned by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_open: Option<bool>,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Legacy automatic layout: field keys, rendered after tables.
    #[serde(default)]
    pub fields: Vec<String>,
    /// Legacy automatic layout only: table indices. Ordered content uses keys.
    #[serde(default)]
    pub tables: Vec<usize>,
    /// Legacy automatic layout: action IDs, rendered after inputs.
    #[serde(default)]
    pub actions: Vec<String>,
}
/// Protocol 3 native composition. Groups retain host-owned responsive layout.
/// Sections may appear only at the root; no recursive widget trees are accepted.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Content {
    Metrics { keys: Vec<String> },
    Fields { keys: Vec<String> },
    Actions { keys: Vec<String> },
    Table { key: String },
    Chart { key: String },
    Link { key: String },
    ActionGroup { key: String },
    Section { key: String },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub api: u32,
    pub id: u64,
    pub view: View,
    /// A complete updated preference snapshot, merged against sent settings by
    /// the host. Absent on actions that do not change preferences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<BTreeMap<String, serde_json::Value>>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub multiline: bool,
    /// Automatic layout only: keep this input outside the Settings disclosure.
    /// Ordered layouts express placement through `content` and reject `true`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub value: String,
    pub label: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Metric {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub progress: Option<f32>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Table {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    pub title: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub confirm: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub selected: bool,
}
impl View {
    /// Reject capabilities unavailable in the negotiated protocol, rather than
    /// accidentally accepting version-2 controls from a version-1 provider.
    pub fn validate_version(&self, api: u32) -> anyhow::Result<()> {
        anyhow::ensure!(matches!(api, 1..=3), "Unsupported panel protocol");
        self.validate()?;
        if api < 3 {
            anyhow::ensure!(
                self.content.is_empty()
                    && self.layout.is_none()
                    && self.sections.iter().all(|s| s.content.is_empty()
                        && s.key.is_empty()
                        && !s.collapsible
                        && s.default_open.is_none())
                    && self.metrics.iter().all(|x| x.key.is_empty())
                    && self.tables.iter().all(|x| x.key.is_empty())
                    && self.links.iter().all(|x| x.key.is_empty())
                    && self.action_groups.iter().all(|x| x.key.is_empty()),
                "Panel protocol 3 required for composition and layout preferences"
            );
        }
        if api == 1 {
            anyhow::ensure!(
                self.layout.is_none()
                    && self.sections.is_empty()
                    && self.action_groups.is_empty()
                    && self.charts.is_empty()
                    && self.links.is_empty()
                    && self.fields.iter().all(|f| !f.inline && f.submit.is_none())
                    && self.actions.iter().all(|a| !a.selected),
                "Panel protocol 1 cannot use protocol-2 controls"
            );
        }
        Ok(())
    }
    fn validate_content(&self) -> anyhow::Result<()> {
        use anyhow::ensure;
        if self.content.is_empty() {
            ensure!(
                self.sections.iter().all(|s| s.content.is_empty()),
                "Ordered sections require a root content layout"
            );
            return Ok(());
        }
        ensure!(
            self.fields.iter().all(|field| !field.inline),
            "Ordered layouts use content for placement; field.inline requires automatic layout"
        );
        ensure!(
            self.sections.iter().all(|s| valid_key(&s.key)
                && s.fields.is_empty()
                && s.tables.is_empty()
                && s.actions.is_empty()),
            "Ordered layout requires keyed sections without legacy references"
        );
        let mut expected = std::collections::BTreeSet::new();
        for (kind, keys) in [
            (
                "metric",
                self.metrics.iter().map(|x| &x.key).collect::<Vec<_>>(),
            ),
            ("field", self.fields.iter().map(|x| &x.key).collect()),
            ("action", self.actions.iter().map(|x| &x.id).collect()),
            ("table", self.tables.iter().map(|x| &x.key).collect()),
            ("chart", self.charts.iter().map(|x| &x.key).collect()),
            ("link", self.links.iter().map(|x| &x.key).collect()),
            (
                "action_group",
                self.action_groups.iter().map(|x| &x.key).collect(),
            ),
            ("section", self.sections.iter().map(|x| &x.key).collect()),
        ] {
            for key in keys {
                ensure!(
                    valid_key(key) && expected.insert((kind, key.as_str())),
                    "Ordered layout requires unique stable control keys"
                );
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for (items, root) in std::iter::once((&self.content, true))
            .chain(self.sections.iter().map(|s| (&s.content, false)))
        {
            ensure!(items.len() <= 256, "Too many layout blocks");
            for item in items {
                let (kind, keys) = match item {
                    Content::Metrics { keys } => ("metric", keys.clone()),
                    Content::Fields { keys } => ("field", keys.clone()),
                    Content::Actions { keys } => ("action", keys.clone()),
                    Content::Table { key } => ("table", vec![key.clone()]),
                    Content::Chart { key } => ("chart", vec![key.clone()]),
                    Content::Link { key } => ("link", vec![key.clone()]),
                    Content::ActionGroup { key } => ("action_group", vec![key.clone()]),
                    Content::Section { key } => {
                        ensure!(root, "Nested sections are not supported");
                        ("section", vec![key.clone()])
                    }
                };
                ensure!(
                    !keys.is_empty() && keys.len() <= 128,
                    "Invalid layout group size"
                );
                for key in keys {
                    ensure!(
                        expected.contains(&(kind, key.as_str())) && seen.insert((kind, key)),
                        "Unknown or repeated layout reference"
                    );
                }
            }
        }
        ensure!(
            seen.len() == expected.len(),
            "Ordered layout must place every control exactly once"
        );
        Ok(())
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        use anyhow::ensure;
        let text = |s: &str| max_text(s, 8192);
        ensure!(
            max_text(&self.title, 120) && text(&self.status) && text(&self.message),
            "Panel text exceeds its limit"
        );
        ensure!(
            self.sections.len() <= 12
                && self.fields.len() <= 128
                && self.metrics.len() <= 24
                && self.tables.len() <= 12
                && self.actions.len() <= 32
                && self.action_groups.len() <= 8
                && self.charts.len() <= 4
                && self.links.len() <= 8,
            "Panel has too many controls"
        );
        ensure!(
            self.layout
                .is_none_or(|layout| layout.metric_columns.is_none_or(|n| (1..=4).contains(&n))),
            "Metric columns must be between 1 and 4"
        );
        let mut keys = std::collections::BTreeSet::new();
        for f in &self.fields {
            ensure!(
                valid_key(&f.key)
                    && keys.insert(&f.key)
                    && text(&f.label)
                    && text(&f.value)
                    && f.choices.len() <= 1024
                    && f.choices.iter().all(|c| text(&c.value) && text(&c.label)),
                "Panel contains invalid fields"
            );
        }
        keys.clear();
        for a in self
            .actions
            .iter()
            .chain(self.action_groups.iter().flat_map(|g| &g.actions))
        {
            ensure!(
                valid_key(&a.id)
                    && !lifecycle_action(&a.id)
                    && keys.insert(&a.id)
                    && max_text(&a.label, 8192)
                    && a.confirm.as_ref().is_none_or(|s| text(s)),
                "Panel contains invalid actions"
            );
        }
        ensure!(keys.len() <= 128, "Panel has too many actions");
        for group in &self.action_groups {
            ensure!(text(&group.title), "Invalid action-group title");
        }
        for field in &self.fields {
            ensure!(
                field.submit.as_ref().is_none_or(|id| keys.contains(id)),
                "Input names an undeclared submit action"
            );
        }
        for keys in [
            self.metrics.iter().map(|x| &x.key).collect::<Vec<_>>(),
            self.tables.iter().map(|x| &x.key).collect(),
            self.links.iter().map(|x| &x.key).collect(),
            self.action_groups.iter().map(|x| &x.key).collect(),
        ] {
            let mut unique = std::collections::BTreeSet::new();
            ensure!(
                keys.into_iter()
                    .filter(|k| !k.is_empty())
                    .all(|k| valid_key(k) && unique.insert(k)),
                "Invalid or repeated control key"
            );
        }
        self.validate_content()?;
        let mut placed_fields = std::collections::BTreeSet::new();
        let mut placed_tables = std::collections::BTreeSet::new();
        let mut placed_actions = std::collections::BTreeSet::new();
        let mut section_keys = std::collections::BTreeSet::new();
        for section in &self.sections {
            ensure!(
                !section.collapsible || !section.key.is_empty(),
                "Collapsible sections require a stable key"
            );
            ensure!(
                section.key.is_empty()
                    || (valid_key(&section.key) && section_keys.insert(&section.key)),
                "Invalid or repeated section key"
            );
            ensure!(
                section.collapsible || section.default_open.is_none(),
                "Section initial state requires a collapsible section"
            );
            ensure!(
                text(&section.title)
                    && text(&section.description)
                    && section.fields.len() <= 128
                    && section.tables.len() <= 12
                    && section.actions.len() <= 32,
                "Invalid panel section"
            );
            for key in &section.fields {
                ensure!(
                    self.fields.iter().any(|f| &f.key == key) && placed_fields.insert(key),
                    "Section references an unknown or repeated field"
                );
            }
            for index in &section.tables {
                ensure!(
                    *index < self.tables.len() && placed_tables.insert(index),
                    "Section references an unknown or repeated table"
                );
            }
            for id in &section.actions {
                ensure!(
                    self.actions.iter().any(|a| &a.id == id) && placed_actions.insert(id),
                    "Section references an unknown or repeated action"
                );
            }
        }
        for link in &self.links {
            ensure!(
                text(&link.label)
                    && text(&link.url)
                    && (link.url.starts_with("https://") || link.url.starts_with("http://"))
                    && !link.url.chars().any(char::is_control),
                "Invalid panel link"
            );
        }
        let mut chart_keys = std::collections::BTreeSet::new();
        for chart in &self.charts {
            ensure!(
                valid_key(&chart.key)
                    && chart_keys.insert(&chart.key)
                    && text(&chart.title)
                    && text(&chart.subtitle),
                "Invalid chart identity"
            );
            chart.series.validate()?;
        }
        for m in &self.metrics {
            ensure!(
                text(&m.label)
                    && text(&m.value)
                    && m.progress
                        .is_none_or(|v| v.is_finite() && (0.0..=1.0).contains(&v)),
                "Panel metric is invalid"
            );
        }
        for t in &self.tables {
            ensure!(
                text(&t.title)
                    && (1..=16).contains(&t.columns.len())
                    && t.columns.iter().all(|s| text(s))
                    && t.rows.len() <= 4096
                    && t.rows
                        .iter()
                        .all(|r| r.len() == t.columns.len() && r.iter().all(|s| text(s))),
                "Panel table is invalid"
            );
        }
        ensure!(
            self.refresh_seconds
                .is_none_or(|s| (30..=3600).contains(&s)),
            "Panel refresh interval must be 30–3600 seconds"
        );
        Ok(())
    }
}
fn max_text(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains('\0')
}
fn valid_key(s: &str) -> bool {
    !s.is_empty() && s.len() <= 160 && !s.chars().any(char::is_control)
}

/// Native selectable action rows serve search results, watchlists and range
/// controls without provider-specific widgets or executable UI code.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionGroup {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    pub title: String,
    pub actions: Vec<Action>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    pub label: String,
    pub url: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Chart {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    pub series: Series,
    #[serde(default)]
    pub candles: bool,
    #[serde(default)]
    pub volume: bool,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bar {
    pub at: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Series {
    pub name: String,
    pub currency: String,
    pub exchange: String,
    pub bars: Vec<Bar>,
}
impl Series {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            max_text(&self.name, 8192)
                && max_text(&self.currency, 120)
                && max_text(&self.exchange, 8192)
                && !self.bars.is_empty()
                && self.bars.len() <= 25_000,
            "Invalid chart data"
        );
        anyhow::ensure!(
            self.bars.windows(2).all(|w| w[0].at < w[1].at)
                && self
                    .bars
                    .iter()
                    .all(|b| (-62_135_596_800..=253_402_300_799).contains(&b.at)
                        && [b.open, b.high, b.low, b.close, b.volume]
                            .iter()
                            .all(|v| v.is_finite() && v.abs() <= 1e100)
                        && b.volume >= 0.
                        && b.low <= b.high
                        && (b.low..=b.high).contains(&b.open)
                        && (b.low..=b.high).contains(&b.close)),
            "Malformed chart observations"
        );
        Ok(())
    }
    pub fn change_percent(&self) -> Option<f64> {
        let first = self.bars.first()?.close;
        if first <= 0. {
            return None;
        }
        let result = (self.bars.last()?.close / first - 1.) * 100.;
        result.is_finite().then_some(result)
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub api: u32,
    pub id: u64,
    pub action: String,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    #[serde(default)]
    pub settings: BTreeMap<String, serde_json::Value>,
}
// Version 2 always carries its settings object, including an empty one. Version
// 1 must retain its original wire shape without an unsupported settings field.
impl Serialize for Request {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let settings = self.api >= 2 || !self.settings.is_empty();
        let mut request = serializer.serialize_struct("Request", if settings { 5 } else { 4 })?;
        request.serialize_field("api", &self.api)?;
        request.serialize_field("id", &self.id)?;
        request.serialize_field("action", &self.action)?;
        request.serialize_field("values", &self.values)?;
        if settings {
            request.serialize_field("settings", &self.settings)?;
        }
        request.end()
    }
}
impl Request {
    /// Validate before dispatching domain work, including the complete frame
    /// budget. Invalid lifecycle requests cannot accidentally submit a draft.
    pub fn validate(&self) -> anyhow::Result<()> {
        use anyhow::ensure;
        ensure!(matches!(self.api, 1..=3), "Unsupported panel protocol");
        ensure!(valid_key(&self.action), "Invalid panel action");
        ensure!(
            self.api >= 2 || self.action != "settings",
            "Settings action requires panel protocol 2"
        );
        ensure!(
            self.values.len() <= 128
                && self
                    .values
                    .iter()
                    .all(|(k, v)| valid_key(k) && max_text(v, 8192)),
            "Invalid panel input"
        );
        ensure!(
            !lifecycle_action(&self.action) || self.values.is_empty(),
            "Lifecycle requests cannot submit input values"
        );
        ensure!(
            self.api >= 2 || self.settings.is_empty(),
            "Protocol 1 cannot receive host settings"
        );
        ensure!(
            serde_json::to_vec(&self.settings)?.len() <= SETTINGS_LIMIT,
            "Extension settings exceed their limit"
        );
        ensure!(
            serde_json::to_vec(self)?.len() < REQUEST_LIMIT,
            "Panel request exceeds 1 MiB including settings"
        );
        Ok(())
    }
}
impl Reply {
    /// Common admission rules for host receivers and SDK servers. Identity and
    /// protocol checks precede publication of a view or updated preferences.
    pub fn validate(&self, api: u32, id: u64) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.api == api && self.id == id,
            "Unexpected panel reply identity"
        );
        self.view.validate_version(api)?;
        if let Some(settings) = &self.settings {
            anyhow::ensure!(api >= 2, "Protocol 1 cannot write host settings");
            anyhow::ensure!(
                serde_json::to_vec(settings)?.len() <= SETTINGS_LIMIT,
                "Extension settings exceed their limit"
            );
        }
        Ok(())
    }
}
impl Action {
    pub fn new(id: &str, label: &str, enabled: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled,
            primary: false,
            confirm: None,
            selected: false,
        }
    }
}

/// Match native chart precision for microprices, normal quotes and large values.
pub fn format_price(value: f64) -> String {
    if value.abs() >= 1e9 || (value != 0. && value.abs() < 0.0001) {
        format!("{value:.2e}")
    } else if value != 0. && value.abs() < 0.01 {
        format!("{value:.6}")
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn composed_view() -> View {
        serde_json::from_value(serde_json::json!({
            "title":"Service overview",
            "fields":[{"key":"region","label":"Region","value":"EU"}],
            "metrics":[{"key":"uptime","label":"Uptime","value":"99.9%"}],
            "tables":[{"key":"events","title":"Events","columns":["Event"],"rows":[]}],
            "actions":[{"id":"refresh-data","label":"Refresh","enabled":true}],
            "links":[{"key":"docs","label":"Documentation","url":"https://example.com"}],
            "action_groups":[{"key":"ranges","title":"Range","actions":[{"id":"day","label":"Day","enabled":true}]}],
            "charts":[{"key":"latency","title":"Latency","series":{"name":"Latency","currency":"ms","exchange":"local","bars":[{"at":0,"open":1,"high":1,"low":1,"close":1,"volume":0}]}}],
            "sections":[{"key":"details","title":"Details","content":[{"kind":"fields","keys":["region"]},{"kind":"chart","key":"latency"},{"kind":"table","key":"events"},{"kind":"actions","keys":["refresh-data"]}]}],
            "content":[{"kind":"metrics","keys":["uptime"]},{"kind":"section","key":"details"},{"kind":"action_group","key":"ranges"},{"kind":"link","key":"docs"}]
        })).unwrap()
    }
    #[test]
    fn composed_layout_is_complete_keyed_and_protocol_three_only() {
        let view = composed_view();
        view.validate_version(3).unwrap();
        assert!(view.validate_version(2).is_err());
        for invalid in 0..8 {
            let mut view = view.clone();
            match invalid {
                0 => view.content.push(Content::Table {
                    key: "events".into(),
                }),
                1 => view.sections[0].content.push(Content::Table {
                    key: "unknown".into(),
                }),
                2 => {
                    view.content.pop();
                }
                3 => view.sections[0].content.push(Content::Section {
                    key: "details".into(),
                }),
                4 => view.tables[0].key.clear(),
                5 => view.sections[0].tables.push(0),
                6 => view.fields[0].inline = true,
                _ => view.content.push(Content::Actions { keys: vec![] }),
            }
            assert!(view.validate().is_err(), "case {invalid}");
        }
        // Display titles and storage order do not define identity.
        let mut renamed = view.clone();
        renamed.tables[0].title = "Renamed events".into();
        renamed.tables.insert(
            0,
            Table {
                key: "other".into(),
                title: "Events".into(),
                columns: vec!["Event".into()],
                rows: vec![],
            },
        );
        renamed.content.push(Content::Table {
            key: "other".into(),
        });
        renamed.validate_version(3).unwrap();
    }
    #[test]
    fn automatic_layout_retains_legacy_controls_without_ordered_content() {
        let view: View = serde_json::from_value(serde_json::json!({
            "title":"Legacy settings", "fields":[{"key":"region","label":"Region","value":"EU","inline":true}],
            "tables":[{"title":"Events","columns":["Event"],"rows":[]}],
            "sections":[{"title":"Details","tables":[0]}]
        })).unwrap();
        view.validate_version(2).unwrap();
        view.validate_version(3).unwrap();
        assert!(view.validate_version(1).is_err());
    }
    #[test]
    fn protocol_three_retains_settings_and_rejects_unknown_versions() {
        let request = Request {
            api: 3,
            id: 1,
            action: "open".into(),
            values: Default::default(),
            settings: BTreeMap::from([("region".into(), serde_json::json!("EU"))]),
        };
        request.validate().unwrap();
        let reply = Reply {
            api: 3,
            id: 1,
            view: composed_view(),
            settings: Some(request.settings.clone()),
        };
        reply.validate(3, 1).unwrap();
        assert!(reply.validate(2, 1).is_err());
        let mut bad = request;
        bad.api = 4;
        assert!(bad.validate().is_err());
    }
    #[test]
    fn layout_preferences_require_protocol_three() {
        let original = serde_json::json!({"title":"Legacy"});
        let legacy: View = serde_json::from_value(original).unwrap();
        legacy.validate_version(1).unwrap();
        assert!(
            serde_json::to_value(legacy)
                .unwrap()
                .get("layout")
                .is_none()
        );
        for density in ["auto", "compact", "comfortable"] {
            let value = serde_json::json!({"title":"Dashboard", "layout":{"metric_columns":4,"metric_density":density},
                "sections":[{"key":"details","title":"Details","collapsible":true,"default_open":false}]});
            let view: View = serde_json::from_value(value).unwrap();
            view.validate_version(3).unwrap();
            assert!(view.validate_version(2).is_err());
            assert!(view.validate_version(1).is_err());
            let mut duplicate = view.clone();
            duplicate.sections.push(view.sections[0].clone());
            assert!(duplicate.validate().is_err());
        }
        for columns in [0, 5] {
            let view: View = serde_json::from_value(
                serde_json::json!({"title":"Invalid","layout":{"metric_columns":columns}}),
            )
            .unwrap();
            assert!(view.validate().is_err());
        }
        assert!(
            serde_json::from_value::<View>(
                serde_json::json!({"title":"Invalid","layout":{"metric_density":"pixel-perfect"}})
            )
            .is_err()
        );
        let view: View = serde_json::from_value(serde_json::json!({"title":"Invalid","sections":[{"title":"Details","collapsible":true}]})).unwrap();
        assert!(view.validate().is_err());
    }
    #[test]
    fn sections_validate_unique_references_and_require_protocol_two() {
        let value = serde_json::json!({"title":"Dashboard",
            "fields":[{"key":"weight","label":"Project weight","value":"70"}],
            "tables":[{"title":"Projects","columns":["Project"],"rows":[["A"]]}],
            "actions":[{"id":"preview","label":"Preview","enabled":true}],
            "sections":[{"title":"Projects","fields":["weight"],"tables":[0],"actions":["preview"]}]});
        let valid: View = serde_json::from_value(value.clone()).unwrap();
        valid.validate_version(2).unwrap();
        assert!(valid.validate_version(1).is_err());
        for (key, bad) in [
            ("fields", serde_json::json!(["missing"])),
            ("tables", serde_json::json!([1])),
            ("actions", serde_json::json!(["missing"])),
            ("fields", serde_json::json!(["weight", "weight"])),
        ] {
            let mut invalid = value.clone();
            invalid["sections"][0][key] = bad;
            assert!(
                serde_json::from_value::<View>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let mut duplicate = valid.clone();
        duplicate.sections.push(valid.sections[0].clone());
        assert!(duplicate.validate().is_err());
    }
    #[test]
    fn charts_reject_invalid_values_order_ohlc_and_unbounded_histories() {
        let bar = Bar {
            at: 0,
            open: 1.,
            low: 0.,
            high: 2.,
            close: 1.,
            volume: 0.,
        };
        let mut series = Series {
            name: "Synthetic".into(),
            currency: "USD".into(),
            exchange: "Test".into(),
            bars: vec![bar],
        };
        series.validate().unwrap();
        series.bars[0].volume = f64::NAN;
        assert!(series.validate().is_err());
        series.bars[0] = bar;
        series.bars.push(bar);
        assert!(series.validate().is_err());
        series.bars = vec![bar];
        series.bars[0].close = 3.;
        assert!(series.validate().is_err());
        series.bars = vec![bar; 25_001];
        assert!(series.validate().is_err());
    }
    #[test]
    fn old_views_roundtrip_without_enabling_new_protocol_capabilities() {
        let original = serde_json::json!({"title":"Legacy","fields":[{"key":"draft","label":"Draft","value":""}],"actions":[{"id":"save","label":"Save","enabled":true}]});
        let view: View = serde_json::from_value(original).unwrap();
        view.validate().unwrap();
        let saved = serde_json::to_value(view).unwrap();
        assert!(saved.get("charts").is_none());
        assert!(saved.get("action_groups").is_none());
        assert!(saved["fields"][0].get("inline").is_none());
    }
    #[test]
    fn action_identity_and_input_submission_are_validated_across_groups() {
        let mut view:View=serde_json::from_value(serde_json::json!({"title":"Test","fields":[{"key":"query","label":"Query","value":"","submit":"search"}],"actions":[{"id":"search","label":"Search","enabled":true}]})).unwrap();
        view.validate().unwrap();
        view.action_groups.push(ActionGroup {
            key: String::new(),
            title: "Results".into(),
            actions: vec![Action::new("search", "Ambiguous action", true)],
        });
        assert!(view.validate().is_err());
        view.action_groups.clear();
        view.actions.clear();
        assert!(view.validate().is_err());
    }
}

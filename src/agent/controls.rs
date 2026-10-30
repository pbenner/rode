//! Bounded model catalogs and capability-driven agent selectors.
//!
//! # Design decisions
//!
//! Model IDs and ACP options come from the active process, never a hard-coded
//! vendor catalog. Unsupported controls are omitted. Discovery happens during
//! the first submitted conversation, and metadata is not persisted as settings.
//! Only explicit user choices are saved. Protocol updates are acknowledged
//! before a queued prompt is sent; they cannot expand the enclosing sandbox.

use serde_json::Value;

/// Maximum selectors/model entries retained from an agent.
pub const CONTROL_LIMIT: usize = 32;
/// A stable bounded identifier suitable for a saved option or model name.
pub(crate) fn identifier(value: &str, empty: bool) -> bool {
    (empty || !value.is_empty()) && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn text(value: &Value, fallback: &str, limit: usize) -> String {
    value
        .as_str()
        .unwrap_or(fallback)
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

/// An advertised model or reasoning option shown in a native selector.
#[derive(Clone, Debug)]
pub struct OptionValue {
    /// Literal value sent to the agent.
    pub id: String,
    /// Human-readable option name.
    pub name: String,
    /// Optional explanatory tooltip.
    pub description: String,
}
/// One Codex model with its advertised reasoning capabilities.
#[derive(Clone, Debug)]
pub struct Model {
    /// Model identity and presentation metadata.
    pub value: OptionValue,
    /// Advertised reasoning effort choices, including future backend-defined values.
    pub efforts: Vec<OptionValue>,
    /// Whether the backend marks this model as its default.
    pub default: bool,
}
/// Decode one bounded model-list page, excluding hidden/invalid entries.
pub(crate) fn models(result: &Value) -> Vec<Model> {
    result["data"]
        .as_array()
        .into_iter()
        .flatten()
        .take(128)
        .filter_map(|m| {
            let id = m["model"].as_str().or_else(|| m["id"].as_str())?;
            if !identifier(id, false) || m["hidden"] == true {
                return None;
            }
            let efforts = m["supportedReasoningEfforts"]
                .as_array()
                .into_iter()
                .flatten()
                .take(CONTROL_LIMIT)
                .filter_map(|e| {
                    let id = e["reasoningEffort"].as_str()?;
                    identifier(id, false).then(|| OptionValue {
                        id: id.into(),
                        name: id.into(),
                        description: text(&e["description"], "", 1024),
                    })
                })
                .collect();
            Some(Model {
                value: OptionValue {
                    id: id.into(),
                    name: text(&m["displayName"], id, 256),
                    description: text(&m["description"], "", 2048),
                },
                efforts,
                default: m["isDefault"] == true,
            })
        })
        .collect()
}

/// One supported ACP select/boolean capability, including current backend state.
#[derive(Clone, Debug)]
pub struct Control {
    /// Agent-defined option identifier (or the legacy mode selector identifier).
    pub id: String,
    /// Human-readable selector label.
    pub name: String,
    /// Semantic grouping: model, mode, thought_level, model_config or agent-defined.
    pub category: String,
    /// Explanatory tooltip supplied by the agent.
    pub description: String,
    /// Actual backend value; staged user choices are kept separately.
    pub current: Value,
    /// Allowed string values; empty for a boolean control.
    pub options: Vec<OptionValue>,
    /// Whether the control is boolean rather than a string selector.
    pub boolean: bool,
    /// Whether to use the legacy session/set_mode method.
    pub legacy: bool,
}
impl Control {
    /// Validate an explicit user selection against this session's current capabilities.
    pub fn accepts(&self, value: &Value) -> bool {
        if self.boolean {
            value.is_boolean()
        } else {
            value
                .as_str()
                .is_some_and(|v| self.options.iter().any(|o| o.id == v))
        }
    }
}

/// Decode supported modern ACP controls, or the stable legacy mode fallback.
pub(crate) fn acp(result: &Value) -> Vec<Control> {
    if let Some(values) = result["configOptions"].as_array().filter(|v| !v.is_empty()) {
        return values
            .iter()
            .take(CONTROL_LIMIT)
            .filter_map(|v| {
                let id = v["id"].as_str()?;
                if !identifier(id, false) {
                    return None;
                }
                let boolean = v["type"] == "boolean";
                if !boolean && v["type"] != "select" {
                    return None;
                }
                let mut options = vec![];
                for value in v["options"].as_array().into_iter().flatten().take(64) {
                    // ACP also supports named groups of select options.
                    let members = value["options"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or(std::slice::from_ref(value));
                    for item in members.iter().take(64) {
                        let Some(id) = item["value"].as_str().filter(|id| identifier(id, false))
                        else {
                            continue;
                        };
                        if options.len() >= 128 {
                            break;
                        }
                        options.push(OptionValue {
                            id: id.into(),
                            name: text(&item["name"], id, 256),
                            description: text(&item["description"], "", 2048),
                        });
                    }
                }
                let control = Control {
                    id: id.into(),
                    name: text(&v["name"], id, 256),
                    category: text(&v["category"], "", 128),
                    description: text(&v["description"], "", 2048),
                    current: v["currentValue"].clone(),
                    options,
                    boolean,
                    legacy: false,
                };
                control.accepts(&control.current).then_some(control)
            })
            .collect();
    }
    let modes = &result["modes"];
    let options: Vec<_> = modes["availableModes"]
        .as_array()
        .into_iter()
        .flatten()
        .take(128)
        .filter_map(|m| {
            let id = m["id"].as_str().filter(|id| identifier(id, false))?;
            Some(OptionValue {
                id: id.into(),
                name: text(&m["name"], id, 256),
                description: text(&m["description"], "", 2048),
            })
        })
        .collect();
    let control = Control {
        id: "rode:legacy-mode".into(),
        name: "Agent mode".into(),
        category: "mode".into(),
        description: "Operating modes advertised by this agent".into(),
        current: modes["currentModeId"].clone(),
        options,
        boolean: false,
        legacy: true,
    };
    if control.accepts(&control.current) {
        vec![control]
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_types_and_invalid_choices_never_become_controls() {
        let result = json!({"configOptions":[
            {"id":"secret","type":"text","currentValue":"not-a-selector"},
            {"id":"bad","type":"select","currentValue":"unlisted","options":[{"value":"listed","name":"Listed"}]},
            {"id":"model","name":"Model","type":"select","currentValue":"a","options":[{"group":"Models","options":[{"value":"a","name":"A"},{"value":"b","name":"B"}]}]},
            {"id":"automatic","type":"boolean","currentValue":false}
        ]});
        let parsed = acp(&result);
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].accepts(&json!("b")));
        assert!(!parsed[0].accepts(&json!("unlisted")));
        assert!(!parsed[1].accepts(&json!("true")));
        assert!(parsed[1].accepts(&json!(true)));
    }

    #[test]
    fn catalogs_bound_choices_and_reject_hidden_or_unusable_ids() {
        let catalog = models(&json!({"data":[
            {"model":"hidden","hidden":true},
            {"model":"bad\n"},
            {"model":"ok","displayName":"Model","supportedReasoningEfforts":[{"reasoningEffort":"future-effort"}]}]}));
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].efforts[0].id, "future-effort");
        let values: Vec<_> = (0..100)
            .map(|n| json!({"id":format!("option-{n}"),"type":"boolean","currentValue":false}))
            .collect();
        assert_eq!(acp(&json!({"configOptions":values})).len(), CONTROL_LIMIT);
    }
}

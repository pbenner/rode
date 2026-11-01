//! Parse only after an edit; keep the raw JSON while it is incomplete.
use crate::debugger;

#[derive(Default)]
/// Raw debugger JSON and its parsed form; incomplete text is retained with an error instead of discarded.
pub(super) struct Draft {
    pub text: String,
    pub parsed: Option<debugger::Config>,
    error: String,
}
impl Draft {
    /// Replace the draft from accepted settings and regenerate its formatted JSON text.
    pub fn load(&mut self, config: &debugger::Config) {
        self.parsed = Some(config.clone());
        self.reformat();
        self.error.clear();
    }
    /// Regenerate text only when a complete parsed configuration exists.
    pub fn reformat(&mut self) {
        if let Some(config) = &self.parsed {
            self.text = serde_json::to_string_pretty(config).unwrap_or_default();
        }
    }
    /// Parse after an edit while preserving raw text on failure; incomplete JSON clears the parsed value.
    pub fn parse(&mut self) {
        match serde_json::from_str(&self.text) {
            Ok(config) => {
                self.parsed = Some(config);
                self.error.clear();
            }
            Err(error) => {
                self.parsed = None;
                self.error = error.to_string();
            }
        }
    }
    /// Return the complete configuration only after JSON parsing and semantic validation both succeed.
    pub fn validated(&self) -> anyhow::Result<&debugger::Config> {
        let config = self
            .parsed
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Invalid debugger configuration: {}", self.error))?;
        config.validate()?;
        Ok(config)
    }
}

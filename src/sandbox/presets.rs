//! Reusable, user-owned sandbox policies. Paths stay literal until the project
//! host prepares a plan; reading a preset never probes or changes access.
use super::Options;
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;

pub(crate) const STORAGE_KEY: &str = "sandbox_presets";
/// Named sandbox option snapshots; labels are user-facing identities rather than filesystem paths.
pub(crate) type Presets = BTreeMap<String, Options>;

/// Validate names independently of filesystem conventions: these are labels,
/// not filenames, and may include spaces and Unicode.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name == name.trim()
            && name.len() <= 128
            && !name.chars().any(char::is_control),
        "Preset names must contain 1–128 bytes, with no surrounding whitespace or control characters."
    );
    Ok(())
}

/// Bounded typed decoding shared by the UI and cooperating preference writers.
/// Validate paths even for a preset saved in normal mode: enabling it later must
/// not turn an unchecked draft into a supposedly valid reusable policy.
pub(crate) fn decode(text: &str) -> Result<Presets> {
    ensure!(
        text.len() <= 2 * 1024 * 1024,
        "Sandbox presets exceed 2 MiB."
    );
    let presets: Presets = serde_json::from_str(text).context("Invalid sandbox presets")?;
    ensure!(
        presets.len() <= 64,
        "At most 64 sandbox presets are supported."
    );
    for (name, options) in &presets {
        validate_name(name)?;
        let mut enabled = options.clone();
        enabled.enabled = true;
        enabled
            .validate()
            .with_context(|| format!("Invalid sandbox preset '{name}'"))?;
    }
    Ok(presets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn presets_roundtrip_without_resolving_host_paths() {
        let presets = decode(
            &json!({"Remote toolchain": {
                "enabled": true, "backend": "landlock", "share_network": false,
                "read_only": ["~/.config/dev-shell", "/srv/site-tools"],
                "read_write": ["~/.cache/dev-tools"], "project_read_only": true,
                "overrides": {"~/.codex": "read_only"}
            }})
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            presets["Remote toolchain"].read_only[0],
            "~/.config/dev-shell"
        );
        assert_eq!(
            decode(&serde_json::to_string(&presets).unwrap()).unwrap(),
            presets
        );
    }

    #[test]
    fn invalid_names_limits_and_disabled_path_drafts_are_rejected() {
        for name in ["", " padded", "padded ", "line\nbreak", &"x".repeat(129)] {
            assert!(decode(&json!({name: {}}).to_string()).is_err());
        }
        assert!(
            decode(&json!({"Normal": {"enabled": false, "read_only": ["relative"]}}).to_string())
                .is_err()
        );
        let too_many: Presets = (0..65)
            .map(|i| (i.to_string(), Options::default()))
            .collect();
        assert!(decode(&serde_json::to_string(&too_many).unwrap()).is_err());
        assert!(decode(&format!("{{}}{}", " ".repeat(2 * 1024 * 1024))).is_err());
    }
}

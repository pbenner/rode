//! Runtime release pins shared by desktop selection and host cache deployment.
//!
//! Deserialize metadata, then validate versions and targets before deriving a
//! pathname or trusting an upload size. Artifact order and cache identity are a
//! compatibility contract: the main executable comes first, helpers follow in
//! sorted filename order, and legacy main-only targets retain their digest key.
//! Filesystem ownership, notice-name policies, downloads and publication belong
//! to the calling desktop installer or host cache, not to these data types.
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::collections::BTreeMap;

const MAX_EXECUTABLE_SIZE: u64 = 512 * 1024 * 1024;

/// Expected byte count and SHA-256 for one auxiliary runtime executable.
#[derive(Deserialize)]
pub struct Artifact {
    /// Exact executable length, from 1 byte through 512 MiB after validation.
    pub size: u64,
    /// Lowercase hexadecimal SHA-256 digest, checked before cache publication.
    pub sha256: String,
}

/// Main executable and optional helpers for one runtime architecture.
/// Deserialization does not validate pins; callers must use [`Self::validate`].
#[derive(Deserialize)]
pub struct Target {
    /// Exact main executable length, from 1 byte through 512 MiB after validation.
    pub size: u64,
    /// Lowercase hexadecimal SHA-256 digest of the main executable.
    pub sha256: String,
    /// Auxiliary executables keyed by their allowed runtime filenames.
    #[serde(default)]
    pub helpers: BTreeMap<String, Artifact>,
}

impl Target {
    /// Check executable sizes, digests, helper count and allowed helper names.
    /// This validates metadata, not the content or provenance of an executable.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_EXECUTABLE_SIZE).contains(&self.size) && valid_sha256(&self.sha256),
            "Invalid Codex executable pin"
        );
        ensure!(self.helpers.len() <= 8, "Too many Codex helper executables");
        for (name, pin) in &self.helpers {
            ensure!(
                matches!(name.as_str(), "codex-code-mode-host" | "codex-exec-server")
                    && (1..=MAX_EXECUTABLE_SIZE).contains(&pin.size)
                    && valid_sha256(&pin.sha256),
                "Invalid Codex helper pin"
            );
        }
        Ok(())
    }

    /// List transfer members as `(filename, exact bytes, SHA-256)` in wire order.
    /// The main executable is first; helpers follow sorted by filename.
    pub fn artifacts(&self) -> Vec<(&str, u64, &str)> {
        std::iter::once(("codex", self.size, self.sha256.as_str()))
            .chain(
                self.helpers
                    .iter()
                    .map(|(name, pin)| (name.as_str(), pin.size, pin.sha256.as_str())),
            )
            .collect()
    }

    /// Derive the immutable directory key for a previously validated target.
    /// Main-only targets keep the legacy digest key. Targets with helpers hash
    /// the ordered artifact metadata so missing/changed helpers select a new cache.
    pub fn cache_key(&self) -> Result<String> {
        if self.helpers.is_empty() {
            Ok(self.sha256.clone())
        } else {
            Ok(crate::files::digest(&serde_json::to_vec(
                &self.artifacts(),
            )?))
        }
    }
}

/// Required license/notice content shipped with a runtime release.
#[derive(Deserialize)]
pub struct Notice {
    /// Runtime-relative filename; each caller applies its supported name policy.
    pub file: String,
    /// Expected lowercase hexadecimal SHA-256 digest of the notice bytes.
    pub sha256: String,
}

/// Whether a pin is exactly 64 lowercase hexadecimal SHA-256 characters.
pub fn valid_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Require a bounded, path-safe runtime version beginning with an ASCII digit.
/// Versions contain at most 64 ASCII alphanumeric characters or `.`, `+`, `-`.
pub fn validate_version(version: &str) -> Result<()> {
    ensure!(
        !version.is_empty()
            && version.len() <= 64
            && version.as_bytes()[0].is_ascii_digit()
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b)),
        "Invalid Codex runtime version"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target(helpers: bool) -> Target {
        let helpers = if helpers {
            json!({
                "codex-exec-server": {"size":3,"sha256":"c".repeat(64)},
                "codex-code-mode-host": {"size":2,"sha256":"b".repeat(64)},
            })
        } else {
            json!({})
        };
        serde_json::from_value(json!({"size":1,"sha256":"a".repeat(64),"helpers":helpers})).unwrap()
    }

    #[test]
    fn cache_identity_preserves_legacy_and_ordered_multi_executable_paths() {
        let single = target(false);
        single.validate().unwrap();
        assert_eq!(single.cache_key().unwrap(), "a".repeat(64));
        let mut multiple = target(true);
        multiple.validate().unwrap();
        assert_eq!(
            multiple
                .artifacts()
                .iter()
                .map(|(name, _, _)| *name)
                .collect::<Vec<_>>(),
            ["codex", "codex-code-mode-host", "codex-exec-server"]
        );
        // Existing desktop/helper cache identity; changing serialization or
        // transfer order must not silently invalidate deployed runtimes.
        assert_eq!(
            multiple.cache_key().unwrap(),
            "e0e15a0cdf1000dbff809ae7c512dc2dc1a013f08380a1cb1c446335fb2474ef"
        );
        multiple.helpers.get_mut("codex-exec-server").unwrap().size = 4;
        assert_ne!(
            multiple.cache_key().unwrap(),
            target(true).cache_key().unwrap()
        );
    }

    #[test]
    fn pins_and_versions_reject_invalid_paths_sizes_and_digests() {
        for size in [0, MAX_EXECUTABLE_SIZE + 1] {
            let mut pin = target(false);
            pin.size = size;
            assert!(pin.validate().is_err());
        }
        for hash in ["a".repeat(63), "A".repeat(64), "g".repeat(64)] {
            let mut pin = target(false);
            pin.sha256 = hash;
            assert!(pin.validate().is_err());
        }
        for name in ["../helper", "arbitrary-helper", "codex"] {
            let mut pin = target(true);
            let helper = pin.helpers.remove("codex-exec-server").unwrap();
            pin.helpers.insert(name.into(), helper);
            assert!(pin.validate().is_err());
        }
        let mut pin = target(true);
        pin.helpers.get_mut("codex-exec-server").unwrap().size = 0;
        assert!(pin.validate().is_err());
        for version in ["", "../1", "1/2", "1\\2", "v1", "1\0"] {
            assert!(validate_version(version).is_err(), "{version:?}");
        }
        validate_version("0.166.0-rc.1+build").unwrap();
    }
}

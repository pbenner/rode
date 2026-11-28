//! Compatibility of independently installed helpers. Release labels are diagnostic;
//! the API revision covers all helper entry points, including non-RPC streams.
use serde::{Deserialize, Serialize};

/// Increment for incompatible CLI, shared request/schema or stream changes not
/// already covered by the host framing or persistent-session protocol versions.
pub const API: u32 = 1;
/// Bounded machine-readable discovery response, independent of RPC framing.
pub const INFO_FORMAT: u32 = 1;
/// Maximum discovery response size.
pub const MAX_INFO: usize = 16 * 1024;
/// Maximum executable size accepted by deployment and identity checks.
pub const MAX_EXECUTABLE: u64 = 24 * 1024 * 1024;
/// Required host functionality; additive capabilities remain compatible.
pub const CAPABILITIES: &[&str] = &[
    "files",
    "project_tools",
    // Old installed helpers would still require Node/Prettier for these files.
    "bundled_web_formatters",
    "git_memory_auth",
    "contexts",
    "sessions",
    "native_landlock",
    // Resolver targets, login profiles and command lookup defaults must match
    // the desktop's path controls, even when an older helper is preinstalled.
    "sandbox_system_lookups",
    // Device binds must remain device-enabled, optional and evaluated on each execution host.
    "sandbox_devices",
    // Notebook plots use bounded fragments and a separate raster/text retention policy.
    "notebook_rich_output",
    "native_mise",
    "shared_compute_helper",
    "process",
    "debugger",
    "vaults",
    "latex",
    "marp",
    "extensions",
    "runtime_cache",
    "agent_files",
    "agent_offline_tools",
    "agent_private_state",
    "agent_async_questions",
    "agent_partial_answers",
    // History transfers keep the live reply budget and stderr is streamed rather
    // than treating an oversized diagnostic line as a fatal protocol violation.
    "agent_streaming_output",
    "bundled_bwrap",
    "compact_sandbox_arguments",
    "explorer_mutations",
    "large_document_transfer",
    "streaming_file_transfers",
    "shared_folders",
    "agent_undo",
    "connection_heartbeats",
    // Desktop heartbeats must reach the agent bridge even if an intermediate
    // stdio supervisor survives the original desktop process.
    "agent_attachment_leases",
    // Desktop owner metadata and generation-checked takeover bridge arguments.
    "agent_takeover",
    "agent_auto_persist",
    "private_home_tmp",
    // Encrypted mounts must share the desktop's host-scoped private temp root.
    "private_vault_tmp",
    // Lock checks use mount IDs without requiring ptrace access to every process.
    "vault_mount_table_scan",
];

/// Metadata repeated in the live handshake after discovery and cache publication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    /// Discovery metadata schema version.
    pub format: u32,
    /// Human-readable package version, not a substitute for protocol compatibility.
    pub version: String,
    /// Compatibility revision of the complete helper API.
    pub api: u32,
    /// Host RPC protocol version.
    pub host_protocol: u32,
    /// Persistent-session protocol version.
    pub session_protocol: u32,
    /// Native operating system label.
    pub os: String,
    /// Native executable architecture label.
    pub architecture: String,
    /// Implemented operations and auxiliary helper entry points.
    pub capabilities: Vec<String>,
}

impl Info {
    /// Describe this build without executing any host operation.
    pub fn current(version: &str, session_protocol: u32) -> Self {
        Self {
            format: INFO_FORMAT,
            version: version.into(),
            api: API,
            host_protocol: crate::framing::VERSION,
            session_protocol,
            os: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            capabilities: CAPABILITIES.iter().map(|s| (*s).into()).collect(),
        }
    }

    /// Check the complete contract before admitting any operation. Different
    /// package releases may interoperate when these explicit versions agree.
    pub fn validate(
        &self,
        os: &str,
        architecture: &str,
        session_protocol: u32,
    ) -> Result<(), String> {
        if self.version.is_empty()
            || self.version.len() > 64
            || self.version.chars().any(char::is_control)
            || self.capabilities.len() > 64
            || self
                .capabilities
                .iter()
                .any(|s| s.len() > 64 || s.chars().any(char::is_control))
        {
            return Err("Malformed native helper metadata".into());
        }
        if self.format != INFO_FORMAT
            || self.api != API
            || self.host_protocol != crate::framing::VERSION
            || self.session_protocol != session_protocol
        {
            return Err(format!(
                "Incompatible native helper {}: metadata/API/host/session versions {}/{}/{}/{}, expected {}/{}/{}/{}. Install a compatible rode-sessiond or clear the remote helper override to allow deployment.",
                self.version,
                self.format,
                self.api,
                self.host_protocol,
                self.session_protocol,
                INFO_FORMAT,
                API,
                crate::framing::VERSION,
                session_protocol
            ));
        }
        if self.os != os || self.architecture != architecture {
            return Err(format!(
                "Native helper {} targets {}/{}, expected {os}/{architecture}",
                self.version, self.os, self.architecture
            ));
        }
        for capability in CAPABILITIES {
            if !self.capabilities.iter().any(|s| s == capability) {
                return Err(format!(
                    "Native helper {} lacks required capability '{capability}'",
                    self.version
                ));
            }
        }
        Ok(())
    }
}

/// Read-only discovery result. The executable digest is checked again when the
/// remote copy is published and when the gateway starts.
#[derive(Debug, Serialize, Deserialize)]
pub struct Installed {
    /// Canonical absolute path of the executable that produced this response.
    pub executable: String,
    /// SHA-256 of its bytes.
    pub digest: String,
    /// Exact executable size.
    pub size: u64,
    /// Version and feature contract.
    pub info: Info,
}

impl Installed {
    /// Validate bounded deployment arguments and the helper's compatibility contract.
    pub fn validate(
        &self,
        os: &str,
        architecture: &str,
        session_protocol: u32,
    ) -> Result<(), String> {
        if !self.executable.starts_with('/')
            || self.executable.len() > 4096
            || self.executable.chars().any(char::is_control)
            || !(20..=MAX_EXECUTABLE).contains(&self.size)
            || self.digest.len() != 64
            || !self.digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("Malformed installed helper identity".into());
        }
        self.info.validate(os, architecture, session_protocol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_with_small_notebook_output_limits_require_an_upgrade() {
        let mut info = Info::current("test", 27);
        info.capabilities
            .retain(|cap| cap != "notebook_rich_output");
        assert!(
            info.validate(std::env::consts::OS, std::env::consts::ARCH, 27)
                .unwrap_err()
                .contains("notebook_rich_output")
        );
    }

    #[test]
    fn helpers_without_optional_device_grants_require_an_upgrade() {
        let mut info = Info::current("test", 11);
        info.capabilities.retain(|cap| cap != "sandbox_devices");
        assert!(
            info.validate(std::env::consts::OS, std::env::consts::ARCH, 11)
                .unwrap_err()
                .contains("sandbox_devices")
        );
    }

    #[test]
    fn helpers_requiring_ptrace_for_vault_lock_checks_require_an_upgrade() {
        let mut info = Info::current("test", 11);
        info.capabilities
            .retain(|cap| cap != "vault_mount_table_scan");
        assert!(
            info.validate(std::env::consts::OS, std::env::consts::ARCH, 11)
                .unwrap_err()
                .contains("vault_mount_table_scan")
        );
    }

    #[test]
    fn helpers_with_public_temporary_vault_mounts_require_an_upgrade() {
        let mut info = Info::current("test", 11);
        info.capabilities.retain(|cap| cap != "private_vault_tmp");
        assert!(
            info.validate(std::env::consts::OS, std::env::consts::ARCH, 11)
                .unwrap_err()
                .contains("private_vault_tmp")
        );
    }

    #[test]
    fn helpers_without_streaming_agent_output_require_an_upgrade() {
        let mut info = Info::current("test", 11);
        info.capabilities
            .retain(|capability| capability != "agent_streaming_output");
        assert!(
            info.validate(std::env::consts::OS, std::env::consts::ARCH, 11)
                .unwrap_err()
                .contains("agent_streaming_output")
        );
    }

    #[test]
    fn helpers_without_private_home_temporary_storage_require_an_upgrade() {
        let mut info = Info::current("test", 11);
        info.capabilities
            .retain(|capability| capability != "private_home_tmp");
        assert!(
            info.validate(std::env::consts::OS, std::env::consts::ARCH, 11)
                .unwrap_err()
                .contains("private_home_tmp")
        );
    }

    #[test]
    fn compatibility_uses_explicit_contracts_not_release_labels() {
        let mut info = Info::current("0.1.0", 11);
        info.version = "0.2.3-custom".into();
        info.capabilities.push("future_optional_operation".into());
        assert!(
            info.validate(std::env::consts::OS, std::env::consts::ARCH, 11)
                .is_ok()
        );
        let mut old = info.clone();
        old.capabilities
            .retain(|capability| capability != "agent_private_state");
        assert!(
            old.validate(std::env::consts::OS, std::env::consts::ARCH, 11)
                .unwrap_err()
                .contains("agent_private_state")
        );
        for field in [
            "format",
            "api",
            "host_protocol",
            "session_protocol",
            "os",
            "architecture",
            "capabilities",
        ] {
            let mut value = serde_json::to_value(&info).unwrap();
            value[field] = match field {
                "os" | "architecture" => "wrong".into(),
                "capabilities" => serde_json::json!([]),
                _ => 999.into(),
            };
            let incompatible: Info = serde_json::from_value(value).unwrap();
            assert!(
                incompatible
                    .validate(std::env::consts::OS, std::env::consts::ARCH, 11)
                    .is_err(),
                "Accepted incompatible {field}"
            );
        }
    }

    #[test]
    fn deployment_identity_rejects_unsafe_paths_hashes_and_sizes() {
        let good = Installed {
            executable: "/opt/rode build/rode-sessiond".into(),
            digest: "a".repeat(64),
            size: 1024,
            info: Info::current("0.1.0", 11),
        };
        assert!(
            good.validate(std::env::consts::OS, std::env::consts::ARCH, 11)
                .is_ok()
        );
        for (field, bad) in [
            ("executable", serde_json::json!("relative/helper")),
            ("executable", serde_json::json!("/tmp/helper\nextra")),
            ("digest", serde_json::json!("; touch marker")),
            ("size", serde_json::json!(MAX_EXECUTABLE + 1)),
            ("size", serde_json::json!(0)),
        ] {
            let mut value = serde_json::to_value(&good).unwrap();
            value[field] = bad;
            let invalid: Installed = serde_json::from_value(value).unwrap();
            assert!(
                invalid
                    .validate(std::env::consts::OS, std::env::consts::ARCH, 11)
                    .is_err()
            );
        }
    }

    #[test]
    fn helpers_with_old_sandbox_defaults_require_an_upgrade() {
        let mut info = Info::current("0.1.0", 13);
        info.capabilities
            .retain(|cap| cap != "sandbox_system_lookups");
        let error = info
            .validate(std::env::consts::OS, std::env::consts::ARCH, 13)
            .unwrap_err();
        assert!(error.contains("sandbox_system_lookups"), "{error}");
    }

    #[test]
    fn helpers_without_connection_expiry_require_an_upgrade() {
        let mut info = Info::current("0.1.0", 20);
        info.capabilities
            .retain(|cap| cap != "connection_heartbeats");
        let error = info
            .validate(std::env::consts::OS, std::env::consts::ARCH, 20)
            .unwrap_err();
        assert!(error.contains("connection_heartbeats"), "{error}");
    }

    #[test]
    fn helpers_without_takeover_bridge_arguments_require_an_upgrade() {
        let mut info = Info::current("test", 26);
        info.capabilities.retain(|cap| cap != "agent_takeover");
        let error = info.validate(&info.os, &info.architecture, 26).unwrap_err();
        assert!(error.contains("agent_takeover"), "{error}");
    }

    #[test]
    fn helpers_without_agent_attachment_leases_require_an_upgrade() {
        let mut info = Info::current("0.1.0", 20);
        info.capabilities
            .retain(|cap| cap != "agent_attachment_leases");
        let error = info
            .validate(std::env::consts::OS, std::env::consts::ARCH, 20)
            .unwrap_err();
        assert!(error.contains("agent_attachment_leases"), "{error}");
    }

    #[test]
    fn helpers_without_native_formatters_require_an_upgrade() {
        let mut info = Info::current("0.1.0", 20);
        info.capabilities
            .retain(|cap| cap != "bundled_web_formatters");
        let error = info
            .validate(std::env::consts::OS, std::env::consts::ARCH, 20)
            .unwrap_err();
        assert!(error.contains("bundled_web_formatters"), "{error}");
    }
}

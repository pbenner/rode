//! Host-bound, reusable reverse-tunnel settings stored in desktop preferences.
//! Records contain paths and public relay-key pins, never private key material.
use super::Profile;
use anyhow::{Context, Result, ensure};
use rode_session::tunnels::Spec;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Structured preference map, merged by record and field across editor windows.
pub const STORAGE_KEY: &str = "tunnel_profiles";

/// Computer/account on which a tunnel's paths and loopback destination apply.
/// Desktop routing, project folders and transport choices do not change it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Host {
    /// A local service on this named computer.
    Local {
        /// Hostname prevents shared desktop preferences crossing local machines.
        hostname: String,
    },
    /// An authenticated SSH destination, retaining its original configuration identity.
    Ssh {
        /// Original hostname or SSH configuration alias, without an embedded user.
        host: String,
        /// Explicit login; empty retains SSH configuration's default.
        user: String,
        /// Original port override; empty retains SSH configuration's default.
        port: String,
        /// Desktop configuration file used to interpret the alias.
        config: String,
    },
}
impl Host {
    /// Capture host identity without connecting, reading SSH files or resolving aliases.
    pub fn for_profile(profile: Option<&Profile>) -> Self {
        match profile {
            None => Self::Local {
                hostname: rode_session::tunnels::local_hostname().unwrap_or_default(),
            },
            Some(profile) => {
                let (embedded_user, host) =
                    profile.host.rsplit_once('@').unwrap_or(("", &profile.host));
                Self::Ssh {
                    host: host.into(),
                    user: if profile.user.is_empty() {
                        embedded_user
                    } else {
                        &profile.user
                    }
                    .into(),
                    port: profile.port.clone(),
                    config: profile.config.clone(),
                }
            }
        }
    }

    /// Host/account label used to explain where saved settings can be reopened.
    pub fn label(&self) -> String {
        match self {
            Self::Local { hostname } => format!("Local · {hostname}"),
            Self::Ssh {
                host,
                user,
                port,
                config,
            } => {
                let host = if host.contains(':') && !host.starts_with('[') {
                    format!("[{host}]")
                } else {
                    host.clone()
                };
                format!(
                    "SSH · {}{host}{}{}",
                    if user.is_empty() {
                        String::new()
                    } else {
                        format!("{user}@")
                    },
                    if port.is_empty() {
                        String::new()
                    } else {
                        format!(":{port}")
                    },
                    if config.is_empty() {
                        String::new()
                    } else {
                        format!(" · {config}")
                    }
                )
            }
        }
    }

    fn validate(&self) -> Result<()> {
        let fields = match self {
            Self::Local { hostname } => {
                ensure!(
                    !hostname.is_empty(),
                    "Cannot save a local tunnel without this computer's hostname"
                );
                vec![hostname]
            }
            Self::Ssh {
                host,
                user,
                port,
                config,
            } => {
                ensure!(!host.is_empty(), "Saved tunnel is missing its SSH host");
                ensure!(
                    port.is_empty() || port.parse::<u16>().is_ok_and(|port| port > 0),
                    "Invalid saved tunnel host port"
                );
                vec![host, user, port, config]
            }
        };
        ensure!(
            fields
                .iter()
                .all(|value| value.len() <= 4096 && !value.chars().any(char::is_control)),
            "Invalid saved tunnel host"
        );
        Ok(())
    }
}

/// One saved setup; loading it changes a draft and never starts a tunnel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// Host that owns the tunnel and its credential paths.
    pub host: Host,
    /// Tunnel settings, including any explicitly verified relay public key.
    pub spec: Spec,
    /// Optional desktop return-route alias/jump chain.
    #[serde(default)]
    pub relay_jump: String,
}

/// Stable record IDs allow renames without replacing another saved setup.
pub type Profiles = BTreeMap<String, Saved>;

/// Validate bounded saved data before loading or publishing a preference edit.
pub fn decode(text: &str) -> Result<Profiles> {
    ensure!(text.len() <= 1024 * 1024, "Saved tunnels exceed 1 MiB");
    let profiles: Profiles =
        serde_json::from_str(text).context("Invalid saved tunnels; existing data was preserved")?;
    ensure!(
        profiles.len() <= 128,
        "At most 128 saved tunnels are supported"
    );
    let mut names = HashSet::new();
    for (id, profile) in &profiles {
        ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            "Invalid saved tunnel ID"
        );
        profile.host.validate()?;
        profile.spec.validate()?;
        ensure!(
            profile.relay_jump.len() <= 4096
                && !profile
                    .relay_jump
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace()),
            "Invalid saved tunnel return route"
        );
        ensure!(
            names.insert((&profile.host, &profile.spec.name)),
            "A tunnel named {:?} is already saved for {}",
            profile.spec.name,
            profile.host.label()
        );
    }
    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_binding_survives_project_and_transport_changes_but_keeps_login_identity() {
        let profile = Profile {
            host: "researcher@compute".into(),
            port: "2222".into(),
            config: "~/.ssh/site-config".into(),
            ..Default::default()
        };
        let host = Host::for_profile(Some(&profile));
        let mut routed = profile.clone();
        routed.host = "compute".into();
        routed.user = "researcher".into();
        routed.name = "Compute through relay".into();
        routed.root = "/different/project".into();
        routed.backend = super::super::Backend::OpenSsh;
        routed.jump = "relay.example.org".into();
        routed.tunnel = Some(Box::new(super::super::TunnelRoute {
            port: 22022,
            host_key_alias: "[compute]:2222".into(),
        }));
        assert_eq!(Host::for_profile(Some(&routed)), host);
        assert_ne!(Host::for_profile(None), host);
        assert_eq!(
            host.label(),
            "SSH · researcher@compute:2222 · ~/.ssh/site-config"
        );
        routed.user = "other-account".into();
        assert_ne!(Host::for_profile(Some(&routed)), host);
    }
}

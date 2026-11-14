//! Connection identity is recovery metadata, independent of transport or file grants.
use crate::ssh::Profile;
use std::sync::Arc;

/// Comparable host/project configuration captured for disconnect recovery matching.
/// This is metadata, not proof of authentication or a grant of file-I/O authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct ConnectionId(Arc<str>);
impl ConnectionId {
    /// Display labels and sandbox choices do not change the host/project identity.
    pub(in crate::app) fn for_profile(profile: &Profile) -> Self {
        Self(Arc::from(
            serde_json::to_string(&(
                &profile.host,
                &profile.user,
                &profile.port,
                &profile.identity,
                &profile.config,
                &profile.jump,
                &profile.root,
            ))
            .expect("SSH profile strings serialize"),
        ))
    }
}

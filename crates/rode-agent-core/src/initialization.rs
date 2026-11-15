//! Initialization payloads shared by conversation and read-only metadata clients.
//! Role-specific feature negotiation stays explicit; this module starts no
//! transport and grants no executable or filesystem tools.
use crate::Provider;
use serde_json::{Value, json};

/// Connection purpose, retaining the existing protocol capability differences.
#[derive(Clone, Copy)]
pub(crate) enum Role {
    Conversation,
    History,
    Catalog,
    PromptHistory,
}

/// Build the initialize parameters without changing a caller's handshake state.
pub(crate) fn parameters(provider: Provider, role: Role) -> Value {
    let title = match role {
        Role::PromptHistory => "RODE prompt history",
        _ => "RODE",
    };
    let client = json!({"name":"rode", "title":title, "version":env!("CARGO_PKG_VERSION")});
    if provider.is_acp() {
        json!({"protocolVersion":1,"clientInfo":client,
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},
                "terminal":false,"session":{"configOptions":{"boolean":true}}}})
    } else {
        let mut params = json!({"clientInfo":client});
        if matches!(role, Role::Conversation | Role::PromptHistory) {
            params["capabilities"] = json!({"experimentalApi":true});
        }
        params
    }
}

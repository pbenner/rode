//! Shared extension contracts. No editor, GUI, SSH or application dependencies.
//! Protocol 1 remains supported; protocol 2 adds shared settings, action groups
//! and native interactive charts. Executables own domain logic and may be built
//! and replaced independently of their host.
pub mod panel;
pub mod server;

pub mod manifest;
pub mod tool;

/// Maximum serialized settings object per extension, shared by both contribution types.
pub const SETTINGS_LIMIT: usize = 192 * 1024;

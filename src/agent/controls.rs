//! Shared headless agent controls.
pub use rode_agent_core::controls::*;

/// Discover models through the same project-host transport used for chat/history.
pub fn discover(launch: super::Launch, wake: std::sync::Arc<dyn Fn() + Send + Sync>) -> Catalog {
    Catalog::open(launch.core(), wake)
}

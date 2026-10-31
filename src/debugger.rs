//! Project debugging through installed DAP adapters and host-owned controllers.
const REQUEST_LIMIT: usize = 256;
pub mod bridge;
pub mod framing;
pub mod protocol;
pub use rode_runtime_core::debugger::{
    Backend, Breakpoint, Command, Config, Event, Settings, Start, expand, replace,
};

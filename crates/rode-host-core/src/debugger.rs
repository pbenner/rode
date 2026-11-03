//! Native debugger host coordinator.
pub mod bridge;
pub mod framing;
pub use rode_runtime_core::debugger::{Backend, Command, Event, Settings, Start, expand, replace};

const REQUEST_LIMIT: usize = 256;

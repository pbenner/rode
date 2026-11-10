//! Native host services, independent of GUI and persistent session ownership.
#![warn(missing_docs)]
pub mod agent_commands;
pub mod agent_files;
/// Pinned bubblewrap fallback available through the native helper's early dispatch.
pub use rode_bwrap as bwrap;
pub mod command;
pub mod connection;
mod coordinator;
pub mod debugger;
pub mod environment;
pub mod extensions;
pub mod files;
pub mod landlock;
pub mod languages;
pub mod latex;
pub mod locking;
pub mod marp;
pub mod mise;
pub mod paths;
pub mod persistence;
pub mod project;
pub mod runtime_cache;
pub mod sandbox;
pub mod slurm;
pub mod supervisor;
pub mod vault;

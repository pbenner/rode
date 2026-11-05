//! Native host services, independent of GUI and persistent session ownership.
#![warn(missing_docs)]
pub mod agent_commands;
pub mod agent_files;
pub mod command;
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
pub mod persistence;
pub mod project;
pub mod runtime_cache;
pub mod sandbox;
pub mod slurm;
pub mod supervisor;
pub mod vault;

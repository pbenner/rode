//! Native editor services shared by the desktop UI and project-host helpers.
//!
//! [`app`] owns UI state and accepts versioned worker results. [`files`],
//! [`sandbox`] and [`ssh`] define host-aware I/O; [`process`] supervises bounded
//! tool protocols. [`persistence`] merges explicit preferences across windows.
//! [`rode_session`] owns persistent PTYs and agent controllers independently of
//! desktop lifetimes; [`rode_agent_core`] keeps their protocol logic headless.
//!
//! Blocking service entry points must run on workers or inside helper processes.
//! A request's paths belong to its execution host; an active sandbox must never
//! be silently bypassed on failure. See the repository's development and
//! architecture guides for ownership, protocols and validation commands.
//!
//! # Design decisions
//!
//! The native egui UI and Rust services keep the editor independent of a browser
//! application runtime. Slow work crosses explicit worker/process boundaries so
//! idle frames and ordinary editing do not depend on project-wide background
//! indexing. This favors bounded, on-demand tools over automatically activating
//! every installed capability.
//!
//! Local and SSH services share headless Rust host algorithms and policy
//! rules. The native gateway is connection-owned; the persistent service remains
//! separate because its jobs must outlive GUI/SSH attachments.
#![warn(missing_docs)]

pub mod agent;
pub mod app;
pub mod binary;
pub mod bundled;
pub mod debugger;
pub mod environment;
pub mod extensions;
pub mod files;
mod host;
pub mod images;
pub mod languages;
pub mod latex;
pub mod marp;
pub mod mise;
pub mod notebook;
pub mod pdf;
pub mod persistence;
pub mod process;
pub mod project;
pub mod runtime;
pub mod sandbox;
pub mod sessions;
pub mod slurm;
pub mod ssh;
pub mod terminal;
pub mod theme;
pub mod vault;

//! Headless notebook and debugger state shared by the desktop and host service.
#![warn(missing_docs)]
pub mod debugger;
mod json;
pub use json::json_size;
pub mod notebook;
pub mod process;
pub mod service;

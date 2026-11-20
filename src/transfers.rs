//! File-to-file copies over a captured SSH connection. UI state and project
//! selection are deliberately absent; one worker owns each stream and stage.
mod transport;
pub(crate) use transport::Endpoint;
pub(crate) mod copy;

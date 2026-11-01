//! Window-owned cleanup survives notebook reparsing, reload and tab removal.
use crate::notebook::kernel::{Event, Kernel};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
/// Shared window-level ownership of kernels whose explicit host shutdown is still pending.
pub(in crate::app) struct Shutdowns(Arc<Mutex<Vec<Kernel>>>);
impl Shutdowns {
    /// Request termination and retain host-owned kernels until their lifetime reply is drained.
    pub fn stop(&self, mut kernel: Kernel) {
        kernel.stop();
        if kernel.host_owned() {
            self.0.lock().unwrap().push(kernel);
        }
    }
    /// Report cleanup ownership that must keep normal application exit waiting.
    pub fn pending(&self) -> bool {
        !self.0.lock().unwrap().is_empty()
    }
    /// Drain cleanup events, release acknowledged kernels and return errors after the tab has gone.
    pub fn poll(&self) -> Vec<String> {
        let mut errors = vec![];
        self.0.lock().unwrap().retain_mut(|kernel| {
            for event in kernel.poll() {
                if let Event::Error(error) = event {
                    errors.push(error);
                }
            }
            kernel.pending()
        });
        errors
    }
}

#[cfg(test)]
mod tests;

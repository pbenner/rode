//! Window-owned cleanup survives notebook reparsing, reload and tab removal.
use crate::notebook::kernel::{Event, Kernel};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(in crate::app) struct Shutdowns(Arc<Mutex<Vec<Kernel>>>);
impl Shutdowns {
    pub fn stop(&self, mut kernel: Kernel) {
        kernel.stop();
        if kernel.host_owned() {
            self.0.lock().unwrap().push(kernel);
        }
    }
    pub fn pending(&self) -> bool {
        !self.0.lock().unwrap().is_empty()
    }
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

//! Standalone host-side terminal helper; delegates the bounded CLI/socket
//! protocol to the same library embedded by the desktop executable.

fn main() {
    // SAFETY: main has not initialized any service, runtime or worker threads.
    unsafe { rode_session::helper_main() }
}

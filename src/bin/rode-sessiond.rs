//! Headless terminal helper installed alongside the editor by `cargo install`.
//!
//! This frontend calls only the shared PTY library: it neither initializes the
//! GUI nor links desktop code. The library package retains an opt-in standalone
//! target for building the same helper with musl without desktop dependencies.

fn main() {
    // SAFETY: main has not initialized any service, runtime or worker threads.
    unsafe { rode_session::helper_main() }
}

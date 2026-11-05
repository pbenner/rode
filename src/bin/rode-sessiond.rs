//! Headless terminal helper installed alongside the editor by `cargo install`.
//!
//! This frontend calls only the shared PTY library: it neither initializes the
//! GUI nor links desktop code. The library package retains an opt-in standalone
//! target for building the same helper with musl without desktop dependencies.

fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().is_some_and(|arg| arg == "--bwrap-launch") {
        // SAFETY: Dispatch occurs before any service or worker threads exist.
        if let Err(error) = unsafe { rode_host_core::bwrap::launch(&arguments[1..]) } {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = rode_session::cli(&arguments) {
        rode_session::report_error(&arguments, &error);
        std::process::exit(1);
    }
}

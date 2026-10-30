//! Headless terminal helper installed alongside the editor by `cargo install`.
//!
//! This frontend calls only the shared PTY library: it neither initializes the
//! GUI nor links desktop code. The library package retains an opt-in standalone
//! target for building the same helper with musl without desktop dependencies.

fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let Err(error) = rode_session::cli(&arguments) {
        rode_session::report_error(&arguments, &error);
        std::process::exit(1);
    }
}

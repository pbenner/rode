//! Standalone host-side terminal helper; delegates the bounded CLI/socket
//! protocol to the same library embedded by the desktop executable.

fn main() {
    if let Err(error) = rode_session::cli(&std::env::args().skip(1).collect::<Vec<_>>()) {
        rode_session::report_error(&std::env::args().skip(1).collect::<Vec<_>>(), &error);
        std::process::exit(1);
    }
}

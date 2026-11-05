//! Standalone host-side terminal helper; delegates the bounded CLI/socket
//! protocol to the same library embedded by the desktop executable.

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

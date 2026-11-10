//! One early helper dispatch path for both installed and standalone frontends.
//! Keep bootstrap dispatch here so the two packaging targets cannot drift.

/// Run the headless helper's process entry point, reporting fatal CLI failures.
///
/// Both the Cargo-installed frontend and the standalone/static helper use this
/// entry point. It reads process arguments and exits with status 1 on failure;
/// ordinary library callers should use [`crate::cli`] and handle its result.
///
/// # Safety
/// Call only at process startup before creating worker threads or initializing
/// a runtime. The bubblewrap branch may replace process state and exec in place.
pub unsafe fn helper_main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().is_some_and(|arg| arg == "--bwrap-launch") {
        // SAFETY: this entry point requires a fresh, single-threaded process.
        if let Err(error) = unsafe { rode_host_core::bwrap::launch(&arguments[1..]) } {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = crate::cli(&arguments) {
        crate::report_error(&arguments, &error);
        std::process::exit(1);
    }
}

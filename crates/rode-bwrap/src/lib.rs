//! Bundled, unprivileged bubblewrap for hosts without a system executable.
//! The native helper dispatches here in a fresh process before starting threads.
//! RODE does not download or install tools, set setuid permissions, or change
//! host policies when using this fallback. Namespace restrictions still apply.
#![warn(missing_docs)]

/// Run upstream bubblewrap in this dedicated launcher process. Its C entry point
/// may terminate the process, so never call this from a running GUI or service.
///
/// # Safety
/// Call only at process startup, before creating threads or initializing other
/// subsystems. Upstream owns process-global state and environment mutation.
pub unsafe fn launch(arguments: &[String]) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        unsafe extern "C" {
            fn rode_bwrap_main(argc: libc::c_int, argv: *mut *mut libc::c_char) -> libc::c_int;
        }
        let mut strings = std::iter::once("rode-bwrap")
            .chain(arguments.iter().map(String::as_str))
            .map(|argument| CString::new(argument).map(CString::into_bytes_with_nul))
            .collect::<Result<Vec<_>, _>>()?;
        let mut argv: Vec<_> = strings
            .iter_mut()
            .map(|argument| argument.as_mut_ptr().cast())
            .collect();
        argv.push(std::ptr::null_mut());
        // SAFETY: All argument strings and the null-terminated pointer vector
        // remain alive and mutable for the call. This is a process entry point, not a
        // reusable API; bubblewrap owns process setup and exit from here.
        let code = unsafe { rode_bwrap_main(strings.len().try_into()?, argv.as_mut_ptr()) };
        std::process::exit(code);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = arguments;
        anyhow::bail!("Bundled bubblewrap requires Linux")
    }
}

//! Minimal Linux GSS-API v2 ABI (RFC 2744). No link-time Kerberos dependency.
//! Opaque handles are owned here; calls never run concurrently on a context.
use anyhow::{Context as _, Result, bail, ensure};
use libloading::Library;
use std::{
    ffi::c_void,
    ptr,
    sync::{Arc, OnceLock},
};

type Handle = *mut c_void;
#[repr(C)]
struct Oid {
    length: u32,
    elements: *mut c_void,
}
impl Oid {
    fn new(bytes: &[u8]) -> Self {
        Self {
            length: bytes.len() as u32,
            elements: bytes.as_ptr().cast_mut().cast(),
        }
    }
}
#[repr(C)]
struct OidSet {
    count: usize,
    elements: *mut Oid,
}
#[repr(C)]
#[derive(Default)]
struct Buffer {
    length: usize,
    value: *mut c_void,
}
impl Buffer {
    fn input(bytes: &[u8]) -> Self {
        Self {
            length: bytes.len(),
            value: bytes.as_ptr().cast_mut().cast(),
        }
    }
}
const KRB5: &[u8] = b"\x2a\x86\x48\x86\xf7\x12\x01\x02\x02";
const HOST_SERVICE: &[u8] = b"\x2a\x86\x48\x86\xf7\x12\x01\x02\x01\x04";
const MUTUAL: u32 = 2;
const INTEGRITY: u32 = 32;
const DELEGATE: u32 = 1;

// Signatures and structures follow gssapi/gssapi.h on Linux. The channel
// bindings argument is always GSS_C_NO_CHANNEL_BINDINGS; SSH binds the context
// to its session/user/service with the final MIC instead (RFC 4462 §3.5/7.2).
struct Api {
    import_name: unsafe extern "C" fn(*mut u32, *mut Buffer, *mut Oid, *mut Handle) -> u32,
    release_name: unsafe extern "C" fn(*mut u32, *mut Handle) -> u32,
    acquire_cred: unsafe extern "C" fn(
        *mut u32,
        Handle,
        u32,
        *mut OidSet,
        i32,
        *mut Handle,
        *mut *mut OidSet,
        *mut u32,
    ) -> u32,
    release_cred: unsafe extern "C" fn(*mut u32, *mut Handle) -> u32,
    init: unsafe extern "C" fn(
        *mut u32,
        Handle,
        *mut Handle,
        Handle,
        *mut Oid,
        u32,
        u32,
        Handle,
        *mut Buffer,
        *mut *mut Oid,
        *mut Buffer,
        *mut u32,
        *mut u32,
    ) -> u32,
    delete: unsafe extern "C" fn(*mut u32, *mut Handle, *mut Buffer) -> u32,
    get_mic: unsafe extern "C" fn(*mut u32, Handle, u32, *mut Buffer, *mut Buffer) -> u32,
    release_buffer: unsafe extern "C" fn(*mut u32, *mut Buffer) -> u32,
    display_status:
        unsafe extern "C" fn(*mut u32, u32, i32, *mut Oid, *mut u32, *mut Buffer) -> u32,
    // All function pointers and handles are released before this is unloaded.
    _library: Library,
}
impl Api {
    fn load() -> Result<Arc<Self>> {
        // Keep the library loaded for this connection process, including any
        // Kerberos thread-local destructors retained by the blocking workers.
        static API: OnceLock<std::result::Result<Arc<Api>, String>> = OnceLock::new();
        API.get_or_init(|| {
            Self::load_from(&["libgssapi_krb5.so.2", "libgssapi.so.3", "libgssapi.so.2"])
                .map(Arc::new)
                .map_err(|error| format!("{error:#}"))
        })
        .clone()
        .map_err(anyhow::Error::msg)
    }

    fn load_from(names: &[&str]) -> Result<Self> {
        for name in names {
            // Only fixed system SONAMEs are supplied by production code. No
            // project path, configuration value or remote input selects code.
            let Ok(library) = (unsafe { Library::new(name) }) else {
                continue;
            };
            // SAFETY: the symbols use the Linux GSS-API C ABI above. Library is
            // retained alongside every pointer and outlives all owned handles.
            return unsafe {
                Ok(Self {
                    import_name: *library.get(b"gss_import_name\0")?,
                    release_name: *library.get(b"gss_release_name\0")?,
                    acquire_cred: *library.get(b"gss_acquire_cred\0")?,
                    release_cred: *library.get(b"gss_release_cred\0")?,
                    init: *library.get(b"gss_init_sec_context\0")?,
                    delete: *library.get(b"gss_delete_sec_context\0")?,
                    get_mic: *library.get(b"gss_get_mic\0")?,
                    release_buffer: *library.get(b"gss_release_buffer\0")?,
                    display_status: *library.get(b"gss_display_status\0")?,
                    _library: library,
                })
            };
        }
        bail!(
            "System GSSAPI library unavailable; install the Kerberos runtime for ticket authentication"
        )
    }

    fn output(&self, buffer: &mut Buffer) -> Result<Vec<u8>> {
        let result = if buffer.length > super::TOKEN_LIMIT {
            Err(anyhow::anyhow!("Kerberos output exceeds limit"))
        } else if buffer.length == 0 {
            Ok(Vec::new())
        } else if buffer.value.is_null() {
            Err(anyhow::anyhow!("Invalid Kerberos output buffer"))
        } else {
            // SAFETY: this buffer was allocated by GSSAPI, with length bytes
            // valid until release_buffer. Copy before releasing its ownership.
            Ok(
                unsafe { std::slice::from_raw_parts(buffer.value.cast::<u8>(), buffer.length) }
                    .to_vec(),
            )
        };
        let mut minor = 0;
        unsafe { (self.release_buffer)(&mut minor, buffer) };
        result
    }

    fn check(&self, major: u32, minor: u32) -> Result<()> {
        if major == 0 {
            return Ok(());
        }
        let mut messages = Vec::new();
        for (code, kind) in [(major, 1), (minor, 2)] {
            let mut next = 0;
            for _ in 0..4 {
                let mut ignored = 0;
                let mut buffer = Buffer::default();
                let status = unsafe {
                    (self.display_status)(
                        &mut ignored,
                        code,
                        kind,
                        &mut Oid::new(KRB5),
                        &mut next,
                        &mut buffer,
                    )
                };
                let bytes = self.output(&mut buffer)?;
                if status != 0 {
                    break;
                }
                messages.push(
                    String::from_utf8_lossy(&bytes)
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(1024)
                        .collect::<String>(),
                );
                if next == 0 {
                    break;
                }
            }
        }
        bail!(
            "Kerberos error {major:#x}/{minor:#x}: {}",
            messages.join("; ")
        )
    }
}

pub(super) struct Context {
    api: Arc<Api>,
    name: Handle,
    cred: Handle,
    context: Handle,
    flags: u32,
    complete: bool,
}
// SAFETY: a context has a single owner. Moving it between blocking workers does
// not overlap calls; MIT/Heimdal support independent contexts on other threads.
unsafe impl Send for Context {}

impl Context {
    pub(super) fn new(host: &str, delegate: bool) -> Result<Self> {
        ensure!(
            !host.is_empty()
                && host.len() <= 1024
                && !host.contains(['@', '/', '\0'])
                && !host.chars().any(char::is_whitespace),
            "Invalid Kerberos target hostname"
        );
        let mut this = Self {
            api: Api::load()?,
            name: ptr::null_mut(),
            cred: ptr::null_mut(),
            context: ptr::null_mut(),
            flags: MUTUAL | INTEGRITY | if delegate { DELEGATE } else { 0 },
            complete: false,
        };
        let service = format!("host@{host}");
        let mut minor = 0;
        let major = unsafe {
            (this.api.import_name)(
                &mut minor,
                &mut Buffer::input(service.as_bytes()),
                &mut Oid::new(HOST_SERVICE),
                &mut this.name,
            )
        };
        this.api.check(major, minor)?;
        let mut mechanism = Oid::new(KRB5);
        let mut mechanisms = OidSet {
            count: 1,
            elements: &mut mechanism,
        };
        let major = unsafe {
            (this.api.acquire_cred)(
                &mut minor,
                ptr::null_mut(),
                0,
                &mut mechanisms,
                1,
                &mut this.cred,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        this.api
            .check(major, minor)
            .context("No usable Kerberos ticket; obtain or renew tickets outside RODE")?;
        Ok(this)
    }

    pub(super) fn step(&mut self, token: Option<&[u8]>) -> Result<Vec<u8>> {
        ensure!(!self.complete, "Kerberos context already complete");
        let mut input = token.map(Buffer::input);
        let mut output = Buffer::default();
        let (mut minor, mut flags) = (0, 0);
        let major = unsafe {
            (self.api.init)(
                &mut minor,
                self.cred,
                &mut self.context,
                self.name,
                &mut Oid::new(KRB5),
                self.flags,
                0,
                ptr::null_mut(),
                input.as_mut().map_or(ptr::null_mut(), |b| b),
                ptr::null_mut(),
                &mut output,
                &mut flags,
                ptr::null_mut(),
            )
        };
        let output = self.api.output(&mut output)?;
        // Only COMPLETE and CONTINUE_NEEDED are successful here. Replay,
        // duplicate and unsequenced supplementary statuses must not pass.
        self.api.check(major & !1, minor)?;
        self.complete = major == 0;
        if self.complete {
            ensure!(
                flags & (MUTUAL | INTEGRITY) == MUTUAL | INTEGRITY,
                "Kerberos context lacks mutual authentication or integrity"
            );
        }
        Ok(output)
    }

    pub(super) fn complete(&self) -> bool {
        self.complete
    }

    pub(super) fn mic(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        ensure!(self.complete, "Kerberos context is not complete");
        let mut minor = 0;
        let mut output = Buffer::default();
        let major = unsafe {
            (self.api.get_mic)(
                &mut minor,
                self.context,
                0,
                &mut Buffer::input(data),
                &mut output,
            )
        };
        let bytes = self.api.output(&mut output)?;
        self.api.check(major, minor)?;
        ensure!(!bytes.is_empty(), "Kerberos context returned an empty MIC");
        Ok(bytes)
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        let mut ignored = 0;
        // SAFETY: each non-null handle belongs to this context and API; no
        // operation is outstanding when the single owner is dropped.
        unsafe {
            if !self.context.is_null() {
                (self.api.delete)(&mut ignored, &mut self.context, ptr::null_mut());
            }
            if !self.cred.is_null() {
                (self.api.release_cred)(&mut ignored, &mut self.cred);
            }
            if !self.name.is_null() {
                (self.api.release_name)(&mut ignored, &mut self.name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_library_is_a_recoverable_authentication_error() {
        let error = Api::load_from(&["/nonexistent/rode-test-gssapi.so"])
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("System GSSAPI library unavailable")
        );
    }
}

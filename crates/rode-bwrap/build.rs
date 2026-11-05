//! Build pinned C sources for Cargo's target, including static libcap. Runtime
//! hosts need neither a compiler nor libcap; musl helpers retain their ABI floor.
use std::{env, fs, path::PathBuf, process::Stdio};

/// Native musl-tools sometimes supplies only libc headers. Discover Linux UAPI
/// roots through the native compiler, rather than assuming a distribution's
/// multiarch paths. Cross builds must use their target sysroot/CFLAGS instead.
fn native_kernel_headers() -> Vec<PathBuf> {
    let host = env::var("HOST").unwrap();
    let compiler = cc::Build::new()
        .host(&host)
        .target(&host)
        .cargo_metadata(false)
        .get_compiler();
    let output = compiler
        .to_command()
        .args(["-E", "-v", "-x", "c", "/dev/null"])
        .env("LC_ALL", "C")
        .output()
        .expect("Cannot query the native C compiler's include paths");
    assert!(
        output.status.success(),
        "Cannot discover native Linux UAPI headers: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut searching = false;
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| {
            if line.contains("search starts here:") {
                searching = true;
                return None;
            }
            if line.contains("End of search list.") {
                searching = false;
                return None;
            }
            let path = PathBuf::from(line.trim());
            (searching
                && (path.join("linux/types.h").is_file() || path.join("asm/types.h").is_file()))
            .then_some(path)
        })
        .collect()
}

fn compiler() -> cc::Build {
    let mut build = cc::Build::new();
    build.define("_GNU_SOURCE", None);
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl")
        && env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok(env::consts::ARCH)
        && env::var("HOST").unwrap().contains("-linux-")
    {
        let has_uapi = build
            .get_compiler()
            .to_command()
            .args([
                "-E",
                "-x",
                "c",
                "-include",
                "linux/types.h",
                "-include",
                "asm/types.h",
                "/dev/null",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("Cannot probe the target C compiler's Linux UAPI headers")
            .success();
        if !has_uapi {
            for directory in native_kernel_headers() {
                build.flag(format!("-idirafter{}", directory.display()));
            }
        }
    }
    build
}

fn main() {
    println!("cargo:rerun-if-changed=vendor");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(
        out.join("config.h"),
        "#define PACKAGE_STRING \"bubblewrap 0.13.1 (bundled with RODE)\"\n",
    )
    .unwrap();
    let compiler = compiler();
    let mut bwrap = compiler.clone();
    bwrap
        .include(&out)
        .include("vendor/bubblewrap")
        .include("vendor/libcap/libcap/include")
        .include("vendor/libcap/libcap/include/uapi")
        .define("main", "rode_bwrap_main");
    for source in [
        "bubblewrap",
        "bind-mount",
        "network",
        "utils",
        "chroot_realpath",
        "safe_openat",
    ] {
        bwrap.file(format!("vendor/bubblewrap/{source}.c"));
    }
    bwrap.compile("rode_bwrap");
    let mut cap = compiler;
    cap.include("vendor/libcap/libcap")
        .include("vendor/libcap/libcap/include")
        .include("vendor/libcap/libcap/include/uapi");
    for source in [
        "alloc", "proc", "extint", "flag", "text", "file", "syscalls",
    ] {
        cap.file(format!("vendor/libcap/libcap/cap_{source}.c"));
    }
    cap.compile("rode_bwrap_cap");
}

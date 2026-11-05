//! Build pinned C sources for Cargo's target, including static libcap. Runtime
//! hosts need neither a compiler nor libcap; musl helpers retain their ABI floor.
use std::{env, fs, path::PathBuf, process::Stdio};

type BuildResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Native musl-tools sometimes supplies only libc headers. Discover Linux UAPI
/// roots through the native compiler, rather than assuming a distribution's
/// multiarch paths. Cross builds must use their target sysroot/CFLAGS instead.
fn native_kernel_headers() -> BuildResult<Vec<PathBuf>> {
    let host = env::var("HOST")?;
    let compiler = cc::Build::new()
        .host(&host)
        .target(&host)
        .cargo_metadata(false)
        .try_get_compiler()?;
    let output = compiler
        .to_command()
        .args(["-E", "-v", "-x", "c", "/dev/null"])
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Cannot discover native Linux UAPI headers: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let mut searching = false;
    Ok(String::from_utf8_lossy(&output.stderr)
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
        .collect())
}

fn compiler() -> BuildResult<cc::Build> {
    let mut build = cc::Build::new();
    build.define("_GNU_SOURCE", None);
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl")
        && env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok(env::consts::ARCH)
        && env::var("HOST")?.contains("-linux-")
    {
        let compiler = build.try_get_compiler()?;
        let has_uapi = compiler
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
            .map_err(|error| {
                format!(
                    "Cannot run target C compiler '{}': {error}",
                    compiler.path().display()
                )
            })?
            .success();
        if !has_uapi {
            for directory in native_kernel_headers()? {
                build.flag(format!("-idirafter{}", directory.display()));
            }
        }
    }
    Ok(build)
}

fn main() {
    println!("cargo:rerun-if-changed=vendor");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    if let Err(error) = build() {
        let target = env::var("TARGET").unwrap_or_else(|_| "the Linux target".into());
        eprintln!("error: Cannot build bundled bubblewrap for {target}: {error}");
        if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl") {
            eprintln!(
                "The static helper requires a musl C compiler and target Linux UAPI headers.\n\
                 Rustup supplies Rust's musl libraries, but does not install a C toolchain.\n\
                 For native Debian/Ubuntu builds, run: sudo apt install musl-tools\n\
                 Then retry cargo xtask install (or cargo xtask build-helper).\n\
                 For custom or cross toolchains, configure CC_{} and CFLAGS_{} with the target compiler/sysroot.",
                target.replace('-', "_"),
                target.replace('-', "_")
            );
        } else {
            eprintln!("Install a C compiler and the target's Linux UAPI headers, then retry.");
        }
        std::process::exit(1);
    }
}

fn build() -> BuildResult<()> {
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo did not set OUT_DIR")?);
    fs::write(
        out.join("config.h"),
        "#define PACKAGE_STRING \"bubblewrap 0.13.1 (bundled with RODE)\"\n",
    )?;
    let compiler = compiler()?;
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
    bwrap.try_compile("rode_bwrap")?;
    let mut cap = compiler;
    cap.include("vendor/libcap/libcap")
        .include("vendor/libcap/libcap/include")
        .include("vendor/libcap/libcap/include/uapi");
    for source in [
        "alloc", "proc", "extint", "flag", "text", "file", "syscalls",
    ] {
        cap.file(format!("vendor/libcap/libcap/cap_{source}.c"));
    }
    cap.try_compile("rode_bwrap_cap")?;
    Ok(())
}

//! Source installation and portable-helper builds, separate from the application.
//! Cargo runs this host tool before it builds the editor and musl helper for their
//! respective targets; no installer work runs during ordinary builds or SSH login.

use anyhow::{Context, Result, bail, ensure};
use std::{env, ffi::OsString, path::PathBuf, process::Command};

mod helper;
mod install;

const HELP: &str = "RODE developer tasks

Usage: cargo xtask install [--root PREFIX]
       cargo xtask build-helper [x86_64|aarch64]

install       Build/install the editor and an automatically discovered static helper.
build-helper  Build/verify a musl helper and print its absolute artifact path.
              Defaults to the current architecture; cross builds need a linker.

Installation defaults to CARGO_INSTALL_ROOT, CARGO_HOME, or ~/.cargo.
";

/// Capture tools and workspace identity once; every child receives literal arguments.
struct Workspace {
    root: PathBuf,
    cargo: OsString,
    rustup: OsString,
}

impl Workspace {
    fn current() -> Self {
        Self {
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("xtask is inside the workspace")
                .into(),
            cargo: env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
            rustup: env::var_os("RUSTUP").unwrap_or_else(|| "rustup".into()),
        }
    }

    fn cargo(&self) -> Command {
        let mut command = Command::new(&self.cargo);
        command.current_dir(&self.root);
        command
    }

    fn target_directory(&self) -> Result<PathBuf> {
        let output = self
            .cargo()
            .args(["metadata", "--locked", "--no-deps", "--format-version=1"])
            .output()
            .context("Cannot read Cargo workspace metadata")?;
        ensure!(
            output.status.success(),
            "Cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        Ok(metadata["target_directory"]
            .as_str()
            .context("Cargo did not report its target directory")?
            .into())
    }
}

/// Reap each build/install command before proceeding to dependent publication.
fn run(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("Cannot start {command:?}"))?;
    ensure!(status.success(), "{command:?} failed ({status})");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let Some(task) = args.first() else {
        print!("{HELP}");
        return Ok(());
    };
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    let workspace = Workspace::current();
    if task == "build-helper" {
        ensure!(args.len() <= 2, "{HELP}");
        let architecture = helper::Architecture::parse(
            args.get(1)
                .map(|arg| arg.to_str().context("Architecture must be UTF-8"))
                .transpose()?
                .unwrap_or(env::consts::ARCH),
        )?;
        println!("{}", helper::build(&workspace, architecture)?.display());
    } else if task == "install" {
        let root = match &args[1..] {
            [] => install::default_root(
                env::var_os("CARGO_INSTALL_ROOT"),
                env::var_os("CARGO_HOME"),
                env::var_os("HOME"),
            )?,
            [flag, path] if flag == "--root" => PathBuf::from(path),
            _ => bail!("{HELP}"),
        };
        install::install(&workspace, root)?;
    } else {
        bail!("Unknown task: {}\n{HELP}", task.to_string_lossy());
    }
    Ok(())
}

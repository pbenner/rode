//! Build the headless helper for its remote Linux target and verify its ABI.

use crate::{Workspace, install::publish, run};
use anyhow::{Context, Result, ensure};
use object::{
    Endianness, elf,
    read::elf::{Dyn, ElfFile64, FileHeader, ProgramHeader},
};
use std::{
    env, fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
};

/// Supported Linux helper architecture paired with its required ELF machine identifier.
#[derive(Clone, Copy)]
pub(super) struct Architecture {
    name: &'static str,
    machine: u16,
}

impl Architecture {
    /// Resolve a supported helper architecture name; reject targets outside x86_64/aarch64.
    pub(super) fn parse(name: &str) -> Result<Self> {
        match name {
            "x86_64" => Ok(Self {
                name: "x86_64",
                machine: elf::EM_X86_64,
            }),
            "aarch64" => Ok(Self {
                name: "aarch64",
                machine: elf::EM_AARCH64,
            }),
            _ => anyhow::bail!("Supported Linux helper architectures: x86_64, aarch64"),
        }
    }
}

/// Inspect program headers as well as machine identity: a matching architecture
/// alone does not prevent a newer glibc requirement or an unavailable loader.
pub(super) fn check(bytes: &[u8], architecture: Architecture) -> Result<()> {
    let file = ElfFile64::<Endianness>::parse(bytes).context("Invalid 64-bit Linux ELF helper")?;
    let endian = file.endian();
    ensure!(
        endian == Endianness::Little
            && file.elf_header().e_machine(endian) == architecture.machine
            && matches!(file.elf_header().e_type(endian), elf::ET_EXEC | elf::ET_DYN),
        "The helper must be a matching little-endian 64-bit Linux executable"
    );
    for header in file.elf_program_headers() {
        ensure!(
            header.p_type(endian) != elf::PT_INTERP,
            "The helper has a dynamic loader; a static musl build is required"
        );
        if let Some(dynamic) = header.dynamic(endian, bytes)? {
            ensure!(
                dynamic
                    .iter()
                    .all(|entry| entry.tag32(endian) != Some(elf::DT_NEEDED)),
                "The helper needs shared libraries; a static musl build is required"
            );
        }
    }
    Ok(())
}

/// Build only the headless package; use Cargo's reported path rather than a
/// guessed target directory or a previous build's executable.
pub(super) fn build(workspace: &Workspace, architecture: Architecture) -> Result<PathBuf> {
    let target = format!("{}-unknown-linux-musl", architecture.name);
    run(Command::new(&workspace.rustup)
        .current_dir(&workspace.root)
        .args(["target", "add", &target]))?;
    let mut child = workspace
        .cargo()
        .args([
            "build",
            "--locked",
            "--release",
            "-p",
            "rode-session",
            "--features",
            "standalone",
            "--bin",
            "rode-sessiond-standalone",
            "--target",
            &target,
            "--message-format=json-render-diagnostics",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .context("Cannot build the static helper")?;
    let mut executable = None;
    for line in BufReader::new(child.stdout.take().context("Missing Cargo output")?).lines() {
        let line = line?;
        match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(event) => {
                if event["reason"] == "compiler-artifact"
                    && event["target"]["name"] == "rode-sessiond-standalone"
                    && let Some(path) = event["executable"].as_str()
                {
                    executable = Some(PathBuf::from(path));
                }
            }
            Err(_) => eprintln!("{line}"),
        }
    }
    ensure!(child.wait()?.success(), "Static helper build failed");
    let executable = executable.context("Cargo did not report its helper executable")?;
    check(&fs::read(&executable)?, architecture)?;
    if architecture.name == env::consts::ARCH && env::consts::OS == "linux" {
        publish(
            &executable,
            &workspace
                .target_directory()?
                .join("release/rode-sessiond-static"),
            || Ok(()),
        )?;
    }
    Ok(executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal executable headers exercise ELF checks without a compiler,
    /// external readelf or executing the inspected bytes.
    fn executable(machine: u16, segment: u32, tag: u64) -> Vec<u8> {
        let mut bytes = vec![0; 136];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&elf::ET_DYN.to_le_bytes());
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
        bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1_u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&segment.to_le_bytes());
        bytes[72..80].copy_from_slice(&120_u64.to_le_bytes());
        bytes[96..104].copy_from_slice(&16_u64.to_le_bytes());
        bytes[120..128].copy_from_slice(&tag.to_le_bytes());
        bytes
    }

    #[test]
    fn static_helper_validation_checks_architecture_loader_and_shared_libraries() {
        for name in ["x86_64", "aarch64"] {
            let architecture = Architecture::parse(name).unwrap();
            let valid = executable(
                architecture.machine,
                elf::PT_DYNAMIC,
                u64::from(elf::DT_NULL),
            );
            check(&valid, architecture).unwrap();
            let wrong = executable(0, elf::PT_LOAD, 0);
            assert!(check(&wrong, architecture).is_err());
            let loader = executable(architecture.machine, elf::PT_INTERP, 0);
            assert!(
                check(&loader, architecture)
                    .unwrap_err()
                    .to_string()
                    .contains("dynamic loader")
            );
            let library = executable(
                architecture.machine,
                elf::PT_DYNAMIC,
                u64::from(elf::DT_NEEDED),
            );
            assert!(
                check(&library, architecture)
                    .unwrap_err()
                    .to_string()
                    .contains("shared libraries")
            );
            assert!(check(&valid[..65], architecture).is_err());
            let mut big_endian = valid;
            big_endian[5] = 2;
            assert!(check(&big_endian, architecture).is_err());
        }
        assert!(Architecture::parse("unknown").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn builds_use_cargo_artifacts_and_configured_target_directory_before_installing() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("custom target with spaces");
        let executable_path = target.join("reported-artifact");
        fs::create_dir(&target).unwrap();
        let architecture = Architecture::parse(env::consts::ARCH).unwrap();
        let bytes = executable(architecture.machine, elf::PT_LOAD, 0);
        fs::write(&executable_path, &bytes).unwrap();
        fs::write(
            root.path().join("artifact.json"),
            serde_json::json!({
                "reason":"compiler-artifact", "target":{"name":"rode-sessiond-standalone"},
                "executable":executable_path
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            root.path().join("metadata.json"),
            serde_json::json!({"target_directory":target}).to_string(),
        )
        .unwrap();
        let cargo = root.path().join("cargo");
        fs::write(&cargo, "#!/bin/sh\ncase $1 in\nbuild) cat artifact.json;;\nmetadata) cat metadata.json;;\ninstall) touch editor-install-started;;\n*) exit 1;;\nesac\n").unwrap();
        let rustup = root.path().join("rustup");
        fs::write(&rustup, "#!/bin/sh\nexit 0\n").unwrap();
        for tool in [&cargo, &rustup] {
            fs::set_permissions(tool, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let workspace = Workspace {
            root: root.path().into(),
            cargo: cargo.into(),
            rustup: rustup.into(),
        };
        assert_eq!(build(&workspace, architecture).unwrap(), executable_path);
        if env::consts::OS == "linux" {
            assert_eq!(
                fs::read(target.join("release/rode-sessiond-static")).unwrap(),
                bytes
            );
            fs::write(
                &executable_path,
                executable(architecture.machine, elf::PT_INTERP, 0),
            )
            .unwrap();
            assert!(crate::install::install(&workspace, root.path().join("prefix")).is_err());
            assert!(!root.path().join("editor-install-started").exists());
            assert!(!root.path().join("prefix").exists());
        }
    }
}

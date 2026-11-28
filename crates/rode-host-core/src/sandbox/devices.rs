//! Vendor-independent device grants. Paths are optional because login and compute
//! hosts need not have the same devices. No discovery or privilege escalation is implied.
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

/// Accept explicit children of /dev, never its root, traversal, patterns or descriptor aliases.
pub(super) fn validate_path(path: &str) -> Result<()> {
    let parts = path.split('/').collect::<Vec<_>>();
    ensure!(
        path.len() <= 4096
            && !path.chars().any(char::is_control)
            && !path.contains(['*', '?', '[', ']'])
            && parts.len() >= 3
            && parts[0].is_empty()
            && parts[1] == "dev"
            && parts[2..]
                .iter()
                .all(|part| !matches!(*part, "" | "." | "..")),
        "Device paths must be literal absolute paths below /dev, such as /dev/dri or /dev/kfd"
    );
    ensure!(
        !matches!(parts[2], "fd" | "stdin" | "stdout" | "stderr" | "core"),
        "Process descriptor aliases are not sandbox device paths"
    );
    Ok(())
}

/// Validate present resources and resolve aliases for Landlock's inode-based grants.
/// Missing nodes are deliberately skipped; any other lookup error remains an error.
pub(crate) fn resolve(path: &str) -> Result<Option<PathBuf>> {
    validate_path(path)?;
    let resolved = match Path::new(path).canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("Cannot resolve device path {path}"));
        }
    };
    ensure!(
        resolved.starts_with("/dev") && resolved.components().count() > 2,
        "Device path must resolve below /dev: {path}"
    );
    let metadata = resolved.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        ensure!(
            metadata.is_dir()
                || metadata.file_type().is_char_device()
                || metadata.file_type().is_block_device(),
            "Device path is not a device node or directory: {path}"
        );
    }
    Ok(Some(resolved))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_are_explicit_vendor_independent_and_bounded() {
        for path in [
            "/dev/dri",
            "/dev/kfd",
            "/dev/nvidia0",
            "/dev/serial/by-id/adapter",
        ] {
            validate_path(path).unwrap();
        }
        for path in [
            "",
            "/dev",
            "/dev/",
            "/etc/passwd",
            "~/device",
            "/device/x",
            "/dev/../etc",
            "/dev//dri",
            "/dev/dri/.",
            "/dev/nvidia*",
            "/dev/tty?",
            "/dev/fd/3",
            "/dev/stdin",
            "/dev/core",
            "/dev/x\n",
        ] {
            assert!(validate_path(path).is_err(), "{path}");
        }
        assert!(validate_path(&format!("/dev/{}", "x".repeat(4096))).is_err());
        assert!(resolve("/dev/rode-test-missing-device").unwrap().is_none());
        assert!(resolve("/dev/null").unwrap().is_some());
    }

    #[test]
    fn options_retain_device_grants_and_reject_conflicts_and_combined_limits() {
        use super::super::{Access, Options};
        let legacy: Options = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(legacy.devices.is_empty());
        let mut options = Options {
            enabled: true,
            devices: vec!["/dev/dri".into(), "/dev/kfd".into()],
            ..Default::default()
        };
        options.validate().unwrap();
        let encoded = serde_json::to_string(&options).unwrap();
        assert_eq!(serde_json::from_str::<Options>(&encoded).unwrap(), options);
        options.devices.push("/dev/dri".into());
        assert!(options.validate().is_err());
        options.devices.pop();
        options.overrides.insert("/usr".into(), Access::Device);
        assert!(options.validate().is_err());
        options.overrides.clear();
        options.read_only = vec!["/data".into(); 31];
        assert!(options.validate().is_err());
        options.read_only.clear();
        options.devices = (0..5)
            .map(|i| format!("/dev/{i}{}", "x".repeat(3500)))
            .collect();
        assert!(options.validate().is_err());
    }
}

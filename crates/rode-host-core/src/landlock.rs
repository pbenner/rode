//! Native Linux Landlock launcher. Enforce in a fresh, single-threaded process before exec.
//! ABI 4 supplies filesystem/TCP restrictions; ABI 5 adds device-ioctl mediation,
//! and ABI 6 adds signal/abstract-socket scoping. Network-off also uses seccomp.
use anyhow::{Context, Result, bail, ensure};

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        collections::BTreeMap,
        ffi::CString,
        fs::File,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::process::CommandExt,
        },
        path::{Path, PathBuf},
        process::Command,
    };

    fn checked(value: libc::c_long, operation: &str) -> Result<libc::c_long> {
        if value < 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| operation.to_owned());
        }
        Ok(value)
    }
    fn fd(value: libc::c_int, operation: &str) -> Result<File> {
        checked(value as _, operation)?;
        // Ownership transfers from the successful open/syscall to File exactly once.
        Ok(unsafe { File::from_raw_fd(value) })
    }

    /// Walk each prepared component with O_NOFOLLOW, pinning ancestors as we descend.
    fn captured(path: &Path) -> Result<File> {
        ensure!(
            path.is_absolute()
                && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str(),
            "Landlock requires a canonical captured path: {}",
            path.display()
        );
        let mut directory = fd(
            unsafe {
                libc::open(
                    c"/".as_ptr(),
                    libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
                )
            },
            "Open Landlock root",
        )?;
        let components = path.components().skip(1).collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            ensure!(
                matches!(component, std::path::Component::Normal(_)),
                "Invalid captured Landlock component"
            );
            let name = CString::new(component.as_os_str().as_encoded_bytes())?;
            let flags = libc::O_PATH
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | if index + 1 < components.len() {
                    libc::O_DIRECTORY
                } else {
                    0
                };
            directory = fd(
                unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) },
                "Open captured Landlock grant",
            )?;
            ensure!(
                !directory.metadata()?.file_type().is_symlink(),
                "Landlock grant changed to a symlink: {}",
                path.display()
            );
        }
        Ok(directory)
    }
    fn scratch(home: &Path, root: &Path) -> Result<(PathBuf, File)> {
        crate::private_tmp::project(home, root)
    }
    fn grant_descriptor(path: &Path, scratch_path: &Path, scratch_fd: &File) -> Result<File> {
        if path == scratch_path {
            Ok(scratch_fd.try_clone()?)
        } else {
            captured(path)
        }
    }
    #[test]
    fn scratch_replacement_keeps_the_original_inode_and_rejects_other_symlinks() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let home = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let project = tempfile::tempdir().unwrap();
        let private = tempfile::tempdir().unwrap();
        let (path, descriptor) = scratch(home.path(), project.path()).unwrap();
        let original = descriptor.metadata().unwrap().ino();
        std::fs::rename(&path, path.with_extension("original")).unwrap();
        std::os::unix::fs::symlink(private.path(), &path).unwrap();
        let granted = grant_descriptor(&path, &path, &descriptor).unwrap();
        assert_eq!(granted.metadata().unwrap().ino(), original);
        assert_ne!(
            granted.metadata().unwrap().ino(),
            private.path().metadata().unwrap().ino()
        );
        assert!(captured(&path).is_err());
        assert!(grant_descriptor(&path.join("secret"), &path, &descriptor).is_err());
    }
    fn resolved(path: &Path) -> Result<PathBuf> {
        match path.canonicalize() {
            Ok(path) => Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(resolved(path.parent().context("Missing grant parent")?)?
                    .join(path.file_name().context("Missing grant filename")?))
            }
            Err(error) => Err(error.into()),
        }
    }
    struct Policy {
        grants: Vec<(PathBuf, bool)>,
        hidden: Vec<PathBuf>,
        environment: BTreeMap<String, String>,
        network: bool,
        root: PathBuf,
        home: PathBuf,
        command: Vec<String>,
    }
    fn policy(arguments: &[String]) -> Result<Policy> {
        let mut p = Policy {
            grants: vec![],
            hidden: vec![],
            environment: BTreeMap::new(),
            network: false,
            root: PathBuf::new(),
            home: PathBuf::new(),
            command: vec![],
        };
        let mut args = arguments.iter();
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--" => {
                    p.command.extend(args.cloned());
                    break;
                }
                "--unshare-all" | "--die-with-parent" => {}
                "--share-net" => p.network = true,
                "--ro-bind" | "--bind" => {
                    let source = args.next().context("Missing Landlock grant source")?;
                    ensure!(
                        Some(source) == args.next(),
                        "Landlock cannot remap filesystem paths"
                    );
                    p.grants.push((source.into(), flag == "--bind"));
                }
                "--rode-home" => p.home = args.next().context("Missing Landlock home")?.into(),
                "--rode-deny" => p.hidden.push(resolved(Path::new(
                    args.next().context("Missing hidden Landlock path")?,
                ))?),
                "--setenv" => {
                    let name = args.next().context("Missing environment name")?;
                    let value = args.next().context("Missing environment value")?;
                    ensure!(
                        !name.is_empty() && !name.contains(['=', '\0']) && !value.contains('\0'),
                        "Invalid Landlock environment"
                    );
                    p.environment.insert(name.clone(), value.clone());
                }
                "--tmpfs" | "--dir" | "--proc" | "--dev" | "--chdir" => {
                    let path = args.next().context("Missing Landlock policy path")?;
                    match (flag.as_str(), path.as_str()) {
                        ("--chdir", _) => p.root = path.into(),
                        ("--proc", "/proc") => p.grants.push((path.into(), false)),
                        ("--dev", "/dev") => {
                            for device in
                                ["null", "zero", "random", "urandom", "tty", "ptmx", "pts"]
                            {
                                let path = Path::new("/dev").join(device);
                                if path.exists() {
                                    p.grants.push((path.canonicalize()?, true));
                                }
                            }
                        }
                        ("--tmpfs", "/tmp" | "/home") | ("--dir", _) => {}
                        _ => bail!("Unsupported Landlock device/process/temporary view"),
                    }
                }
                _ => bail!("Unsupported Landlock policy flag: {flag}"),
            }
        }
        ensure!(
            p.root.is_absolute() && p.home.is_absolute() && !p.command.is_empty(),
            "Incomplete Landlock launch policy"
        );
        Ok(p)
    }
    fn validate(p: &Policy) -> Result<()> {
        for (child, writable) in p
            .grants
            .iter()
            .map(|(p, w)| (p, *w))
            .chain(p.hidden.iter().map(|p| (p, false)))
        {
            for (parent, parent_writable) in &p.grants {
                ensure!(
                    !(child.starts_with(parent)
                        && (p.hidden.contains(child) || (*parent_writable && !writable))),
                    "Landlock cannot enforce a restrictive nested grant: {} inside {}; use bwrap or remove the broader grant",
                    child.display(),
                    parent.display()
                );
            }
        }
        // Executable and shared libraries run before enforcement. A writable grant
        // must not permit replacement of any code used by the next launcher.
        let mut runtime = vec![std::env::current_exe()?.canonicalize()?];
        let maps = std::fs::read_to_string("/proc/self/maps")?;
        ensure!(
            maps.len() <= 4 * 1024 * 1024,
            "Native launcher mappings exceed the validation limit"
        );
        for line in maps.lines() {
            if let Some(index) = line.find('/') {
                let name = line[index..]
                    .replace("\\040", " ")
                    .replace("\\011", "\t")
                    .replace("\\012", "\n")
                    .replace("\\134", "\\");
                let path = PathBuf::from(name);
                // A deleted executable/library cannot safely support a new launch.
                runtime.push(
                    path.canonicalize()
                        .context("Cannot verify native launcher runtime")?,
                );
            }
        }
        for runtime in runtime {
            for (path, writable) in &p.grants {
                ensure!(
                    !(*writable && (runtime.starts_with(path) || path.starts_with(&runtime))),
                    "Landlock interpreter/runtime overlaps a writable grant: {}",
                    path.display()
                );
            }
        }
        Ok(())
    }
    fn restrict_network() -> Result<()> {
        #[cfg(target_arch = "x86_64")]
        let (arch, denied): (u32, &[u32]) = (
            0xc000003e,
            &[41, 42, 43, 44, 46, 49, 50, 288, 307, 425, 426, 427],
        );
        #[cfg(target_arch = "aarch64")]
        let (arch, denied): (u32, &[u32]) = (
            0xc00000b7,
            &[198, 200, 201, 202, 203, 206, 211, 242, 269, 425, 426, 427],
        );
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        let (arch, denied): (u32, &[u32]) = (0, &[]);
        let mut rows = vec![
            (0x20, 0, 0, 4),
            (0x15, 1, 0, arch),
            (0x06, 0, 0, 0x80000000),
            (0x20, 0, 0, 0),
            (0x35, 0, 1, 0x40000000),
            (0x06, 0, 0, 0x80000000),
        ];
        for syscall in denied {
            rows.extend([
                (0x15, 0, 1, *syscall),
                (0x06, 0, 0, 0x00050000 | libc::EPERM as u32),
            ]);
        }
        rows.push((0x06, 0, 0, 0x7fff0000));
        let mut filters = rows
            .into_iter()
            .map(|(code, jt, jf, k)| libc::sock_filter { code, jt, jf, k })
            .collect::<Vec<_>>();
        let program = libc::sock_fprog {
            len: filters.len() as u16,
            filter: filters.as_mut_ptr(),
        };
        checked(
            unsafe { libc::prctl(libc::PR_SET_SECCOMP, 2, &program as *const _, 0, 0) } as _,
            "Seccomp network policy",
        )?;
        Ok(())
    }
    #[repr(C)]
    struct Ruleset {
        filesystem: u64,
        network: u64,
        scoped: u64,
    }
    impl Ruleset {
        fn for_abi(abi: libc::c_long, network: bool) -> Result<Self> {
            ensure!(
                abi >= 4,
                "Landlock ABI 4 or newer is required; host provides ABI {abi}"
            );
            Ok(Self {
                filesystem: filesystem_access(abi),
                network: if network { 0 } else { 3 },
                scoped: if abi >= 6 { 3 } else { 0 },
            })
        }

        fn size(&self) -> usize {
            // ABI 4/5 know only the filesystem and network fields.
            if self.scoped == 0 {
                16
            } else {
                std::mem::size_of::<Self>()
            }
        }
    }

    fn filesystem_access(abi: libc::c_long) -> u64 {
        let mut access = (1u64 << 13) - 1;
        if abi >= 2 {
            access |= 1 << 13;
        }
        if abi >= 3 {
            access |= 1 << 14;
        }
        if abi >= 5 {
            access |= 1 << 15;
        }
        access
    }

    fn abi() -> Result<libc::c_long> {
        checked(
            unsafe { libc::syscall(444, std::ptr::null::<u8>(), 0usize, 1u32) },
            "Landlock ABI probe",
        )
    }

    /// Probe the available Landlock ruleset and describe that ABI's enforcement limits.
    /// Return an error when the required rules cannot be created; this does not confine the caller.
    pub(super) fn compatibility_notice() -> Result<String> {
        let abi = abi()?;
        Ruleset::for_abi(abi, true)?;
        Ok(match abi {
            4 => "Landlock ABI 4: file and network rules are active; device ioctls, host signals and abstract Unix sockets are not isolated.".into(),
            5 => "Landlock ABI 5: file, device-ioctl and network rules are active; host signals and abstract Unix sockets are not isolated.".into(),
            _ => format!("Landlock ABI {abi}: file, device-ioctl, network and signal/abstract-socket rules are active."),
        })
    }
    #[repr(C, packed)]
    struct PathRule {
        access: u64,
        parent_fd: i32,
    }

    /// Narrow filesystem-only policy for an already loaded, one-shot native
    /// helper. Unlike the project launcher, this needs only Landlock ABI 1.
    pub fn confine_agent_files(root: &File, read_only: bool) -> Result<()> {
        let abi = checked(
            unsafe { libc::syscall(444, std::ptr::null::<u8>(), 0usize, 1u32) },
            "File-only mode requires Linux Landlock filesystem support",
        )?;
        ensure!(abi >= 1, "File-only mode requires Landlock ABI 1 or newer");
        let handled = filesystem_access(abi);
        // Passing just the original filesystem field supports older kernels.
        let raw = checked(
            unsafe { libc::syscall(444, &handled as *const u64, 8usize, 0u32) },
            "File-only Landlock ruleset",
        )?;
        let rules = fd(raw as _, "File-only Landlock descriptor")?;
        let read = (1 << 2) | (1 << 3);
        let write = (1 << 1) | (1 << 5) | (1 << 7) | (1 << 8) | (handled & (1 << 14));
        let rule = PathRule {
            access: if read_only { read } else { read | write },
            parent_fd: root.as_raw_fd(),
        };
        checked(
            unsafe { libc::syscall(445, rules.as_raw_fd(), 1u32, &rule as *const _, 0u32) },
            "File-only Landlock project grant",
        )?;
        checked(
            unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } as _,
            "File-only no_new_privs",
        )?;
        checked(
            unsafe { libc::syscall(446, rules.as_raw_fd(), 0u32) },
            "File-only Landlock enforcement",
        )?;
        Ok(())
    }

    /// Dedicated single-threaded helper entry point: validate policy, prepare rules and enter
    /// Landlock before launching the requested command. Unsupported architectures, capabilities or
    /// grants return errors rather than executing without enforcement.
    pub fn launch(arguments: &[String]) -> Result<()> {
        ensure!(
            cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) && usize::BITS == 64,
            "Landlock requires Linux x86_64 or aarch64"
        );
        ensure!(
            std::fs::read_dir("/proc/self/task")?.count() == 1,
            "Landlock must be applied before starting helper threads"
        );
        let parent = unsafe { libc::getppid() };
        let abi = abi()?;
        let mut p = policy(arguments)?;
        let ruleset = Ruleset::for_abi(abi, p.network)?;
        validate(&p)?;
        {
            let (scratch_path, scratch_fd) = scratch(&p.home, &p.root)?;
            p.grants.push((scratch_path.clone(), true));
            validate(&p)?;
            for name in ["TMPDIR", "TMP", "TEMP"] {
                p.environment
                    .insert(name.into(), scratch_path.to_string_lossy().into_owned());
            }
            let raw = checked(
                unsafe { libc::syscall(444, &ruleset as *const _, ruleset.size(), 0u32) },
                "Landlock ruleset",
            )?;
            let rules = fd(raw as _, "Landlock ruleset descriptor")?;
            let mut root = None;
            for (path, writable) in &p.grants {
                let descriptor = grant_descriptor(path, &scratch_path, &scratch_fd)?;
                if path == &p.root {
                    root = Some(descriptor.try_clone()?);
                }
                let mut access = if *writable {
                    ruleset.filesystem
                } else {
                    (1 << 0) | (1 << 2) | (1 << 3)
                };
                if !descriptor.metadata()?.is_dir() {
                    access &= (1 << 0) | (1 << 1) | (1 << 2) | (1 << 14) | (1 << 15);
                }
                access &= ruleset.filesystem;
                let rule = PathRule {
                    access,
                    parent_fd: descriptor.as_raw_fd(),
                };
                checked(
                    unsafe { libc::syscall(445, rules.as_raw_fd(), 1u32, &rule as *const _, 0u32) },
                    &format!("Landlock grant {}", path.display()),
                )?;
            }
            let root = root.context("Landlock project root must have an explicit grant")?;
            checked(
                unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } as _,
                "no_new_privs",
            )?;
            checked(
                unsafe { libc::syscall(446, rules.as_raw_fd(), 0u32) },
                "Landlock enforcement",
            )?;
            checked(
                unsafe { libc::fchdir(root.as_raw_fd()) } as _,
                "Landlock working directory",
            )?;
        }
        if !p.network {
            restrict_network()?;
        }
        // No pre-sandbox descriptor survives exec, including handles above a lowered rlimit.
        checked(
            unsafe { libc::syscall(436, 3u32, u32::MAX, 0u32) },
            "Close inherited descriptors",
        )?;
        checked(
            unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) } as _,
            "Parent lifetime",
        )?;
        ensure!(
            unsafe { libc::getppid() } == parent,
            "Landlock launch parent exited"
        );
        let error = Command::new(&p.command[0])
            .args(&p.command[1..])
            .envs(p.environment)
            .exec();
        Err(error).context("Cannot execute Landlock command")
    }

    #[test]
    fn rulesets_only_request_features_supported_by_the_kernel_abi() {
        for (abi, filesystem, scoped, size) in [
            (4, (1 << 15) - 1, 0, 16),
            (5, (1 << 16) - 1, 0, 16),
            (6, (1 << 16) - 1, 3, 24),
            (12, (1 << 16) - 1, 3, 24),
        ] {
            for network in [false, true] {
                let rules = Ruleset::for_abi(abi, network).unwrap();
                assert_eq!(rules.filesystem, filesystem);
                assert_eq!(rules.network, if network { 0 } else { 3 });
                assert_eq!(rules.scoped, scoped);
                assert_eq!(rules.size(), size);
            }
        }
        for abi in 0..4 {
            let error = Ruleset::for_abi(abi, true).err().unwrap();
            assert!(error.to_string().contains("ABI 4 or newer"));
        }
    }
}

/// Describe the project host's actual Landlock support after a successful probe.
pub(crate) fn compatibility_notice() -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        linux::compatibility_notice()
    }
    #[cfg(not(target_os = "linux"))]
    {
        bail!("Landlock requires Linux")
    }
}

/// Restrict a fresh file-tool helper to its captured directory, without allowing
/// execution or special-file creation. This stacks with the project sandbox.
pub fn confine_agent_files(root: &std::fs::File, read_only: bool) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::confine_agent_files(root, read_only)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, read_only);
        bail!("File-only tools require Linux Landlock")
    }
}

/// Called only from a helper entry point before thread or service initialization.
pub fn launch(arguments: &[String]) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::launch(arguments)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = arguments;
        bail!("Landlock requires Linux")
    }
}

//! Native Linux Landlock launcher. Enforce in a fresh, single-threaded process before exec.
//! ABI 6 is the baseline: filesystem restrictions, TCP restrictions and signal/abstract
//! socket scoping. A seccomp filter additionally denies non-TCP networking and io_uring.
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
            unix::{fs::MetadataExt, process::CommandExt},
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
        let key = crate::files::digest(root.as_os_str().as_encoded_bytes());
        let mut path = home.to_owned();
        let mut directory = captured(home)?;
        for component in [".cache", "rode", "landlock", &key] {
            path.push(component);
            let name = CString::new(component)?;
            let result = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
            if result != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                checked(result as _, "Create Landlock scratch")?;
            }
            directory = fd(
                unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                },
                "Open Landlock scratch",
            )?;
            let metadata = directory.metadata()?;
            ensure!(
                metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
                "Unsafe Landlock scratch directory: {}",
                path.display()
            );
            ensure!(
                component == ".cache" || metadata.mode() & 0o077 == 0,
                "Landlock scratch directory must be owner-only: {}",
                path.display()
            );
        }
        Ok((path, directory))
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
        let home = tempfile::tempdir().unwrap();
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
    #[repr(C, packed)]
    struct PathRule {
        access: u64,
        parent_fd: i32,
    }

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
        let abi = checked(
            unsafe { libc::syscall(444, std::ptr::null::<u8>(), 0usize, 1u32) },
            "Landlock ABI probe",
        )?;
        ensure!(
            abi >= 6,
            "Landlock ABI 6 or newer is required; host provides ABI {abi}"
        );
        let mut p = policy(arguments)?;
        validate(&p)?;
        {
            let (scratch_path, scratch_fd) = scratch(&p.home, &p.root)?;
            p.grants.push((scratch_path.clone(), true));
            validate(&p)?;
            for name in ["TMPDIR", "TMP", "TEMP"] {
                p.environment
                    .insert(name.into(), scratch_path.to_string_lossy().into_owned());
            }
            let ruleset = Ruleset {
                filesystem: (1 << 16) - 1,
                network: if p.network { 0 } else { 3 },
                scoped: 3,
            };
            let raw = checked(
                unsafe {
                    libc::syscall(
                        444,
                        &ruleset as *const _,
                        std::mem::size_of::<Ruleset>(),
                        0u32,
                    )
                },
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
                    (1u64 << 16) - 1
                } else {
                    (1 << 0) | (1 << 2) | (1 << 3)
                };
                if !descriptor.metadata()?.is_dir() {
                    access &= (1 << 0) | (1 << 1) | (1 << 2) | (1 << 14) | (1 << 15);
                }
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

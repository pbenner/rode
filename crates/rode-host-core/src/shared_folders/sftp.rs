//! Small SFTP v3 filesystem adapter for SSHFS. Framing, requests and handle count
//! are bounded. Every pathname operation is relative to a pinned cap-std Dir;
//! symlinks and special files are excluded, and read-only is checked server-side.
use super::*;
use cap_std::fs::{Dir, Metadata, MetadataExt, OpenOptions, OpenOptionsExt};
use std::{
    collections::HashMap,
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{FileExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};

#[cfg(test)]
mod tests;

const MAX_PACKET: usize = 1024 * 1024;
const BLOCK: usize = 64 * 1024;
const HANDLES: usize = 512;
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid SFTP request")
}
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Shared folder is read-only or path is outside its scope",
    )
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if count > self.0.len() {
            return Err(invalid());
        }
        let (head, tail) = self.0.split_at(count);
        self.0 = tail;
        Ok(head)
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let count = self.u32()? as usize;
        self.take(count)
    }
    fn string(&mut self) -> io::Result<String> {
        String::from_utf8(self.bytes()?.into()).map_err(|_| invalid())
    }
    fn path(&mut self) -> io::Result<PathBuf> {
        let text = self.string()?;
        if text.len() > 4096 || text.contains('\0') {
            return Err(invalid());
        }
        let path = Path::new(text.trim_start_matches('/'));
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(denied());
        }
        Ok(if path.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            path.into()
        })
    }
    fn handle(&mut self) -> io::Result<u32> {
        self.string()?.parse().map_err(|_| invalid())
    }
    fn done(&self) -> io::Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}
fn u32(out: &mut Vec<u8>, value: u32) {
    out.extend(value.to_be_bytes());
}
fn string(out: &mut Vec<u8>, value: &[u8]) {
    u32(out, value.len() as u32);
    out.extend(value);
}
fn packet(output: &mut impl Write, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= MAX_PACKET, "SFTP response exceeds limit");
    output.write_all(&(bytes.len() as u32).to_be_bytes())?;
    output.write_all(bytes)?;
    output.flush()?;
    Ok(())
}
fn status(id: u32, code: u32, message: &str) -> Vec<u8> {
    let mut out = vec![101];
    u32(&mut out, id);
    u32(&mut out, code);
    string(&mut out, message.as_bytes());
    string(&mut out, b"en");
    out
}
fn attrs(out: &mut Vec<u8>, meta: &Metadata, read_only: bool) {
    u32(out, 15);
    out.extend(meta.len().to_be_bytes());
    u32(out, meta.uid());
    u32(out, meta.gid());
    u32(out, meta.mode() & if read_only { !0o222 } else { u32::MAX });
    u32(out, meta.atime().clamp(0, u32::MAX as i64) as u32);
    u32(out, meta.mtime().clamp(0, u32::MAX as i64) as u32);
}
#[derive(Default)]
struct Attrs {
    size: Option<u64>,
    owner: Option<(u32, u32)>,
    mode: Option<u32>,
    times: Option<(u32, u32)>,
}
impl Attrs {
    fn read(r: &mut Reader<'_>) -> io::Result<Self> {
        let flags = r.u32()?;
        if flags & !15 != 0 {
            return Err(invalid());
        }
        Ok(Self {
            size: if flags & 1 != 0 { Some(r.u64()?) } else { None },
            owner: if flags & 2 != 0 {
                Some((r.u32()?, r.u32()?))
            } else {
                None
            },
            mode: if flags & 4 != 0 { Some(r.u32()?) } else { None },
            times: if flags & 8 != 0 {
                Some((r.u32()?, r.u32()?))
            } else {
                None
            },
        })
    }
    fn apply(&self, file: &File) -> io::Result<()> {
        let meta = Metadata::from_file(file)?;
        if self
            .owner
            .is_some_and(|owner| owner != (meta.uid(), meta.gid()))
        {
            return Err(denied());
        }
        if let Some(size) = self.size {
            if !meta.is_file() {
                return Err(denied());
            }
            file.set_len(size)?;
        }
        if let Some(mode) = self.mode {
            file.set_permissions(std::fs::Permissions::from_mode(mode & 0o777))?;
        }
        if let Some((a, m)) = self.times {
            let times = [
                libc::timespec {
                    tv_sec: a as _,
                    tv_nsec: 0,
                },
                libc::timespec {
                    tv_sec: m as _,
                    tv_nsec: 0,
                },
            ];
            if unsafe { libc::futimens(file.as_raw_fd(), times.as_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}
enum Handle {
    File(File, bool),
    Directory(cap_std::fs::ReadDir),
}
struct Server {
    root: Dir,
    read_only: bool,
    handles: HashMap<u32, Handle>,
    next: u32,
}
impl Server {
    fn writable(&self) -> io::Result<()> {
        if self.read_only {
            Err(denied())
        } else {
            Ok(())
        }
    }
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        let meta = self.root.symlink_metadata(path)?;
        if !meta.is_file() && !meta.is_dir() {
            return Err(denied());
        }
        Ok(meta)
    }
    fn file(&self, handle: u32) -> io::Result<&File> {
        match self.handles.get(&handle) {
            Some(Handle::File(file, _)) => Ok(file),
            _ => Err(invalid()),
        }
    }
    fn opened(&mut self, id: u32, handle: Handle) -> io::Result<Vec<u8>> {
        if self.handles.len() >= HANDLES {
            return Err(io::Error::other("Too many open files in shared folder"));
        }
        self.next = self.next.checked_add(1).ok_or_else(invalid)?;
        self.handles.insert(self.next, handle);
        let mut out = vec![102];
        u32(&mut out, id);
        string(&mut out, self.next.to_string().as_bytes());
        Ok(out)
    }
    fn open(
        &self,
        path: &Path,
        write: bool,
        create: bool,
        exclusive: bool,
        append: bool,
    ) -> io::Result<File> {
        match self.metadata(path) {
            Ok(meta) if meta.is_file() => {}
            Err(e) if create && e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
            _ => return Err(denied()),
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(write)
            .create(create)
            .create_new(exclusive)
            .append(append)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let file = self.root.open_with(path, &options)?.into_std();
        if !file.metadata()?.is_file() {
            return Err(denied());
        }
        Ok(file)
    }
    fn request(&mut self, kind: u8, id: u32, r: &mut Reader<'_>) -> io::Result<Vec<u8>> {
        let mut out = vec![];
        match kind {
            3 => {
                let path = r.path()?;
                let flags = r.u32()?;
                let attributes = Attrs::read(r)?;
                r.done()?;
                if flags & !0x3f != 0 || flags & 3 == 0 {
                    return Err(invalid());
                }
                let writing = flags & 0x3e != 0;
                if writing {
                    self.writable()?;
                }
                let creating = flags & (8 | 32) != 0
                    && self
                        .root
                        .symlink_metadata(&path)
                        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound);
                let file = self.open(
                    &path,
                    writing,
                    flags & 8 != 0,
                    flags & 32 != 0,
                    flags & 4 != 0,
                )?;
                if flags & 16 != 0 {
                    self.writable()?;
                    file.set_len(0)?;
                }
                // SSHFS supplies attributes when creating files. Never apply
                // them to ordinary opens, which must remain side-effect free.
                if creating && writing {
                    attributes.apply(&file)?;
                }
                return self.opened(id, Handle::File(file, flags & 4 != 0));
            }
            4 => {
                let h = r.handle()?;
                r.done()?;
                self.handles.remove(&h).ok_or_else(invalid)?;
            }
            5 => {
                let h = r.handle()?;
                let offset = r.u64()?;
                let size = r.u32()? as usize;
                r.done()?;
                let mut bytes = vec![0; size.min(BLOCK)];
                let count = self.file(h)?.read_at(&mut bytes, offset)?;
                if count == 0 {
                    return Ok(status(id, 1, "End of file"));
                }
                out.push(103);
                u32(&mut out, id);
                string(&mut out, &bytes[..count]);
                return Ok(out);
            }
            6 => {
                self.writable()?;
                let h = r.handle()?;
                let offset = r.u64()?;
                let bytes = r.bytes()?;
                r.done()?;
                if bytes.len() > BLOCK {
                    return Err(invalid());
                }
                let mut file = self.file(h)?;
                if matches!(self.handles.get(&h), Some(Handle::File(_, true))) {
                    file.write_all(bytes)?;
                    return Ok(status(id, 0, "OK"));
                }
                let mut at = 0;
                while at < bytes.len() {
                    let n = file.write_at(
                        &bytes[at..],
                        offset.checked_add(at as u64).ok_or_else(invalid)?,
                    )?;
                    if n == 0 {
                        return Err(io::ErrorKind::WriteZero.into());
                    }
                    at += n;
                }
            }
            7 | 17 => {
                let path = r.path()?;
                r.done()?;
                out.push(105);
                u32(&mut out, id);
                attrs(&mut out, &self.metadata(&path)?, self.read_only);
                return Ok(out);
            }
            8 => {
                let h = r.handle()?;
                r.done()?;
                out.push(105);
                u32(&mut out, id);
                attrs(
                    &mut out,
                    &Metadata::from_file(self.file(h)?)?,
                    self.read_only,
                );
                return Ok(out);
            }
            9 | 10 => {
                self.writable()?;
                let (path, handle) = if kind == 9 {
                    (Some(r.path()?), None)
                } else {
                    (None, Some(r.handle()?))
                };
                let attributes = Attrs::read(r)?;
                r.done()?;
                if let Some(path) = path {
                    let meta = self.metadata(&path)?;
                    let file = if meta.is_dir() {
                        self.root.open_dir(&path)?.into_std_file()
                    } else {
                        self.open(&path, true, false, false, false)?
                    };
                    attributes.apply(&file)?;
                } else {
                    attributes.apply(self.file(handle.unwrap())?)?;
                }
            }
            11 => {
                let path = r.path()?;
                r.done()?;
                self.metadata(&path)?;
                return self.opened(id, Handle::Directory(self.root.read_dir(path)?));
            }
            12 => {
                let h = r.handle()?;
                r.done()?;
                let Some(Handle::Directory(entries)) = self.handles.get_mut(&h) else {
                    return Err(invalid());
                };
                let mut names = vec![];
                let mut count = 0;
                // Only iterator exhaustion means EOF. An arbitrary run of
                // excluded links/special files must not hide later files.
                for entry in entries.by_ref() {
                    let entry = entry?;
                    let meta = entry.metadata()?;
                    if !meta.is_file() && !meta.is_dir() {
                        continue;
                    }
                    let name = entry.file_name();
                    let name = name.as_bytes();
                    if name.len() > 4096 {
                        return Err(invalid());
                    }
                    string(&mut names, name);
                    string(&mut names, name);
                    attrs(&mut names, &meta, self.read_only);
                    count += 1;
                    if count == 64 {
                        break;
                    }
                }
                if count == 0 {
                    return Ok(status(id, 1, "End of directory"));
                }
                out.push(104);
                u32(&mut out, id);
                u32(&mut out, count);
                out.extend(names);
                return Ok(out);
            }
            13..=15 => {
                self.writable()?;
                let path = r.path()?;
                let attributes = if kind == 14 {
                    Attrs::read(r)?
                } else {
                    Attrs::default()
                };
                r.done()?;
                if path == Path::new(".") {
                    return Err(denied());
                }
                match kind {
                    13 => {
                        self.metadata(&path)?;
                        self.root.remove_file(path)?;
                    }
                    14 => {
                        self.root.create_dir(&path)?;
                        attributes.apply(&self.root.open_dir(&path)?.into_std_file())?;
                    }
                    _ => {
                        self.metadata(&path)?;
                        self.root.remove_dir(path)?;
                    }
                }
            }
            16 => {
                let path = r.path()?;
                r.done()?;
                self.metadata(&path)?;
                out.push(104);
                u32(&mut out, id);
                u32(&mut out, 1);
                let path = format!(
                    "/{}",
                    if path == Path::new(".") {
                        String::new()
                    } else {
                        path.to_string_lossy().into_owned()
                    }
                );
                string(&mut out, path.as_bytes());
                string(&mut out, path.as_bytes());
                u32(&mut out, 0);
                return Ok(out);
            }
            18 => {
                self.writable()?;
                let from = r.path()?;
                let to = r.path()?;
                r.done()?;
                self.metadata(&from)?;
                if from == Path::new(".") || to == Path::new(".") {
                    return Err(denied());
                }
                // Use the atomic no-replace primitive for v3 RENAME; its
                // POSIX extension below explicitly permits replacement.
                #[cfg(target_os = "linux")]
                {
                    let source = self.root.open_dir(
                        from.parent()
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or(Path::new(".")),
                    )?;
                    let target = self.root.open_dir(
                        to.parent()
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or(Path::new(".")),
                    )?;
                    let from =
                        std::ffi::CString::new(from.file_name().ok_or_else(invalid)?.as_bytes())
                            .map_err(|_| invalid())?;
                    let to = std::ffi::CString::new(to.file_name().ok_or_else(invalid)?.as_bytes())
                        .map_err(|_| invalid())?;
                    if unsafe {
                        libc::syscall(
                            libc::SYS_renameat2,
                            source.as_raw_fd(),
                            from.as_ptr(),
                            target.as_raw_fd(),
                            to.as_ptr(),
                            libc::RENAME_NOREPLACE,
                        )
                    } != 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                }
                #[cfg(not(target_os = "linux"))]
                return Ok(status(id, 8, "Use POSIX rename on this host"));
            }
            200 => {
                let extension = r.string()?;
                if extension == "posix-rename@openssh.com" {
                    self.writable()?;
                    let from = r.path()?;
                    let to = r.path()?;
                    r.done()?;
                    self.metadata(&from)?;
                    if from == Path::new(".") || to == Path::new(".") {
                        return Err(denied());
                    }
                    self.root.rename(from, &self.root, to)?;
                } else if extension == "fsync@openssh.com" {
                    let h = r.handle()?;
                    r.done()?;
                    self.file(h)?.sync_all()?;
                } else {
                    return Ok(status(id, 8, "Unsupported SFTP extension"));
                }
            }
            _ => return Ok(status(id, 8, "Unsupported shared-folder operation")),
        }
        Ok(status(id, 0, "OK"))
    }
}

pub(super) fn serve(export: &Export, input: &mut impl Read, output: &mut impl Write) -> Result<()> {
    let root = crate::files::expand_home(&export.root).canonicalize()?;
    ensure!(
        root != Path::new("/")
            && std::env::var_os("HOME").is_none_or(|home| PathBuf::from(home)
                .canonicalize()
                .ok()
                .as_ref()
                != Some(&root)),
        "Select a specific source subfolder"
    );
    let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority())?;
    let mut server = Server {
        root: dir,
        read_only: export.read_only,
        handles: HashMap::new(),
        next: 0,
    };
    let mut initialized = false;
    loop {
        let mut length = [0; 4];
        let first = input.read(&mut length[..1])?;
        if first == 0 {
            return Ok(());
        }
        input.read_exact(&mut length[1..])?;
        let length = u32::from_be_bytes(length) as usize;
        ensure!(
            (5..=MAX_PACKET).contains(&length),
            "SFTP packet exceeds its size limit"
        );
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes)?;
        let kind = bytes[0];
        let mut r = Reader(&bytes[1..]);
        let id = r.u32()?;
        if kind == 1 {
            ensure!(!initialized, "Repeated SFTP initialization");
            initialized = true;
            let mut version = vec![2];
            u32(&mut version, 3);
            string(&mut version, b"posix-rename@openssh.com");
            string(&mut version, b"1");
            string(&mut version, b"fsync@openssh.com");
            string(&mut version, b"1");
            packet(output, &version)?;
            continue;
        }
        ensure!(initialized, "SFTP used before initialization");
        let response = server.request(kind, id, &mut r).unwrap_or_else(|error| {
            status(
                id,
                match error.kind() {
                    io::ErrorKind::NotFound => 2,
                    io::ErrorKind::PermissionDenied => 3,
                    io::ErrorKind::InvalidData => 5,
                    _ => 4,
                },
                &error.to_string(),
            )
        });
        packet(output, &response)?;
    }
}

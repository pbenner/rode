//! Byte patches, protected local/Python saves, static object parsing and sandbox access;
//! inspected programs never execute.

use base64::{Engine, engine::general_purpose::STANDARD};
use rode::{
    binary::{self, Buffer},
    files::{self, Request},
    images::Budget,
    sandbox,
};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
    sync::Arc,
};
fn peer(request: &Request) -> anyhow::Result<files::Response> {
    let mut child = Command::new("python3")
        .args(["-c", files::REMOTE_HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(request)?)?;
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
#[test]
fn byte_patches_undo_search_and_inflight_saves_preserve_arbitrary_data() {
    let bytes = Arc::new(vec![0, 0xff, 0x80, 0x90, 0xc3, 0]);
    let mut buffer = Buffer::new(bytes.clone());
    assert!(!buffer.dirty());
    buffer.patch(1, vec![0x90, 0x90]).unwrap();
    let submitted = buffer.bytes.clone();
    buffer.patch(3, vec![0xcc]).unwrap();
    buffer.acknowledge(submitted);
    assert!(buffer.dirty());
    assert_eq!(buffer.find(&[0x90, 0x90], 2), Some(1)); // wraps, including a boundary-spanning match
    assert_eq!(buffer.find(&[0x80], 0), None);
    buffer.undo();
    assert!(!buffer.dirty());
    buffer.undo();
    assert_eq!(buffer.bytes, bytes);
    buffer.redo();
    buffer.redo();
    assert_eq!(buffer.bytes[3], 0xcc);
    assert!(buffer.patch(5, vec![1, 2]).is_err());
    assert!(buffer.patch(usize::MAX, vec![1]).is_err());
    assert_eq!(binary::parse_hex("00 ff\n80").unwrap(), [0, 255, 128]);
    for bad in ["0", "xx", "🦀", "0x90", ""] {
        assert!(binary::parse_hex(bad).is_err());
    }
}
#[test]
fn binary_local_and_python_saves_match_and_detect_conflicts_without_corrupting_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("quoted ' 🦀.bin");
    let path = path.to_string_lossy().into_owned();
    let original = [0, 0xff, 0x80, 0xc3];
    fs::write(&path, original).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
    let request = Request::ReadAuto { path: path.clone() };
    for response in [files::local(&request).unwrap(), peer(&request).unwrap()] {
        assert_eq!(STANDARD.decode(response.binary.unwrap()).unwrap(), original);
        assert_eq!(response.revision, files::digest(&original));
    }
    let write = Request::WriteBinary {
        path: path.clone(),
        data: STANDARD.encode([0, 1, 0x90, 0xc3]),
        expected: Some(files::digest(&original)),
    };
    files::local(&write).unwrap();
    assert!(peer(&write).is_err());
    assert_eq!(fs::read(&path).unwrap(), [0, 1, 0x90, 0xc3]);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o751
    );
    let write = Request::WriteBinary {
        path: path.clone(),
        data: STANDARD.encode(original),
        expected: Some(files::digest(&[0, 1, 0x90, 0xc3])),
    };
    peer(&write).unwrap();
    assert!(files::local(&write).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    fs::write(&path, "plain UTF-8 🦀").unwrap();
    for response in [files::local(&request).unwrap(), peer(&request).unwrap()] {
        assert!(response.binary.is_none());
        assert!(response.text.contains('🦀'));
    }
    for response in [
        files::local(&Request::ReadHex { path: path.clone() }).unwrap(),
        peer(&Request::ReadHex { path: path.clone() }).unwrap(),
    ] {
        assert!(response.binary.is_some());
    }
    fs::File::create(&path)
        .unwrap()
        .set_len(binary::MAX_BYTES as u64 + 1)
        .unwrap();
    assert!(files::local(&request).is_err());
    assert!(peer(&request).is_err());
}
#[test]
fn native_x86_decode_maps_addresses_and_never_launches_inspected_code() {
    let code = [0x55, 0x48, 0x89, 0xe5, 0xb8, 0x2a, 0, 0, 0, 0xc3];
    let decoded = binary::disassemble(&code, 0, 0x401000, 64).unwrap();
    assert_eq!(decoded[0].text, "push rbp");
    assert_eq!(decoded[0].address, 0x401000);
    assert_eq!(decoded[1].text, "mov rbp,rsp");
    assert_eq!(decoded.last().unwrap().text, "ret");
    assert_eq!(decoded.last().unwrap().offset, 9);
    assert!(binary::disassemble(&code, 99, 0, 64).is_err());
    assert!(binary::disassemble(&code, 0, 0, 7).is_err());
    assert_eq!(
        binary::disassemble(&vec![0x90; 10000], 0, 0, 32)
            .unwrap()
            .len(),
        512
    );
    let report = binary::inspect(b"\0Hello binary\0\xff");
    assert_eq!(report.strings[0].offset, 1);
    assert_eq!(report.strings[0].text, "Hello binary");
    assert!(report.entropy > 0.);
    assert_eq!(report.sha256.len(), 64);
}
#[test]
fn elf_sections_symbols_and_entry_addresses_are_available() {
    if sandbox::find_program("cc").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("fixture.c");
    let exe = dir.path().join("fixture");
    fs::write(
        &source,
        "const char marker[] = \"RODE_BINARY_FIXTURE\"; int main(void) { return marker[0]; }",
    )
    .unwrap();
    assert!(
        Command::new("cc")
            .args(["-g", "-o"])
            .arg(&exe)
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    let bytes = fs::read(exe).unwrap();
    let report = binary::inspect(&bytes);
    assert_eq!(report.format, "Elf");
    assert!(report.sections.iter().any(|s| s.name == ".text"));
    assert!(
        report
            .symbols
            .iter()
            .any(|s| s.name == "main" && s.offset.is_some())
    );
    assert!(
        report
            .strings
            .iter()
            .any(|s| s.text.contains("RODE_BINARY_FIXTURE"))
    );
    assert!(report.entry_offset.is_some());
    for section in &report.sections {
        if section.kind == "UninitializedData" {
            assert!(section.offset.is_none());
        }
    }
    let budget = Arc::new(Budget::default());
    let loaded = binary::Loaded::decode(&STANDARD.encode(bytes), &budget).unwrap();
    assert!(budget.used() > 0);
    drop(loaded);
    assert_eq!(budget.used(), 0);
}
#[test]
fn active_sandbox_binary_reads_and_writes_obey_grants() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let inside = project.join("sample.bin");
    fs::write(&inside, [0, 1, 2]).unwrap();
    let outside = root.path().join("secret.bin");
    fs::write(&outside, [0, 9]).unwrap();
    let plan = sandbox::prepare(
        project.to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            project_read_only: true,
            ..Default::default()
        },
    )
    .unwrap();
    if !plan.active {
        eprintln!("bwrap unavailable: {}", plan.reason);
        return;
    }
    let helper = std::path::Path::new(env!("CARGO_BIN_EXE_rode"));
    let response = plan
        .local_request_with_helper(
            &Request::ReadHex {
                path: inside.to_string_lossy().into(),
            },
            helper,
        )
        .unwrap();
    assert_eq!(
        STANDARD.decode(response.binary.unwrap()).unwrap(),
        [0, 1, 2]
    );
    assert!(
        plan.local_request_with_helper(
            &Request::ReadHex {
                path: outside.to_string_lossy().into()
            },
            helper
        )
        .is_err()
    );
    assert!(
        plan.local_request_with_helper(
            &Request::WriteBinary {
                path: inside.to_string_lossy().into(),
                data: STANDARD.encode([9, 9, 9]),
                expected: Some(response.revision)
            },
            helper
        )
        .is_err()
    );
    assert_eq!(fs::read(inside).unwrap(), [0, 1, 2]);
}
#[test]
fn pe_and_macho_headers_use_native_parsers() {
    fn u16at(b: &mut [u8], at: usize, value: u16) {
        b[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn u32at(b: &mut [u8], at: usize, value: u32) {
        b[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn u64at(b: &mut [u8], at: usize, value: u64) {
        b[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    let mut pe = vec![0; 1024];
    pe[..2].copy_from_slice(b"MZ");
    u32at(&mut pe, 0x3c, 0x80);
    pe[0x80..0x84].copy_from_slice(b"PE\0\0");
    u16at(&mut pe, 0x84, 0x8664);
    u16at(&mut pe, 0x86, 1);
    u16at(&mut pe, 0x94, 240);
    u16at(&mut pe, 0x96, 0x22);
    u16at(&mut pe, 0x98, 0x20b);
    u32at(&mut pe, 0xa8, 0x1000);
    u64at(&mut pe, 0xb0, 0x140000000);
    u32at(&mut pe, 0xb8, 0x1000);
    u32at(&mut pe, 0xbc, 0x200);
    u32at(&mut pe, 0xd0, 0x2000);
    u32at(&mut pe, 0xd4, 0x200);
    pe[0x188..0x18d].copy_from_slice(b".text");
    u32at(&mut pe, 0x190, 4);
    u32at(&mut pe, 0x194, 0x1000);
    u32at(&mut pe, 0x198, 0x200);
    u32at(&mut pe, 0x19c, 0x200);
    u32at(&mut pe, 0x1ac, 0x60000020);
    pe[0x200..0x204].copy_from_slice(&[0x90, 0x90, 0x90, 0xc3]);
    let report = binary::inspect(&pe);
    assert_eq!(report.format, "Pe");
    assert_eq!(report.entry_offset, Some(0x200));
    assert_eq!(report.architecture, "X86_64");
    let mut mach = vec![0; 260];
    u32at(&mut mach, 0, 0xfeedfacf);
    u32at(&mut mach, 4, 0x1000007);
    u32at(&mut mach, 8, 3);
    u32at(&mut mach, 12, 2);
    u32at(&mut mach, 16, 1);
    u32at(&mut mach, 20, 152);
    u32at(&mut mach, 32, 0x19);
    u32at(&mut mach, 36, 152);
    mach[40..46].copy_from_slice(b"__TEXT");
    u64at(&mut mach, 56, 0x100000000);
    u64at(&mut mach, 64, 4096);
    u64at(&mut mach, 80, 260);
    u32at(&mut mach, 96, 1);
    mach[104..110].copy_from_slice(b"__text");
    mach[120..126].copy_from_slice(b"__TEXT");
    u64at(&mut mach, 136, 0x100000100);
    u64at(&mut mach, 144, 4);
    u32at(&mut mach, 152, 256);
    u32at(&mut mach, 168, 0x80000400);
    mach[256..].copy_from_slice(&[0x90, 0x90, 0x90, 0xc3]);
    let report = binary::inspect(&mach);
    assert_eq!(report.format, "MachO");
    assert_eq!(report.sections[0].offset, Some(256));
}

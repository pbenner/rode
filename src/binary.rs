//! Bounded binary snapshots, reversible byte patches and static analysis.
//! The inspected program is never launched; parsing and decoding need no host tools.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use object::{Object, ObjectSection, ObjectSymbol};
use std::{collections::VecDeque, sync::Arc};

/// Maximum hex-editor input size in bytes.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Maximum retained rows for bounded metadata/string tables.
pub const MAX_ROWS: usize = 4096;
const HISTORY_BYTES: usize = 1024 * 1024;

/// Object-file section with separate memory and file coordinates.
#[derive(Clone, Debug)]
pub struct Section {
    /// Sanitized section name for display.
    pub name: String,
    /// Section virtual address.
    pub address: u64,
    /// File byte offset, when backed by readable bytes.
    pub offset: Option<usize>,
    /// Section's in-memory size in bytes.
    pub size: u64,
    /// Section's file-backed size in bytes.
    pub file_size: u64,
    /// Object parser's section-kind label.
    pub kind: String,
}
/// Object symbol with an optional safe mapping to file bytes.
#[derive(Clone, Debug)]
pub struct Symbol {
    /// Sanitized symbol name.
    pub name: String,
    /// Symbol virtual address.
    pub address: u64,
    /// Symbol size in bytes, if supplied by the object format.
    pub size: u64,
    /// File byte offset, absent for undefined/unmapped symbols.
    pub offset: Option<usize>,
    /// Object parser's symbol-kind label.
    pub kind: String,
}
/// Bounded printable ASCII sequence found without executing the file.
#[derive(Clone, Debug)]
pub struct FoundString {
    /// Starting byte offset in the source snapshot.
    pub offset: usize,
    /// Printable text, truncated to the display limit.
    pub text: String,
}
/// Bounded static inspection report; unrecognized input remains inspectable as raw bytes.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Detected object format, or a raw-data label.
    pub format: String,
    /// Object architecture label when recognized.
    pub architecture: String,
    /// Object container kind, such as executable or relocatable.
    pub kind: String,
    /// Whether object integers use little-endian byte order.
    pub little_endian: bool,
    /// Object word width in bits; zero when unrecognized.
    pub bits: u32,
    /// Object entry-point virtual address.
    pub entry: u64,
    /// Entry-point file byte offset when safely mapped.
    pub entry_offset: Option<usize>,
    /// Lowercase hexadecimal SHA-256 of the inspected bytes.
    pub sha256: String,
    /// Shannon entropy in bits per byte.
    pub entropy: f64,
    /// Bounded section table.
    pub sections: Vec<Section>,
    /// Bounded symbol table, including dynamic symbols.
    pub symbols: Vec<Symbol>,
    /// Bounded printable-string table.
    pub strings: Vec<FoundString>,
    /// Bounded imported-name list.
    pub imports: Vec<String>,
    /// Bounded exported-name list.
    pub exports: Vec<String>,
    /// Explanation for raw/unsupported input or partial analysis.
    pub note: String,
}
/// Immutable loaded bytes/report with a shared preview-memory reservation.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// Shared original byte snapshot.
    pub bytes: Arc<Vec<u8>>,
    /// Static report computed from the snapshot.
    pub report: Arc<Report>,
    /// Reservation covering editing/save copies and bounded analysis metadata; released with
    /// its final owner.
    pub lease: Arc<crate::images::Lease>,
}
impl Loaded {
    /// Decode base64, enforce input/memory budgets and inspect the snapshot on a worker.
    pub fn decode(encoded: &str, budget: &Arc<crate::images::Budget>) -> Result<Self> {
        ensure!(
            encoded.len() <= MAX_BYTES.div_ceil(3) * 4,
            "Hex files exceed 8 MiB."
        );
        let bytes = STANDARD.decode(encoded).context("Invalid binary data")?;
        ensure!(bytes.len() <= MAX_BYTES, "Hex files exceed 8 MiB.");
        let lease = Arc::new(budget.reserve(bytes.len() * 4 + 5 * 1024 * 1024)?);
        let report = Arc::new(inspect(&bytes));
        Ok(Self {
            bytes: Arc::new(bytes),
            report,
            lease,
        })
    }
}

fn label(value: &str) -> String {
    value
        .chars()
        .take(256)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Inspect bytes without executing them; callers bound input size, while this function bounds
/// retained report tables.
pub fn inspect(bytes: &[u8]) -> Report {
    let mut counts = [0usize; 256];
    for &byte in bytes {
        counts[byte as usize] += 1;
    }
    let entropy = counts
        .iter()
        .filter(|&&n| n > 0)
        .map(|&n| {
            let p = n as f64 / bytes.len().max(1) as f64;
            -p * p.log2()
        })
        .sum();
    let mut report = Report {
        format: "Raw data".into(),
        sha256: crate::files::digest(bytes),
        entropy,
        ..Default::default()
    };
    let mut start = 0;
    for end in 0..=bytes.len() {
        if end < bytes.len() && (bytes[end].is_ascii_graphic() || bytes[end] == b' ') {
            continue;
        }
        if end - start >= 4 && report.strings.len() < MAX_ROWS {
            report.strings.push(FoundString {
                offset: start,
                text: String::from_utf8_lossy(&bytes[start..end.min(start + 256)]).into_owned(),
            });
        }
        start = end + 1;
    }
    let Ok(file) = object::File::parse(bytes) else {
        report.note = "Raw data or unsupported object container. Hex, search and explicit x86 decoding remain available.".into();
        return report;
    };
    report.format = format!("{:?}", file.format());
    report.kind = format!("{:?}", file.kind());
    report.architecture = format!("{:?}", file.architecture());
    report.bits = if file.is_64() { 64 } else { 32 };
    report.little_endian = file.is_little_endian();
    report.entry = file.entry();
    for section in file.sections().take(1024) {
        report.sections.push(Section {
            name: label(section.name().unwrap_or("<unnamed>")),
            address: section.address(),
            offset: section.file_range().and_then(|(offset, size)| {
                (size > 0 && offset < bytes.len() as u64).then_some(offset as usize)
            }),
            file_size: section.file_range().map(|(_, size)| size).unwrap_or(0),
            size: section.size(),
            kind: format!("{:?}", section.kind()),
        });
    }
    report.entry_offset = address_offset(&report.sections, report.entry, bytes.len());
    for symbol in file.symbols().chain(file.dynamic_symbols()).take(MAX_ROWS) {
        report.symbols.push(Symbol {
            name: label(symbol.name().unwrap_or("<unnamed>")),
            address: symbol.address(),
            size: symbol.size(),
            offset: if symbol.is_undefined() {
                None
            } else {
                address_offset(&report.sections, symbol.address(), bytes.len())
            },
            kind: if symbol.is_undefined() {
                "Import".into()
            } else {
                format!("{:?}", symbol.kind())
            },
        });
    }
    if let Ok(imports) = file.imports() {
        report.imports = imports
            .into_iter()
            .take(MAX_ROWS)
            .map(|import| {
                format!(
                    "{}  {}",
                    label(&String::from_utf8_lossy(import.library())),
                    label(&String::from_utf8_lossy(import.name()))
                )
            })
            .collect();
    }
    if let Ok(exports) = file.exports() {
        report.exports = exports
            .into_iter()
            .take(MAX_ROWS)
            .map(|export| {
                format!(
                    "{:016X}  {}",
                    export.address(),
                    label(&String::from_utf8_lossy(export.name()))
                )
            })
            .collect();
    }
    report.note = "Static snapshot. Tables are capped at 4096 entries (sections: 1024); strings are ASCII, minimum 4 bytes, displayed up to 256 characters. Compressed/debug data is not expanded.".into();
    report
}

/// Map a virtual address through file-backed sections, rejecting offsets beyond `len`.
pub fn address_offset(sections: &[Section], address: u64, len: usize) -> Option<usize> {
    sections.iter().find_map(|section| {
        let delta = address.checked_sub(section.address)?;
        if delta >= section.file_size {
            return None;
        }
        let offset = section.offset?.checked_add(usize::try_from(delta).ok()?)?;
        (offset < len).then_some(offset)
    })
}

/// One decoded native x86 instruction.
#[derive(Clone, Debug)]
pub struct Instruction {
    /// Instruction's file byte offset.
    pub offset: usize,
    /// Instruction address shown by the decoder.
    pub address: u64,
    /// Hexadecimal representation of the instruction bytes.
    pub bytes: String,
    /// Intel-syntax instruction text.
    pub text: String,
}
/// Decode a bounded x86 instruction window using the explicit 16/32/64-bit mode; no code
/// executes.
pub fn disassemble(
    bytes: &[u8],
    offset: usize,
    address: u64,
    bits: u32,
) -> Result<Vec<Instruction>> {
    use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};
    ensure!(
        [16, 32, 64].contains(&bits),
        "Choose x86 16, 32 or 64 bits."
    );
    ensure!(offset <= bytes.len(), "Offset is beyond the file.");
    let end = bytes.len().min(offset.saturating_add(8192));
    let mut decoder = Decoder::with_ip(bits, &bytes[offset..end], address, DecoderOptions::NONE);
    let mut formatter = IntelFormatter::new();
    let mut result = Vec::new();
    while decoder.can_decode() && result.len() < 512 {
        let at = decoder.position();
        let instruction = decoder.decode();
        let mut text = String::new();
        formatter.format(&instruction, &mut text);
        result.push(Instruction {
            offset: offset + at,
            address: instruction.ip(),
            bytes: bytes[offset + at..offset + decoder.position()]
                .iter()
                .map(|b| format!("{b:02X} "))
                .collect(),
            text,
        });
    }
    Ok(result)
}

/// Parse whitespace-separated hexadecimal byte pairs, rejecting malformed or excessive patches.
pub fn parse_hex(text: &str) -> Result<Vec<u8>> {
    let digits: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    ensure!(
        !digits.is_empty() && digits.len().is_multiple_of(2),
        "Enter pairs of hex digits, e.g. 90 90 C3."
    );
    ensure!(
        digits.len() <= 8192,
        "Use at most 4096 bytes per patch/search."
    );
    ensure!(
        digits.is_ascii() && digits.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid hex digits."
    );
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).map_err(Into::into))
        .collect()
}

#[derive(Clone)]
struct Patch {
    offset: usize,
    before: Vec<u8>,
    after: Vec<u8>,
}
/// Editable byte snapshot with bounded undo/redo and a separately acknowledged save baseline.
pub struct Buffer {
    /// Current shared byte snapshot; mutation uses copy-on-write.
    pub bytes: Arc<Vec<u8>>,
    /// Most recently acknowledged saved snapshot, which may differ from the current bytes.
    pub saved: Arc<Vec<u8>>,
    undo: VecDeque<Patch>,
    redo: Vec<Patch>,
    history_bytes: usize,
}
impl Buffer {
    /// Create a clean buffer from loaded bytes.
    pub fn new(bytes: Arc<Vec<u8>>) -> Self {
        Self {
            saved: bytes.clone(),
            bytes,
            undo: VecDeque::new(),
            redo: vec![],
            history_bytes: 0,
        }
    }
    /// Return whether current bytes differ from the acknowledged baseline.
    pub fn dirty(&self) -> bool {
        self.bytes != self.saved
    }
    /// Overwrite an in-bounds byte range without changing file length; record bounded
    /// reversible history.
    pub fn patch(&mut self, offset: usize, replacement: Vec<u8>) -> Result<()> {
        let end = offset
            .checked_add(replacement.len())
            .context("Offset overflow")?;
        ensure!(
            !replacement.is_empty() && replacement.len() <= 4096 && end <= self.bytes.len(),
            "Overwrite existing bytes only (up to 4096 per patch)."
        );
        let before = self.bytes[offset..end].to_vec();
        if before == replacement {
            return Ok(());
        }
        self.redo.clear();
        self.undo.push_back(Patch {
            offset,
            before,
            after: replacement.clone(),
        });
        self.history_bytes += replacement.len() * 2;
        while self.history_bytes > HISTORY_BYTES || self.undo.len() > 4096 {
            if let Some(patch) = self.undo.pop_front() {
                self.history_bytes -= patch.before.len() * 2;
            }
        }
        Arc::make_mut(&mut self.bytes)[offset..end].copy_from_slice(&replacement);
        Ok(())
    }
    /// Undo the latest retained patch, if any.
    pub fn undo(&mut self) {
        if let Some(patch) = self.undo.pop_back() {
            Arc::make_mut(&mut self.bytes)[patch.offset..patch.offset + patch.before.len()]
                .copy_from_slice(&patch.before);
            self.history_bytes -= patch.before.len() * 2;
            self.redo.push(patch);
        }
    }
    /// Redo the latest undone patch, if any.
    pub fn redo(&mut self) {
        if let Some(patch) = self.redo.pop() {
            Arc::make_mut(&mut self.bytes)[patch.offset..patch.offset + patch.after.len()]
                .copy_from_slice(&patch.after);
            self.history_bytes += patch.before.len() * 2;
            self.undo.push_back(patch);
        }
    }
    /// Record the exact submitted snapshot as saved, preserving edits made while saving.
    pub fn acknowledge(&mut self, bytes: Arc<Vec<u8>>) {
        self.saved = bytes;
    }
    /// Find bytes from a file offset, wrapping once to the start; returns a byte offset.
    pub fn find(&self, needle: &[u8], from: usize) -> Option<usize> {
        if needle.is_empty() || needle.len() > self.bytes.len() {
            return None;
        }
        // KMP avoids quadratic work on repeated binary patterns.
        let mut prefix = vec![0; needle.len()];
        let mut matched = 0;
        for i in 1..needle.len() {
            while matched > 0 && needle[i] != needle[matched] {
                matched = prefix[matched - 1];
            }
            if needle[i] == needle[matched] {
                matched += 1;
            }
            prefix[i] = matched;
        }
        matched = 0;
        let mut wrapped = None;
        for (i, &byte) in self.bytes.iter().enumerate() {
            while matched > 0 && byte != needle[matched] {
                matched = prefix[matched - 1];
            }
            if byte == needle[matched] {
                matched += 1;
            }
            if matched == needle.len() {
                let offset = i + 1 - needle.len();
                if offset >= from {
                    return Some(offset);
                }
                wrapped.get_or_insert(offset);
                matched = prefix[matched - 1];
            }
        }
        wrapped
    }
}

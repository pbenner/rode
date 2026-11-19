//! Read-only Word pagination inside the private document worker.
//!
//! rdocx lays out the original package with bundled fonts and emits an in-memory
//! PDF. Hayro then provides the same resolution, caching and page rendering as
//! ordinary PDFs. No conversion executable, system font discovery, source write
//! or network request is needed; the desktop retains the original DOCX bytes.
use super::*;

/// Produce a bounded paged snapshot plus human-readable layout diagnostics.
pub(super) fn paginate(bytes: &[u8]) -> Result<(Vec<u8>, Vec<String>)> {
    let document = rdocx::Document::from_bytes(bytes)
        .context("Cannot open this Word document: invalid, encrypted or unsupported DOCX")?;
    let layout = document
        .layout_deterministic()
        .context("Cannot lay out this Word document")?;
    ensure!(
        (1..=MAX_PAGES).contains(&layout.layout.pages.len()),
        "Word previews support 1–10,000 pages."
    );
    let mut warnings = Vec::new();
    for diagnostic in &layout.layout.diagnostics {
        let message: String = diagnostic.message.chars().take(512).collect();
        if !warnings.contains(&message) {
            warnings.push(message);
        }
        if warnings.len() == 8 {
            break;
        }
    }
    let pdf = document
        .to_pdf_deterministic()
        .context("Cannot render this Word document")?;
    Ok((pdf, warnings))
}

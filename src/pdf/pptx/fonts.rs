//! Resolve authored PowerPoint typefaces on the desktop, inside the document
//! worker. Only declared families enter the renderer; Inter remains its first,
//! deterministic fallback. No font is downloaded or written into the package.

use super::*;
use fontdb::{Database, Family, Query, Style, Weight};
use std::collections::{BTreeMap, BTreeSet, HashSet};

const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
// Stay below BetterOffice's 256-face / 32-MiB-per-face limits and leave room
// for layout/raster memory within the worker's 1-GiB address-space allowance.
const MAX_FAMILIES: usize = 48;
const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 96 * 1024 * 1024;

#[derive(Default)]
struct Families {
    names: BTreeMap<String, String>,
    limited: bool,
}
impl Families {
    fn read(&mut self, bytes: &[u8]) -> Result<()> {
        use quick_xml::{Reader, events::Event};
        let mut reader = Reader::from_reader(bytes);
        let mut version = quick_xml::XmlVersion::Implicit1_0;
        loop {
            match reader.read_event()? {
                Event::Start(element) | Event::Empty(element) => {
                    for attribute in element.attributes() {
                        let attribute = attribute?;
                        if attribute.key.as_ref() == b"typeface" {
                            let family = attribute
                                .decoded_and_normalized_value(version, reader.decoder())?;
                            self.insert(&family);
                        }
                    }
                }
                Event::Decl(declaration) => version = declaration.xml_version()?,
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(())
    }
    fn insert(&mut self, family: &str) {
        let family = family.trim();
        // +mj-lt / +mn-lt refer to actual families in the theme parts, which
        // are scanned too. Registering these tokens would defeat inheritance.
        if family.is_empty() || family.starts_with('+') {
            return;
        }
        let key = family.to_lowercase();
        if self.names.contains_key(&key) {
            return;
        }
        if self.names.len() == MAX_FAMILIES || family.chars().count() > 128 {
            self.limited = true;
            return;
        }
        self.names.insert(key, family.to_owned());
    }
}

/// Read only already-parsed package parts. This includes inherited fonts in
/// themes, masters, layouts, charts and tables, without resolving external URLs
/// or maintaining a second implementation of PowerPoint's style cascade.
fn declared(package: &betteroffice_pptx::PptxPackage) -> Result<Families> {
    let mut paths = BTreeSet::from([package.presentation.part_path.as_str()]);
    paths.extend(package.slides.iter().map(|p| p.part_path.as_str()));
    paths.extend(package.layouts.iter().map(|p| p.part_path.as_str()));
    paths.extend(package.masters.iter().map(|p| p.part_path.as_str()));
    paths.extend(package.themes.iter().map(|p| p.part_path.as_str()));
    paths.extend(package.charts.iter().map(|p| p.part_path.as_str()));
    paths.extend(
        package
            .diagram_drawings
            .iter()
            .map(|p| p.part_path.as_str()),
    );
    for relationship in package.relationships.values().flatten() {
        if relationship.relationship_type.ends_with("/tableStyles")
            && let Some(path) = relationship.resolved_target.as_deref()
        {
            paths.insert(path);
        }
    }
    let mut families = Families::default();
    for path in paths {
        if path.ends_with(".xml")
            && let Some(bytes) = package.part_bytes(path)
        {
            families.read(bytes)?;
        }
    }
    Ok(families)
}

pub(super) fn register(deck: &mut betteroffice_pptx::Presentation) -> Result<Vec<String>> {
    let families = declared(deck.package())?;
    let mut database = Database::new();
    // fontdb reads native font metadata without fontconfig executables or
    // libraries. Even Inter-only decks may request installed bold/italic faces.
    if !families.names.is_empty() {
        database.load_system_fonts();
        // fontdb's standard Linux paths predate XDG_DATA_HOME overrides.
        #[cfg(target_os = "linux")]
        if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
            database.load_fonts_dir(Path::new(&path).join("fonts"));
        }
    }
    register_from(deck, families, &database)
}

/// An injected database keeps font matching testable without depending on the
/// developer's installed corporate fonts or changing process-wide font paths.
fn register_from(
    deck: &mut betteroffice_pptx::Presentation,
    families: Families,
    database: &Database,
) -> Result<Vec<String>> {
    deck.register_font("Inter", false, false, INTER)?;
    let mut total_bytes = INTER.len();
    let mut missing = Vec::new();
    let mut limited = families.limited;
    for (key, requested) in families.names {
        // fontdb's query uses case-sensitive names. Find the canonical family
        // first, then let its matcher select weight, slant and stretch.
        let canonical = database.faces().find_map(|face| {
            face.families
                .iter()
                .find_map(|(name, _)| (name.to_lowercase() == key).then_some(name.as_str()))
        });
        let mut registered = key == "inter";
        let mut seen = HashSet::new();
        for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
            // The embedded regular Inter stays deterministic. Installed style
            // variants may supplement it, just as for any other authored font.
            if key == "inter" && !bold && !italic {
                continue;
            }
            let Some(canonical) = canonical else { break };
            let Some(id) = database.query(&Query {
                families: &[Family::Name(canonical)],
                weight: if bold { Weight::BOLD } else { Weight::NORMAL },
                style: if italic { Style::Italic } else { Style::Normal },
                ..Default::default()
            }) else {
                continue;
            };
            // A family with only a regular face should be registered once;
            // BetterOffice already falls back between styles in that family.
            if !seen.insert(id) {
                continue;
            }
            let Some(face) = database.face(id) else {
                continue;
            };
            // Inter's regular face is already bundled. Do not register that
            // same regular system face as bold or italic when styles are absent.
            if key == "inter"
                && ((bold && face.weight < Weight::SEMIBOLD)
                    || (italic && face.style == Style::Normal))
            {
                continue;
            }
            let source_len = match &face.source {
                fontdb::Source::File(path) => std::fs::metadata(path).ok().map(|m| m.len()),
                fontdb::Source::Binary(data) => Some(data.as_ref().as_ref().len() as u64),
            };
            if source_len.is_none_or(|len| len > MAX_FONT_BYTES as u64) {
                limited = true;
                continue;
            }
            let Some(bytes) = database.with_face_data(id, standalone).flatten() else {
                continue;
            };
            if bytes.len() > MAX_FONT_BYTES || total_bytes + bytes.len() > MAX_TOTAL_BYTES {
                limited = true;
                continue;
            }
            if deck.register_font(&requested, bold, italic, &bytes).is_ok() {
                total_bytes += bytes.len();
                registered = true;
            }
        }
        if !registered {
            missing.push(requested);
        }
    }
    let mut warnings = Vec::new();
    if !missing.is_empty() {
        let names = missing
            .iter()
            .take(4)
            .map(|s| s.chars().take(64).collect::<String>())
            .collect::<Vec<_>>()
            .join(", ");
        let more = if missing.len() > 4 { ", …" } else { "" };
        warnings.push(format!(
            "Presentation fonts unavailable on this desktop: {names}{more}. Inter is used instead; text spacing and line breaks may differ. Install the fonts on the desktop and reload the preview."
        ));
    }
    if limited {
        warnings.push("Some presentation fonts exceeded the preview font limits; missing families use Inter and missing styles use an available family face.".into());
    }
    Ok(warnings)
}

/// BetterOffice accepts standalone TTF/OTF faces. Extract the selected TTC/OTC
/// face in memory with Fontations, preserving all tables and recalculating
/// offsets/checksums rather than accidentally registering collection face zero.
fn standalone(bytes: &[u8], index: u32) -> Option<Vec<u8>> {
    if bytes.len() > MAX_FONT_BYTES {
        return None;
    }
    if !bytes.starts_with(b"ttcf") {
        return (index == 0).then(|| bytes.to_vec());
    }
    let font = write_fonts::read::FontRef::from_index(bytes, index).ok()?;
    Some(
        write_fonts::FontBuilder::new()
            .copy_missing_tables(font)
            .build(),
    )
}

#[cfg(test)]
mod tests;

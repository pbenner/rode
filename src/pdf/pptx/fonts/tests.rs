use super::*;
use betteroffice_pptx::{EditCtx, Presentation, RenderOptions, TextStylePatch};

const MONO: &[u8] = include_bytes!("../../../../assets/fonts/JetBrainsMono-Regular.ttf");
const SLIDES: &[u8] = include_bytes!("../../../../tests/fixtures/slides.pptx");

fn deck(family: &str, bold: bool, italic: bool) -> Presentation {
    let deck = Presentation::open(SLIDES).unwrap();
    let snapshot = deck.snapshot().unwrap();
    let story = &snapshot.slides[0].shapes[0].text_stories[0];
    deck.format_text(
        &EditCtx::local("font regression"),
        &story.id,
        0,
        story.length,
        &TextStylePatch {
            font_family: Some(family.into()),
            bold: Some(bold),
            italic: Some(italic),
            ..Default::default()
        },
    )
    .unwrap();
    Presentation::open(&deck.save().unwrap()).unwrap()
}

fn pixels(deck: &Presentation) -> Vec<u8> {
    deck.render_png(
        0,
        &RenderOptions {
            scale: 0.5,
            ..Default::default()
        },
    )
    .unwrap()
    .bytes
}

/// Synthetic style metadata distinguishes selection without installed fonts.
fn styled_database(family: &str, faces: &[(bool, bool, &[u8])]) -> Database {
    let mut db = Database::new();
    let mut fonts = Database::new();
    fonts.load_font_data(INTER.to_vec());
    let template = fonts.faces().next().unwrap().clone();
    for &(bold, italic, bytes) in faces {
        let mut face = template.clone();
        face.families[0].0 = family.into();
        face.weight = if bold { Weight::BOLD } else { Weight::NORMAL };
        face.style = if italic { Style::Italic } else { Style::Normal };
        face.source = fontdb::Source::Binary(Arc::new(bytes.to_vec()));
        db.push_face_info(face);
    }
    db
}

#[test]
fn installed_faces_change_real_glyphs_and_keep_the_presentation_model() {
    let mut db = Database::new();
    db.load_font_data(MONO.to_vec());
    let mut installed = deck("jetbrains mono", false, false);
    let before = installed.snapshot().unwrap();
    let families = declared(installed.package()).unwrap();
    assert!(families.names.contains_key("jetbrains mono"));
    assert!(
        register_from(&mut installed, families, &db)
            .unwrap()
            .is_empty()
    );
    assert_eq!(installed.snapshot().unwrap(), before);
    assert!(
        installed
            .render_slide(0)
            .unwrap()
            .font_substitutions
            .is_empty()
    );

    let mut expected = deck("jetbrains mono", false, false);
    expected
        .register_font("Inter", false, false, INTER)
        .unwrap();
    expected
        .register_font("jetbrains mono", false, false, MONO)
        .unwrap();
    assert_eq!(pixels(&installed), pixels(&expected));

    let mut fallback = deck("jetbrains mono", false, false);
    let families = declared(fallback.package()).unwrap();
    let warnings = register_from(&mut fallback, families, &Database::new()).unwrap();
    assert!(warnings[0].contains("jetbrains mono"));
    assert!(warnings[0].contains("desktop"));
    let substitutions = fallback.render_slide(0).unwrap().font_substitutions;
    assert_eq!(substitutions[0].selected_family, "Inter");
    assert_ne!(pixels(&installed), pixels(&fallback));
}

#[test]
fn style_matching_uses_separate_faces_instead_of_regular_for_every_run() {
    let styles = [
        (false, false, INTER),
        (true, false, MONO),
        (false, true, MONO),
        (true, true, INTER),
    ];
    let db = styled_database("RODE Test Styles", &styles);
    for (bold, italic, bytes) in styles {
        let mut actual = deck("RODE Test Styles", bold, italic);
        let families = declared(actual.package()).unwrap();
        assert!(
            register_from(&mut actual, families, &db)
                .unwrap()
                .is_empty()
        );
        let mut expected = deck("RODE Test Styles", bold, italic);
        expected
            .register_font("Inter", false, false, INTER)
            .unwrap();
        expected
            .register_font("RODE Test Styles", bold, italic, bytes)
            .unwrap();
        assert_eq!(
            pixels(&actual),
            pixels(&expected),
            "bold={bold}, italic={italic}"
        );
    }
}

#[test]
fn inter_keeps_its_bundled_regular_and_only_adds_real_installed_styles() {
    for installed_bold in [false, true] {
        let mut styles = vec![(false, false, MONO)];
        if installed_bold {
            styles.push((true, false, MONO));
        }
        let db = styled_database("Inter", &styles);
        for bold in [false, true] {
            let mut actual = deck("Inter", bold, false);
            let families = declared(actual.package()).unwrap();
            assert!(
                register_from(&mut actual, families, &db)
                    .unwrap()
                    .is_empty()
            );
            let mut expected = deck("Inter", bold, false);
            expected
                .register_font("Inter", false, false, INTER)
                .unwrap();
            if installed_bold {
                expected.register_font("Inter", true, false, MONO).unwrap();
            }
            assert_eq!(pixels(&actual), pixels(&expected));
        }
    }
}

#[test]
fn discovery_ignores_unrelated_xml_parts_in_package_relationships() {
    let presentation = Presentation::open(SLIDES).unwrap();
    let mut package = presentation.package().clone();
    // The parsed presentation does not use this part for text layout. Font
    // discovery must not start parsing arbitrary metadata/custom XML parts.
    let mut relationship = package
        .relationships
        .values()
        .flatten()
        .next()
        .unwrap()
        .clone();
    relationship.relationship_type = "urn:test/customXml".into();
    relationship.resolved_target = Some("[Content_Types].xml".into());
    package
        .relationships
        .entry(String::new())
        .or_default()
        .push(relationship);
    assert!(package.replace_part("[Content_Types].xml", b"<unparsed broken XML".to_vec()));
    let families = declared(&package).unwrap();
    assert_eq!(families.names.values().collect::<Vec<_>>(), [&"Inter"]);
}

#[test]
fn theme_names_entities_placeholders_and_resource_limits_are_handled() {
    let mut families = Families::default();
    families.read(br#"<?xml version="1.0"?><theme><majorFont><latin typeface="Verdana"/></majorFont><minorFont><latin typeface="  verdana  "/></minorFont><font typeface="A&amp;B"/><rPr><latin typeface="+mj-lt"/><latin typeface=""/></rPr></theme>"#).unwrap();
    assert_eq!(
        families.names.values().collect::<Vec<_>>(),
        [&"A&B", &"Verdana"]
    );
    for n in 0..MAX_FAMILIES {
        families.insert(&format!("Test {n}"));
    }
    assert_eq!(families.names.len(), MAX_FAMILIES);
    assert!(families.limited);
    let mut presentation = deck("Inter", false, false);
    let warnings = register_from(&mut presentation, families, &Database::new()).unwrap();
    assert_eq!(warnings.len(), 2);
    assert!(
        warnings
            .iter()
            .all(|warning| warning.chars().count() <= 512)
    );
    assert!(standalone(b"ttcf malformed collection", 0).is_none());
    assert!(standalone(INTER, 1).is_none());
}

#[test]
fn collection_faces_are_extracted_by_index_without_changing_their_glyphs() {
    // Assemble two unmodified SFNT faces into a test TTC; directory table
    // offsets in a collection are relative to the start of the whole file.
    let mut collection = b"ttcf\0\x01\0\0\0\0\0\x02".to_vec();
    let offsets = [20, 20 + INTER.len().next_multiple_of(4)];
    for offset in offsets {
        collection.extend_from_slice(&(offset as u32).to_be_bytes());
    }
    for (bytes, offset) in [(INTER, offsets[0]), (MONO, offsets[1])] {
        collection.resize(offset, 0);
        let mut face = bytes.to_vec();
        let count = u16::from_be_bytes(face[4..6].try_into().unwrap()) as usize;
        for n in 0..count {
            let start = 12 + n * 16 + 8;
            let value = u32::from_be_bytes(face[start..start + 4].try_into().unwrap());
            face[start..start + 4].copy_from_slice(&(value + offset as u32).to_be_bytes());
        }
        collection.extend_from_slice(&face);
    }
    let mut db = Database::new();
    db.load_font_data(collection);
    assert_eq!(db.faces().count(), 2);
    assert_eq!(
        db.faces()
            .find(|f| f.families[0].0 == "JetBrains Mono")
            .unwrap()
            .index,
        1
    );
    let mut actual = deck("JetBrains Mono", false, false);
    let families = declared(actual.package()).unwrap();
    assert!(
        register_from(&mut actual, families, &db)
            .unwrap()
            .is_empty()
    );
    let mut expected = deck("JetBrains Mono", false, false);
    expected
        .register_font("JetBrains Mono", false, false, MONO)
        .unwrap();
    assert_eq!(pixels(&actual), pixels(&expected));
}

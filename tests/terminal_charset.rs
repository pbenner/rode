//! Real parser character sets, streaming boundaries and retained-screen replay.

#[test]
fn dec_graphics_and_shifted_g1_survive_every_stream_boundary() {
    let bytes = b"\x1b(0lqqkx\x1b(B plain q \x1b)0\x0elqqkx\x0f ASCII q";
    for chunk in 1..=bytes.len() {
        let mut parser = vt100::Parser::new(5, 100, 10);
        for bytes in bytes.chunks(chunk) {
            parser.process(bytes);
        }
        assert_eq!(parser.screen().contents(), "┌──┐│ plain q ┌──┐│ ASCII q");
        assert_eq!(parser.screen().cursor_position(), (0, 27));
    }
}

#[test]
fn dec_graphics_translate_only_printed_ascii_and_preserve_colours_and_utf8() {
    let mut parser = vt100::Parser::new(5, 100, 0);
    for byte in "\x1b(0\x1b]0;lqkx\x07\x1bPlqkx\x1b\\\x1b[31mlqkx λ─界\x1b(B q".as_bytes() {
        parser.process(&[*byte]);
    }
    assert_eq!(parser.screen().contents(), "┌─┐│ λ─界 q");
    assert_eq!(
        parser.screen().cell(0, 0).unwrap().fgcolor(),
        vt100::Color::Idx(1)
    );
    assert!(!parser.screen().cell(0, 0).unwrap().is_wide());
    assert!(parser.screen().cell(0, 7).unwrap().is_wide());
}

#[test]
fn dec_graphics_table_and_uk_designation_are_independent() {
    let mut parser = vt100::Parser::new(5, 100, 0);
    parser.process(b"\x1b(0_`abcdefghijklmnopqrstuvwxyz{|}~\x1b(A #\x1b(B #");
    assert_eq!(
        parser.screen().contents(),
        " ◆▒␉␌␍␊°±␤␋┘┐┌└┼⎺⎻─⎼⎽├┤┴┬│≤≥π≠£· £ #"
    );
}

#[test]
fn saved_cursor_alternate_screen_and_reset_restore_character_sets() {
    let mut parser = vt100::Parser::new(5, 80, 0);
    parser.process(b"\x1b)0\x0e\x1b7\x0fq\x1b8\x1b[2;1Hq");
    assert_eq!(parser.screen().cell(1, 0).unwrap().contents(), "─");
    parser.process(b"\x1b[?1049h\x0fq\x1b[?1049l\x1b[3;1Hx");
    assert_eq!(parser.screen().cell(2, 0).unwrap().contents(), "│");
    parser.process(b"\x1bcq");
    assert_eq!(parser.screen().contents(), "q");
}

#[test]
fn snapshots_diffs_and_rows_preserve_cells_and_subsequent_graphics_output() {
    for designation in [b"\x1b(0".as_slice(), b"\x1b)0\x0e".as_slice()] {
        let mut source = vt100::Parser::new(5, 80, 10);
        source.process(b"plain qx ");
        let previous = source.screen().clone();
        source.process(designation);
        source.process(b"lqqkx");
        for snapshot in [
            source.screen().contents_formatted(),
            source.screen().state_formatted(),
        ] {
            let mut replay = vt100::Parser::new(5, 80, 10);
            // Replay must overwrite an already selected character set too.
            replay.process(b"\x1b(0\x1b)0\x0e");
            replay.process(&snapshot);
            assert_eq!(replay.screen().contents(), source.screen().contents());
            replay.process(b"qx\x0f\x1b(B plain-q");
            assert_eq!(replay.screen().contents(), "plain qx ┌──┐│─│ plain-q");
        }
        let mut replay = vt100::Parser::new(5, 80, 10);
        replay.process(&previous.state_formatted());
        replay.process(&source.screen().state_diff(&previous));
        assert_eq!(replay.screen().contents(), source.screen().contents());
        replay.process(b"qx");
        assert_eq!(replay.screen().contents(), "plain qx ┌──┐│─│");
        let row = source.screen().rows_formatted(0, 80).next().unwrap();
        let mut replay = vt100::Parser::new(5, 80, 0);
        replay.process(designation);
        replay.process(&row);
        assert_eq!(replay.screen().contents(), source.screen().contents());
    }
}

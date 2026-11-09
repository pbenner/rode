# RODE terminal character-set support

Based on the published `vt100` 0.16.2 source, upstream revision
`eb66ffaf7d771c13303ef73b29f6f2a56fdacecf` from
<https://github.com/doy/vt100-rust>. The upstream MIT license is retained in
`LICENSE`. The manifest omits upstream development dependencies and the changelog
is omitted; the original parser implementation is otherwise retained except for
the character-set changes below.

RODE adds G0/G1 ASCII, UK and DEC special-graphics designation, SI/SO selection,
cursor save/restore and reset handling. Screen snapshots and diffs print their
Unicode cell contents in ASCII mode and restore character-set state afterwards,
so a reattached terminal continues to interpret subsequent output correctly.

The patch is shared by the desktop, persistent terminal service and debugger
screen cache. It avoids a second byte parser or translation at the rendering
layer. Regression coverage lives in `tests/terminal_charset.rs`, terminal UI
tests and native session tests in the RODE workspace. Replace this patch with an
upstream version once it supports the same behaviour.

Character-set designation, shifts and cursor saving follow the
[XTerm control-sequence reference](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html).

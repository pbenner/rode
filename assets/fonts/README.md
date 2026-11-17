# Bundled fonts

RODE embeds the unmodified `Inter-Regular.ttf` from the official
[Inter 4.1 release](https://github.com/rsms/inter/releases/tag/v4.1).
The font is by Rasmus Andersson and the Inter Project Authors, distributed under
the SIL Open Font License 1.1. Its complete copyright and license text are
retained in `Inter-LICENSE.txt`.

Code, terminal and other monospace text use the unmodified
`JetBrainsMono-Regular.ttf` from the official
[JetBrains Mono 2.304 release](https://github.com/JetBrains/JetBrainsMono/releases/tag/v2.304).
Its complete SIL Open Font License 1.1 and copyright notice are retained in
`JetBrainsMono-OFL.txt`.

`manifest.json` pins both release archives and their extracted files by SHA-256.
When updating the font, review the upstream release and license, then update the
binary, complete notice and manifest together. No font modifications are made.

Both faces are compiled into the editor and installed once per UI context,
before its first native frame. There is no runtime download or system-font dependency.
Proportional interface text uses Inter; monospace text uses JetBrains Mono.
Existing font-size preferences and Unicode/emoji fallbacks remain available.

egui rasterizes outline glyphs into its DPI-aware atlas using grayscale coverage
and linear texture filtering; text is aligned to physical pixels. It does not
use the operating system's RGB subpixel renderer or TrueType hinting engine.
Typeface choice therefore matters at small sizes. Ligatures are not enabled by
installing a font: source and terminal text keep egui's character-based layout.

`rode --theme-licenses` prints both complete font notices. The Debian notice
collector includes this output in `THIRD-PARTY-NOTICES.gz`; standalone binary
distributions must retain the applicable font notices too.

# Interface font

RODE embeds the unmodified `Inter-Regular.ttf` from the official
[Inter 4.1 release](https://github.com/rsms/inter/releases/tag/v4.1).
The font is by Rasmus Andersson and the Inter Project Authors, distributed under
the SIL Open Font License 1.1. Its complete copyright and license text are
retained in `Inter-LICENSE.txt`.

`manifest.json` pins the release archive and both extracted files by SHA-256.
When updating the font, review the upstream release and license, then update the
binary, complete notice and manifest together. No font modifications are made.

The face is compiled into the editor and installed once per UI context, before
its first native frame. There is no runtime download or system-font dependency.
Only proportional interface text uses Inter; code and terminal text retain the
existing monospace family and font-size preference. Existing Unicode and emoji
fallbacks remain available.

`rode --theme-licenses` prints the complete Inter notice. The Debian notice
collector includes this output in `THIRD-PARTY-NOTICES.gz`; standalone binary
distributions must retain the applicable font notices too.

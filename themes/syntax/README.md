# SynthWave ’84 source

The embedded `synthwave84.json` adapts all 88 TextMate token rules from
[Robb Owen’s SynthWave ’84](https://github.com/robb0wen/synthwave-vscode),
revision `ecfa2fe1279f7233663fa3f98a96e6756000567b`.

Source: `themes/synthwave-color-theme.json`. The source’s trailing JSON comma
is removed, scope arrays become comma-separated selectors, three-digit colors
expand to six digits, and alpha foreground colors are composited against the
original `#262335` editor background. The leading dot in the upstream Elixir
`punctuation.binary.elixir` selector is corrected. The original bold metadata
is retained; the current native text renderer applies italic/underline styles.
The unstyled syntax foreground is the upstream `#b6b1b1` used by its token rules.

The interface/terminal colors in `../synthwave84.json` come from the same source.
Unset upstream ANSI black/white slots use the matching surfaces/foreground.
Bright ANSI red alone is lifted by one step per green/blue channel to meet the
existing bright-terminal contrast floor. `../synthwave84-neon.json` and native
`src/theme/glow.rs` / `src/theme/surfaces/neon.rs` adapt the color substitutions
in `src/js/theme_template.js` and the static geometry in
`src/css/editor_chrome.css`. Halos approximate CSS shadows using the existing
font atlas; they do not install or execute the upstream extension.

Copyright (c) 2019 Robb Owen. MIT license; unchanged full text:
[`packaging/licenses/synthwave84-LICENSE-MIT`](../../packaging/licenses/synthwave84-LICENSE-MIT).
The executable’s `--theme-licenses` output includes that notice, and Debian
packaging includes it through the existing embedded-theme notice collector.

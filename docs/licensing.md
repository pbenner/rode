# Artwork and licensing

RODE's own code is covered by the repository's [MIT license](https://github.com/pbenner/rode/blob/main/LICENSE). Bundled fonts, parser grammars, syntax themes and other dependencies retain their own terms.

## Icon source review

The review on 2026-10-29 inspected the icon drawing code, repository artwork, Cargo dependencies and Debian notice generation:

| Item | Implementation and finding |
| --- | --- |
| Navigation, menus and toolbars | `src/app/icons.rs` draws lines, circles and rectangles in Rust on a 24-unit grid. No imported Codicon SVGs, Codicon font or Codicon dependency was found. |
| Application icon | `assets/rode.svg` is the bracketed `[r]` artwork used for RODE's own identity. It does not contain the Visual Studio Code product logo. |
| Text and emoji fallback fonts | egui embeds third-party fonts, including an emoji/icon fallback font. These are separate from RODE's menu drawings and require their upstream notices. |

The drawing code is licensed with RODE. This source review does not establish the historical origin of every shape or decide whether visual similarity makes a particular design derivative. Drawing an icon in Rust, changing its color or resizing it does not by itself remove another work's license obligations. Familiar symbols and similar appearance are not, on their own, proof of either copying or legal clearance.

## Visual Studio Code artwork

Microsoft distinguishes code licensing, artwork licensing and trademarks:

- The [Visual Studio Code source license](https://github.com/microsoft/vscode/blob/main/LICENSE.txt) is MIT. It is not a blanket license for every related asset or brand.
- The [Codicons legal notice](https://github.com/microsoft/vscode-codicons#legal-notices) licenses artwork/content under **Creative Commons Attribution 4.0** and code under MIT. If Codicon artwork is copied or adapted, retain the required credits and notices, link the source and license, and describe modifications. See the [CC BY 4.0 terms](https://creativecommons.org/licenses/by/4.0/).
- The [Visual Studio Code branding guidelines](https://code.visualstudio.com/brand) separately restrict use of its product logo and names. RODE must use its own application identity and must not suggest Microsoft endorsement. Attribution alone does not supply trademark permission.

RODE's native drawing module does not import that artwork, and this review found no Codicon attribution omission. That finding is limited to the inspected source and dependency graph; it is not a legal clearance opinion or a guarantee against claims about similarity. Other brands also have separate policies, such as the [Python Software Foundation's trademark policy](https://www.python.org/psf/trademarks/).

## Font and distribution notices

The current [`epaint_default_fonts` dependency](https://github.com/emilk/egui/tree/0.33.3/crates/epaint_default_fonts/fonts) supplies complete notices for:

| Font | Upstream notice |
| --- | --- |
| Hack | `fonts/Hack-Regular.txt`, including MIT and Bitstream Vera notices |
| Noto Emoji | `fonts/OFL.txt`, SIL Open Font License 1.1 |
| Ubuntu Light | `fonts/UFL.txt`, Ubuntu Font Licence 1.0 |
| emoji-icon-font | `fonts/emoji-icon-font-mit-license.txt`, MIT, John Slegers |

The Debian builder collects these files from the exact Cargo dependency source and includes them, unchanged, in `/usr/share/doc/rode/THIRD-PARTY-NOTICES.gz`. It fails if an expected font notice is missing. The package also contains this review as `ARTWORK-LICENSING.md`. RODE's MIT license does not replace those font terms. Standalone binary distributions must also ship the applicable dependency and font notices.

The collector covers the workspace's normal/build dependency graph for the build host, including license directories and REUSE copyright headers. Some published crates omit their upstream license files. For these, `packaging/licenses/` keeps unchanged texts from the exact upstream revision recorded in the crate, with package versions, source URLs and SHA-256 hashes in its manifest. Packaging verifies the hashes and requires a license text for every collected dependency; an SPDX declaration alone is insufficient. Collection runs offline. When updating dependencies, review any changed or missing notices before updating the snapshots. See [Debian packaging](packaging.md) for the validation command.

`rode --syntax-licenses` and `rode --theme-licenses` print the separate acknowledgements for embedded grammars and syntax themes; they are not a complete font/dependency license listing.

## Optional Codex runtime

The separate optional `rode-codex` package redistributes the unmodified native Codex executable and its code-mode/execution helpers from the pinned [OpenAI Codex release](https://github.com/openai/codex/releases/tag/rust-v0.166.0), independently of the VS Code extension. The core editor package contains no Codex executable. Its explicit **Install Codex** action can download the same pinned runtime into the user's data directory. Codex's upstream code license is Apache 2.0; RODE's MIT license does not replace it or the terms of its dependencies.

`packaging/codex.json` records the release, architecture-specific main/helper archive and executable sizes and SHA-256 hashes, and the source/hash of each shipped notice snapshot. `packaging/codex-licenses/` preserves upstream LICENSE, NOTICE and third-party texts, including the Ratatui MIT notice referenced by Codex. Complete texts travel with the runtime and are also assembled into `/usr/share/doc/rode-codex/CODEX-NOTICES.gz`. Runtime preparation checks these pins; upgrades require reviewing and updating the manifest and notices together. RODE does not modify the executable or reuse VS Code artwork/branding.

The editor embeds the pin and these notice snapshots so user installation works without a source checkout; the installer writes complete verified notices beside the downloaded executable. Native HTTP/TLS and archive handling use Rust dependencies covered by the distribution's dependency notices, with no external downloader or extraction command required.

## Future artwork changes

Keep the native icon style, record sources for imported or adapted artwork, retain the exact upstream license and required credits, and check trademark conditions separately. Do not trace vendor icons or copy SVG paths into Rust and label them MIT merely because the representation changed. Prefer independently designed generic symbols over product logos. Review both source artwork and published screenshots when replacing an asset with uncertain provenance.

This page records a technical source and notice review. Formal clearance of disputed designs requires qualified legal review.

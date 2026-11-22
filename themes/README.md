# Theme configuration

Select **Theme: …** at the bottom right, **View → Color theme**, or **Theme: …**
in the command palette (**Ctrl+Shift+P**). Atelier Dark is the initial default;
saved choices and explicit `--theme` selections take precedence. The
[theme guide](../docs/themes.md) describes the built-in palettes and their artwork.
This reference explains custom theme files and the supported fields.

Sumi-e Light and Sumi-e Dark form an original Japanese fine-art-inspired family. Light pairs warm washi-paper surfaces with dark ink and vermilion emphasis. Dark uses neutral sumi-black surfaces, paper-white text and restrained gold accents inspired by ink-and-gold screens, with indigo, celadon and pale-violet syntax. Its backgrounds, hover states and selections stay neutral gray. An original pine-and-crane ink-wash illustration occupies unused source-editor space, with palette-aware ink and automatic collision avoidance. They share lightly rounded native controls, matching terminal colors and 4.5:1 terminal text contrast. Choose **Theme: Sumi-e Light** or **Theme: Sumi-e Dark**.

Samurai adds an obsidian-gray Japanese armor-and-ink palette with silver-steel controls and gold details. Three original silver-gray illustrations show a samurai beneath a pine in the editor, a kabuto helmet below Explorer's file tree, and a castle on the welcome screen. Source artwork keeps its viewport-based size and anchor during scrolling and hides when code overlaps it. Explorer and welcome artwork fit around their controls. Selection leaves artwork visible. Choose **Theme: Samurai** or `--theme samurai --syntax-theme ui`.

Independent code palettes are selected under **View → Syntax highlighting** or **Syntax theme: …** in the command palette. SynthWave ’84, Dracula, Catppuccin Mocha, Nord, Monokai, and One Half Dark retain their full scope rules. See [independent syntax themes and custom rules](../docs/syntax-themes.md). The existing `syntax` category overrides below apply when **Follow UI theme** is selected.

UI theme changes are saved immediately with local app preferences and restored on startup. On Linux these live in `${XDG_DATA_HOME:-~/.local/share}/rode/app.ron`, under the `color_theme` key. The optional `--theme ID` argument overrides the stored choice for startup; that choice is saved on normal exit or when another theme is selected. Built-in IDs are `samurai`, `sumie-light`, `sumie-dark`, `synthwave84`, `synthwave84-neon`, `atelier-dark`, `atelier-light`, `phosphor`, `carbon`, `carbon-edge`, `code-dark`, `matrix`, `amber`, `ice`, `neon`, `afterglow`, `knight-rider`, `hologram`, `telemetry`, `studio`, `studio-neutral`, and `studio-hacker`. A custom theme in `violet.json` has ID `custom:violet`, independent of its display name.

## Custom themes

Put UTF-8 JSON files in `${XDG_CONFIG_HOME:-~/.config}/rode/themes/` and restart RODE. The files are loaded once at startup; there is no theme watcher or background service. For example, `violet.json`:

```json
{
  "name": "Violet Carbon",
  "base": "carbon",
  "colors": {
    "accent": "#bbaaff",
    "selection": "#332b40",
    "terminal_background": "#15121a"
  },
  "syntax": {
    "comment": "#a08bb4"
  },
  "terminal": {
    "minimum_contrast": 4.5
  }
}
```

Choose **Theme: Violet Carbon** in the command palette, or launch:

```sh
rode --theme custom:violet
```

`name` is required and must have 1–64 printable characters. `base` is any built-in ID in the [theme guide](../docs/themes.md) (`phosphor` by default). All other sections are optional; omitted values inherit the base, including its appearance, syntax colors and frame treatment. Bases refer to the built-in palettes, not other custom files. The optional `appearance` field selects `"dark"` or `"light"` widget defaults. It inherits the base, so variants of `atelier-light` retain light styling. Explicit appearance stays selected when the operating system changes its color scheme.

The optional `chrome` field selects `"plain"` (standard frames), `"chamfered"` (cut corners, accent rails, subtle card gradients and static edge glow), `"holographic"` (double cut-corner outlines, illuminated corners and segmented rails), or `"bracketed"` (open corner brackets, vertical rails and hatched edge accents). It inherits the base: `"base": "afterglow"` includes the decorated frames; add `"chrome": "plain"` to keep only its colors. Frame decorations use native geometry with no animation, image assets or additional runtime dependencies. Terminal canvases retain solid backgrounds.

The separate optional `artwork` field selects `"none"`, `"sumi_e"` or `"samurai"`. Sumi-e variants inherit `"sumi_e"`; Samurai inherits `"samurai"`; other built-ins default to `"none"`. Sumi-e includes pine/crane, bamboo/bird and plum-blossom illustrations. Samurai includes a warrior landscape, silver-gray kabuto helmet and castle. Original embedded illustrations are tinted from the current palette and painted only in unused source-editor, Explorer and welcome space. Source placement has a fixed size and anchor for each viewport; visible rows after scrolling and scaling can suppress it but cannot shrink or move it. Explorer artwork stays below the file tree, and welcome artwork avoids the actual text/button bounds. A fixed budget of at most two illustrations per area also prevents artwork collisions. Dense or narrow areas suppress decoration, while text selections leave it visible. Each decoded texture is cached per native UI context and shared by its editor windows. Custom definitions cannot load image paths, URLs or scripts. Add `"artwork": "none"` to any illustrated variant to keep its colors without artwork.

The optional `icons` field selects `"outline"` (the default) or `"sumi_e"` (tapered native ink strokes, open circles and paper/brush motifs). It inherits independently of `artwork` and `chrome`; both Sumi-e themes select `"sumi_e"`. Add `"icons": "outline"` to retain standard icons while keeping the illustrations, or disable artwork while keeping ink icons. Icons use the existing button geometry, accessible labels and keyboard targets. Their treatment is installed for the whole UI context and reset on theme changes; it does not load image files, icon fonts or scripts.

Colors must be opaque `#RRGGBB` strings. Invalid colors, unknown keys, invalid bases, or malformed files are reported and skipped. Theme loading is bounded to 32 JSON files of at most 64 KiB each and a scan of at most 1,025 directory entries. Built-in palettes remain available if custom configuration is invalid.

## Component styling

Every theme uses the shared component baseline in [`InterfaceStyle`](../src/theme/interface.rs): embedded Inter interface text, rounded controls, tonal secondary buttons, soft tabs, clear input boundaries and inset keyboard-focus outlines. The optional `ui` object overrides dimensions and typography for controls, cards, dialogs and tabs. Fields are optional: omitted fields inherit the base theme's settings, or the shared defaults when the base has no overrides. An empty `"ui": {}` explicitly inherits those defaults. Colors, native frame decoration, icons and artwork remain independent of these settings.

For example, keep Studio's colors and typography with tighter spacing:

```json
{
  "name": "Studio Compact",
  "base": "studio",
  "ui": {
    "control_radius": 4,
    "item_spacing": [6, 5],
    "surface_padding": 10
  }
}
```

Dimensions use logical points and follow **Interface scale**. These settings do not change the saved editor/terminal font size or independent syntax selection. Switching to a theme without `ui` restores the shared component defaults, so overrides cannot leak between themes. Decorated presets keep their angular card and dialog frames unless an explicit `ui` override supplies corner radii.

| Key | Purpose | Accepted range |
| --- | --- | --- |
| `control_radius` | Buttons, inputs and selected tabs | Integer 0–16 |
| `surface_radius` | Tool cards | Integer 0–24 |
| `popup_radius` | Menus and dialogs | Integer 0–24 |
| `control_height` | Minimum ordinary control height | 22–40 |
| `button_padding` | Horizontal/vertical inner padding | `[4–20, 2–12]` |
| `item_spacing` | Horizontal/vertical gaps | `[2–16, 2–16]` |
| `surface_padding` | Padding inside cards | Integer 8–24 |
| `popup_padding` | Padding inside windows/dialogs | Integer 8–28 |
| `body_size` | Interface body text | 12–18 |
| `button_size` | Button and tab text | 12–18 |
| `heading_size` | Headings; tool-page titles use two points less | 18–28 |
| `small_size` | Secondary labels | 10–14 |
| `border_width` | Ordinary control and surface borders | 0.5–2 |
| `focus_width` | Focus/pressed outline | 1–3, at least `border_width` |
| `disabled_alpha` | Disabled-control opacity | 0.35–0.75 |

Unknown fields and invalid or nonfinite values are rejected with the theme's loading warning. Controls use separate normal, hover, focus/pressed and open states; menus and dialogs use a soft neutral shadow. Frame decoration remains static and independent of component styling.

## Palette keys

The `colors` object accepts:

| Key | Purpose |
| --- | --- |
| `background` | Editor and main window background |
| `panel` | Explorer, title area, and inactive tab surfaces |
| `raised` | Buttons, status bar, find bar, and notifications |
| `border` | Dividers and outlines |
| `control_border` | Optional control outline for `ui` styles; falls back to `border` while surface borders keep their own color |
| `text` | Main UI and default custom-syntax text |
| `muted` | Secondary UI text |
| `accent` | Highlights, active controls, cursor, and links |
| `frame_accent` | Decorative frames and edge glow with decorated chrome |
| `heading` | Section headings with decorated chrome |
| `success` | Positive connection and notification status |
| `error` | Errors and disconnected status |
| `selection` | Selection background |
| `hover` | Hovered controls |
| `active` | Pressed/open controls |
| `gutter` | Inactive line numbers |
| `terminal_background` | Terminal default background |
| `terminal_foreground` | Terminal default text |

The `syntax` object accepts `comment`, `keyword`, `type`, `function`, `string`, `number`, `constant`, `variable`, `operator`, `punctuation`, `builtin`, and `attribute`. These map to the existing parser's scopes; this is syntax coloring, not semantic analysis. Phosphor's root syntax colors live in `phosphor.json` and use the same data parser as the derived themes. Its mint/jade/gold palette and custom variants retain coordinated colors for categories without an explicit override.

Built-ins keep divider borders quiet while `control_border` gives inputs a
readable boundary. Normal, hover, pressed and open states are checked separately:
7:1 primary interface text, 4.5:1 secondary and status text, and 3:1 input
boundaries. Line numbers meet 4.5:1 against the editor. Native semantic syntax
palettes meet 4.5:1 over their matching editor and selection fills. The imported
SynthWave scope rules retain upstream colors, including lower-contrast tokens;
its interface controls and diagnostic colors follow the shared readability
checks. Filled primary-action text reaches 7:1 in native palettes and 4.5:1 with
SynthWave's original accent. Custom palettes remain free to choose other values.

Bold basic foreground colors (SGR 1 plus 30–37) use bright entries 8–15, matching [xterm's default boldColors setting](https://www.invisible-island.net/xterm/manpage/xterm.html). This does not brighten backgrounds, RGB colors or already-bright/extended indices. SGR 22/0 restores normal intensity.

Built-in palettes set `terminal_foreground` and ANSI entry 7 (White) to the UI text colour, with a stronger matching entry 15 (Bright white): lighter on dark themes and darker in light themes. Prompt/path slots follow the theme: warm gold in Carbon/Carbon Edge/Amber, green in Phosphor/Matrix/Studio Hacker, cool blue in Ice/Code Dark/Afterglow/Knight Rider/Hologram/Studio/Atelier, silver-blue in Studio Neutral, violet in Neon, turquoise in Telemetry and muted indigo in Sumi-e. ANSI colour names identify slots; a themed Blue slot need not be literal blue. Bright foreground entries meet at least 4.5:1 contrast against the default terminal background. Red and green remain distinct for error/success indicators. Custom definitions and saved swatch overrides may explicitly choose other values.

The optional `ansi` array overrides the terminal's first 16 indexed colors in standard order: black, red, green, yellow, blue, magenta, cyan, white, followed by their bright variants. All 16 entries are required if the array is supplied. Extended 256-color indices and explicit RGB colors use their standard values before contrast correction.

The optional `terminal.minimum_contrast` is a number from **1** to **21**, inherited from the base theme when omitted. Atelier and Sumi-e default to **4.5**; the other built-in themes use **1** (off). Custom themes inherit their base setting; use a value such as **4.5** to opt into contrast correction. Rendered text below this ratio is mixed toward white or black against its actual cell background, including inverted cells and RGB/256-color prompts. Backgrounds, spaces, Powerline separators and box/block glyphs are preserved. Unattainable ratios use whichever of black/white has the best contrast. **1** disables correction for exact terminal colors.

**View → Terminal colors…** also lets users override foreground/background, individual ANSI colors and minimum contrast without editing JSON. These desktop preferences are saved immediately per theme ID; they take precedence over theme files. **Reset to theme defaults** restores the file/base values. Switching palettes or adjusting terminal colors does not restart a shell or change its prompt/environment.

## Built-in definitions

[sumie-light.json](sumie-light.json), [sumie-dark.json](sumie-dark.json), [carbon.json](carbon.json), [carbon-edge.json](carbon-edge.json), [code-dark.json](code-dark.json), [matrix.json](matrix.json), [amber.json](amber.json), [ice.json](ice.json), [neon.json](neon.json), [afterglow.json](afterglow.json), [knight-rider.json](knight-rider.json), [hologram.json](hologram.json), [telemetry.json](telemetry.json), [studio.json](studio.json), [studio-neutral.json](studio-neutral.json), and [studio-hacker.json](studio-hacker.json) define the palettes embedded in the binary. Carbon Edge inherits Carbon’s color, syntax and terminal palette and adds its frame treatment. Studio opts into shared component defaults; Studio Neutral and Studio Hacker inherit them from Studio. Built-in bases resolve by ID independently of menu order; unknown bases and inheritance cycles are errors. Phosphor remains defined in Rust. Editing a source palette requires rebuilding. To customize without rebuilding, create a user theme with, for example, `"base": "matrix"` in the configuration directory.

Themes are local data. Loading or switching one never executes theme code, fetches resources, reconnects SSH, or restarts a terminal. Theme switches invalidate cached highlighting so already-open documents immediately use the new palette.

## Extending the built-in collection

Add a JSON definition and register its stable ID in [`src/theme/definition.rs`](../src/theme/definition.rs). Use `base` to share a palette or component style, and include only the differences. Catalog order controls the theme menu; it does not constrain inheritance. Custom files may inherit built-ins, including variants, but do not load other custom files.

All presets use the same component builders and egui installer. `appearance`, `colors`, `ui`, `chrome`, `syntax` and `ansi` are independent roles; views must use those roles rather than branch on a preset ID. Run `cargo test --locked -p rode --profile ci --lib --test themes` when adding a variant to check inheritance, parsing, theme switching, contrast and rendered layout.

For native checks, run `scripts/test-ui.py` with the UI harness dependencies described in [development](../docs/development.md). Run the 1×/2× DPI suite with `xvfb-run -a -s "-screen 0 3200x2200x24" python3 scripts/test-scale-ui.py`; its fixed menu targets assume the doubled window fits on the virtual display.

## Full syntax bases and flat neon chrome

`syntax_theme` selects a registered full-scope syntax base (for example
`"syntax_theme": "synthwave84"`) for **Follow UI theme**. It is inherited by
custom interface themes. The existing `syntax` map adds category rules to that
base. `syntax_glow` is an inherited boolean selecting the static Neon Dreams
text treatment; it defaults to false. Independently selected syntax themes keep
their own colors and glow setting. Custom syntax definitions can use
`"base": "synthwave84"` and optional `"glow": true`; `"base": "plain"` starts
an empty scope map.

`"chrome": "neon"` uses flat native surfaces with gradient active tabs and
activity rails. It does not add the margins or corner frames of decorated
chrome. Optional color roles are `activity_background` (otherwise `panel`),
`status_background` (otherwise `raised`), `cursor` (otherwise `accent`), and
`editor_gradient` (the bottom-quarter wash for neon chrome, otherwise flat).
All colors keep the existing `#RRGGBB` format. Glow painting is clipped and bounded;
widget identity and editing geometry do not depend on these settings.

SynthWave’s pinned sources, color normalization and MIT attribution are recorded
in [the adaptation notes](syntax/README.md).

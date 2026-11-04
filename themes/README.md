# Theme configuration

Select a theme from the **Theme: …** control at the bottom right, under **View → Color theme**, or use **Ctrl+Shift+P** and search for **Theme:**. Phosphor is the original green theme; Carbon is inspired by the supplied near-black reference screenshot. Carbon Edge keeps Carbon’s colours and adds amber chamfered frames with static glow. Code Dark adds neutral charcoal surfaces, light text and blue accents inspired by VS Code's classic dark theme. Matrix adds vivid green, Amber warm retro colors, Ice cold blue/cyan, and Neon magenta/cyan. Afterglow combines charcoal-violet surfaces, magenta cut-corner frames, static edge glow, cyan selections, cream headings and rose secondary text, inspired by the supplied dashboard screenshots. Knight Rider uses near-black surfaces, red cut-corner frames and static glow, silver text, amber syntax accents and cool instrument-blue terminal prompts. Hologram uses midnight navy surfaces, cyan double-outline frames with clipped corners, segmented accent rails and pale cyan text, inspired by the supplied holographic loading-panel screenshot. Telemetry pairs near-black instrument panels with turquoise corner brackets, hatched edge accents and mint text, inspired by the supplied sci-fi instrument-screen reference. Studio adds graphite surfaces, a blue-violet accent and shared component settings for rounded controls, typography, spacing and interaction states. Studio Neutral inherits Studio’s component settings with neutral charcoal surfaces, silver accents and sage/sand syntax colors. Studio Hacker inherits the same components with green-black surfaces, green accents and matching code and terminal colors. These are native RODE palettes, rather than imports of third-party editor themes.

Independent code palettes are selected under **View → Syntax highlighting** or **Syntax theme: …** in the command palette. Dracula, Catppuccin Mocha, Nord, Monokai, and One Half Dark retain their full scope rules. See [independent syntax themes and custom rules](../docs/syntax-themes.md). The existing `syntax` category overrides below apply when **Follow UI theme** is selected.

UI theme changes are saved immediately with local app preferences and restored on startup. On Linux these live in `${XDG_DATA_HOME:-~/.local/share}/rode/app.ron`, under the `color_theme` key. The optional `--theme ID` argument overrides the stored choice for startup; that choice is saved on normal exit or when another theme is selected. Built-in IDs are `phosphor`, `carbon`, `carbon-edge`, `code-dark`, `matrix`, `amber`, `ice`, `neon`, `afterglow`, `knight-rider`, `hologram`, `telemetry`, `studio`, `studio-neutral`, and `studio-hacker`. A custom theme in `violet.json` has ID `custom:violet`, independent of its display name.

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
./target/release/rode --theme custom:violet
```

`name` is required and must have 1–64 printable characters. `base` is any built-in ID listed above (`phosphor` by default). All other sections are optional; omitted values inherit the base, including its syntax colors and frame treatment. Bases refer to the built-in palettes, not other custom files. Themes currently use dark UI styling.

The optional `chrome` field selects `"plain"` (standard frames), `"chamfered"` (cut corners, accent rails, subtle card gradients and static edge glow), `"holographic"` (double cut-corner outlines, illuminated corners and segmented rails), or `"bracketed"` (open corner brackets, vertical rails and hatched edge accents). It inherits the base: `"base": "afterglow"` includes the decorated frames; add `"chrome": "plain"` to keep only its colors. Decorations use native geometry with no animation, image assets or additional runtime dependencies. Editor and terminal canvases retain solid backgrounds.

Colors must be opaque `#RRGGBB` strings. Invalid colors, unknown keys, invalid bases, or malformed files are reported and skipped. Theme loading is bounded to 32 JSON files of at most 64 KiB each and a scan of at most 1,025 directory entries. Built-in palettes remain available if custom configuration is invalid.

## Component styling

The optional `ui` object defines shared dimensions and typography for controls, cards, dialogs and tabs. An empty `"ui": {}` opts into the shared rounded-component defaults, as Studio does; Studio Neutral and Studio Hacker inherit those settings from Studio. Fields are optional: omitted fields inherit the base theme's settings. Adding `ui` to a base without component settings starts from the shared defaults in [`InterfaceStyle`](../src/theme/interface.rs). These defaults apply to any color palette or frame treatment. Colors and `chrome` remain independent of these settings.

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

Dimensions use logical points and follow **Interface scale**. These settings do not change the saved editor/terminal font size or independent syntax selection. Switching to a theme without `ui` restores the original component styling.

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

The `syntax` object accepts `comment`, `keyword`, `type`, `function`, `string`, `number`, `constant`, `variable`, `operator`, `punctuation`, `builtin`, and `attribute`. These map to the existing parser's scopes; this is syntax coloring, not semantic analysis. Unmodified Phosphor retains the original syntax theme. A Phosphor theme with syntax overrides uses a matching token palette for the remaining categories.

Bold basic foreground colors (SGR 1 plus 30–37) use bright entries 8–15, matching [xterm's default boldColors setting](https://www.invisible-island.net/xterm/manpage/xterm.html). This does not brighten backgrounds, RGB colors or already-bright/extended indices. SGR 22/0 restores normal intensity.

Built-in palettes set `terminal_foreground` and ANSI entry 7 (White) to the UI text colour, with a lighter matching entry 15 (Bright white). Prompt/path slots follow the theme: warm gold in Carbon/Carbon Edge/Amber, green in Phosphor/Matrix/Studio Hacker, cool blue in Ice/Code Dark/Afterglow/Knight Rider/Hologram/Studio, silver-blue in Studio Neutral, violet in Neon and turquoise in Telemetry. ANSI colour names identify slots; a themed Blue slot need not be literal blue. Bright foreground entries meet at least 4.5:1 contrast against the default terminal background. Red and green remain distinct for error/success indicators. Custom definitions and saved swatch overrides may explicitly choose other values.

The optional `ansi` array overrides the terminal's first 16 indexed colors in standard order: black, red, green, yellow, blue, magenta, cyan, white, followed by their bright variants. All 16 entries are required if the array is supplied. Extended 256-color indices and explicit RGB colors use their standard values before contrast correction.

The optional `terminal.minimum_contrast` is a number from **1** to **21**, inherited from the base theme when omitted. Built-in themes default to **1** (off); use a value such as **4.5** to opt into contrast correction. Rendered text below this ratio is mixed toward white or black against its actual cell background, including inverted cells and RGB/256-color prompts. Backgrounds, spaces, Powerline separators and box/block glyphs are preserved. Unattainable ratios use whichever of black/white has the best contrast. **1** disables correction for exact terminal colors.

**View → Terminal colors…** also lets users override foreground/background, individual ANSI colors and minimum contrast without editing JSON. These desktop preferences are saved immediately per theme ID; they take precedence over theme files. **Reset to theme defaults** restores the file/base values. Switching palettes or adjusting terminal colors does not restart a shell or change its prompt/environment.

## Built-in definitions

[carbon.json](carbon.json), [carbon-edge.json](carbon-edge.json), [code-dark.json](code-dark.json), [matrix.json](matrix.json), [amber.json](amber.json), [ice.json](ice.json), [neon.json](neon.json), [afterglow.json](afterglow.json), [knight-rider.json](knight-rider.json), [hologram.json](hologram.json), [telemetry.json](telemetry.json), [studio.json](studio.json), [studio-neutral.json](studio-neutral.json), and [studio-hacker.json](studio-hacker.json) define the palettes embedded in the binary. Carbon Edge inherits Carbon’s color, syntax and terminal palette and adds its frame treatment. Studio opts into shared component defaults; Studio Neutral and Studio Hacker inherit them from Studio. Built-in bases resolve by ID independently of menu order; unknown bases and inheritance cycles are errors. Phosphor remains defined in Rust. Editing a source palette requires rebuilding. To customize without rebuilding, create a user theme with, for example, `"base": "matrix"` in the configuration directory.

Themes are local data. Loading or switching one never executes theme code, fetches resources, reconnects SSH, or restarts a terminal. Theme switches invalidate cached highlighting so already-open documents immediately use the new palette.

## Extending the built-in collection

Add a JSON definition and register its stable ID in [`src/theme/definition.rs`](../src/theme/definition.rs). Use `base` to share a palette or component style, and include only the differences. Catalog order controls the theme menu; it does not constrain inheritance. Custom files may inherit built-ins, including variants, but do not load other custom files.

All presets use the same component builders and egui installer. `colors`, `ui`, `chrome`, `syntax` and `ansi` are independent roles; views must use those roles rather than branch on a preset ID. Run `cargo test --locked -p rode --profile ci --lib --test themes` when adding a variant to check inheritance, parsing, theme switching, contrast and rendered layout.

For native checks, run `scripts/test-ui.py` with the UI harness dependencies described in [development](../docs/development.md). Run the 1×/2× DPI suite with `xvfb-run -a -s "-screen 0 3200x2200x24" python3 scripts/test-scale-ui.py`; its fixed menu targets assume the doubled window fits on the virtual display.

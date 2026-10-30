# Theme configuration

Select a theme from the **Theme: …** control at the bottom right, under **View → Color theme**, or use **Ctrl+Shift+P** and search for **Theme:**. Phosphor is the original green theme; Carbon is inspired by the supplied near-black reference screenshot. Code Dark adds neutral charcoal surfaces, light text and blue accents inspired by VS Code's classic dark theme. Matrix adds vivid green, Amber warm retro colors, Ice cold blue/cyan, and Neon magenta/cyan. These are native RODE palettes, rather than imports of third-party editor themes.

Independent code palettes are selected under **View → Syntax highlighting** or **Syntax theme: …** in the command palette. Dracula, Catppuccin Mocha, Nord, Monokai, and One Half Dark retain their full scope rules. See [independent syntax themes and custom rules](../docs/syntax-themes.md). The existing `syntax` category overrides below apply when **Follow UI theme** is selected.

UI theme changes are saved immediately with local app preferences and restored on startup. On Linux these live in `${XDG_DATA_HOME:-~/.local/share}/rode/app.ron`, under the `color_theme` key. The optional `--theme ID` argument overrides the stored choice for startup; that choice is saved on normal exit or when another theme is selected. Built-in IDs are `phosphor`, `carbon`, `code-dark`, `matrix`, `amber`, `ice`, and `neon`. A custom theme in `violet.json` has ID `custom:violet`, independent of its display name.

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

`name` is required and must have 1–64 printable characters. `base` is any built-in ID listed above (`phosphor` by default). All other sections are optional; omitted values inherit the base, including its syntax colors. Bases refer to the built-in palettes, not other custom files. Themes currently use dark UI styling.

Colors must be opaque `#RRGGBB` strings. Invalid colors, unknown keys, invalid bases, or malformed files are reported and skipped. Theme loading is bounded to 32 JSON files of at most 64 KiB each and a scan of at most 1,025 directory entries. Built-in palettes remain available if custom configuration is invalid.

## Palette keys

The `colors` object accepts:

| Key | Purpose |
| --- | --- |
| `background` | Editor and main window background |
| `panel` | Explorer, title area, and inactive tab surfaces |
| `raised` | Buttons, status bar, find bar, and notifications |
| `border` | Dividers and outlines |
| `text` | Main UI and default custom-syntax text |
| `muted` | Secondary UI text |
| `accent` | Highlights, active controls, cursor, and links |
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

Built-in palettes use pure white (`#ffffff`) for `terminal_foreground` and ANSI entries 7/15 (White/Bright white), without tinting these with the UI accent. Custom definitions and saved swatch overrides may explicitly choose other values.

The optional `ansi` array overrides the terminal's first 16 indexed colors in standard order: black, red, green, yellow, blue, magenta, cyan, white, followed by their bright variants. All 16 entries are required if the array is supplied. Extended 256-color indices and explicit RGB colors use their standard values before contrast correction.

The optional `terminal.minimum_contrast` is a number from **1** to **21**, inherited from the base theme when omitted. Built-in themes default to **1** (off); use a value such as **4.5** to opt into contrast correction. Rendered text below this ratio is mixed toward white or black against its actual cell background, including inverted cells and RGB/256-color prompts. Backgrounds, spaces, Powerline separators and box/block glyphs are preserved. Unattainable ratios use whichever of black/white has the best contrast. **1** disables correction for exact terminal colors.

**View → Terminal colors…** also lets users override foreground/background, individual ANSI colors and minimum contrast without editing JSON. These desktop preferences are saved immediately per theme ID; they take precedence over theme files. **Reset to theme defaults** restores the file/base values. Switching palettes or adjusting terminal colors does not restart a shell or change its prompt/environment.

## Built-in definitions

[carbon.json](carbon.json), [code-dark.json](code-dark.json), [matrix.json](matrix.json), [amber.json](amber.json), [ice.json](ice.json), and [neon.json](neon.json) contain the complete palettes embedded in the binary. Phosphor remains defined in Rust. Editing a source palette requires rebuilding. To customize without rebuilding, create a user theme with, for example, `"base": "matrix"` in the configuration directory.

Themes are local data. Loading or switching one never executes theme code, fetches resources, reconnects SSH, or restarts a terminal. Theme switches invalidate cached highlighting so already-open documents immediately use the new palette.

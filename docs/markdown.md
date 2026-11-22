# Markdown previews

Open a `.md`, `.markdown`, `.mdown`, or `.mkd` file, then choose **Preview** above the editor. **Edit** returns to the source in the same tab. Extensions are case-insensitive.

You can also use **Ctrl+Shift+R**, **View → Previews → Markdown preview**, or **Markdown: Toggle preview** in the command palette. On macOS, use Command instead of Control.

The preview renders the **current buffer**, including unsaved changes. Switching views preserves the text editor's cursor and undo history. Previewing does not save or modify the document; **Ctrl+S** saves the original Markdown source from either view. Task checkboxes are read-only in Preview. **Ctrl+F** returns to Edit for the existing find/replace controls.

Headings, paragraphs, bold/italic text, strikethrough, lists, task lists, quotes, tables, footnotes, inline code, and fenced code blocks render as native widgets. The preview follows the active theme and font size and adapts to the available editor area. Raw HTML is displayed as text, with no scripts or browser runtime. Navigation to heading fragments is not implemented.

## LaTeX equations

Use `$…$` for inline math and `$$…$$` for a centered display equation, including multiline expressions:

```markdown
The energy is $E = mc^2$.

$$
\frac{\alpha_i^2}{\sqrt{x}} = \sum_{k=1}^{n} k
$$
```

Equations render natively with an embedded math font; no TeX installation, external service or network access is required. Fractions, roots, scripts and matrices are supported. Wide equations shrink to fit the preview, and their color follows the active theme. Click an equation, or focus it with keyboard hints and press **Enter**, to copy its LaTeX source. **Copy LaTeX** is also available in the right-click menu. Code spans and fenced code blocks remain literal. Unsupported, incomplete or oversized expressions stay readable as source. This uses the same bounded worker and cache as [agent-chat maths](agents.md), with a 4096-byte limit per formula. This is a math renderer, not a full LaTeX document compiler.

## Links and images

Relative file links and images resolve from the Markdown file's directory **on the active host**. This works locally and over SSH. File links open in normal editor tabs; image reads follow the current sandbox grants and never retry outside an active sandbox. HTTP/HTTPS and email links open an external application only when clicked. Other URL schemes are not opened.

Supported raster images display inline using the [image preview formats and decoding limits](images.md); animated images show their first frame. External image URLs, embedded data URLs, and SVG images are not loaded automatically. Their alt text remains visible with an image notice. **Reload images** refreshes files after an external change. Changing projects, sandbox rules, or SSH sessions invalidates old image requests and cached textures.

To keep preview work bounded, a document may contain at most **256 KiB** of source, **12,000 parser events**, **64 nesting levels**, and **16 distinct inline raster images**. Images share the application's existing **128 MiB pixel budget**. Documents above the preview limits remain editable as ordinary text without a fixed byte-size ceiling. Closing a tab releases its image textures and cancels queued image reads.

For Markdown slide decks, see [Marp presentations](marp.md), with an installed project-host renderer, native slides and HTML/PDF/PowerPoint exports.

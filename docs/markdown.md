# Markdown previews

Open a `.md`, `.markdown`, `.mdown`, or `.mkd` file, then choose **Preview** above the editor. **Edit** returns to the source in the same tab. Extensions are case-insensitive.

You can also use **Ctrl+Shift+R**, **View → Markdown preview**, or **Markdown: Toggle preview** in the command palette. On macOS, use Command instead of Control.

The preview renders the **current buffer**, including unsaved changes. Switching views preserves the text editor's cursor and undo history. Previewing does not save or modify the document; **Ctrl+S** saves the original Markdown source from either view. Task checkboxes are read-only in Preview. **Ctrl+F** returns to Edit for the existing find/replace controls.

Headings, paragraphs, bold/italic text, strikethrough, lists, task lists, quotes, tables, footnotes, inline code, and fenced code blocks render as native widgets. The preview follows the active theme and font size and adapts to the available editor area. Raw HTML is displayed as text, with no scripts or browser runtime. Math typesetting and navigation to heading fragments are not implemented.

## Links and images

Relative file links and images resolve from the Markdown file's directory **on the active host**. This works locally and over SSH. File links open in normal editor tabs; image reads follow the current sandbox grants and never retry outside an active sandbox. HTTP/HTTPS and email links open an external application only when clicked. Other URL schemes are not opened.

Supported raster images display inline using the [image preview formats and decoding limits](images.md); animated images show their first frame. External image URLs, embedded data URLs, and SVG images are not loaded automatically. Their alt text remains visible with an image notice. **Reload images** refreshes files after an external change. Changing projects, sandbox rules, or SSH sessions invalidates old image requests and cached textures.

To keep preview work bounded, a document may contain at most **256 KiB** of source, **12,000 parser events**, **64 nesting levels**, and **16 distinct inline raster images**. Images share the application's existing **128 MiB pixel budget**. Documents above the preview limits remain editable under the normal 8 MiB text limit. Closing a tab releases its image textures and cancels queued image reads.

For Markdown slide decks, see [Marp presentations](marp.md), with an installed project-host renderer, native slides and HTML/PDF/PowerPoint exports.

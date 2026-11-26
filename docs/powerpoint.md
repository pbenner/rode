# PowerPoint previews

For Word documents, see [Word previews](word.md), which share the same reading controls.

Open a `.pptx` file from Explorer or **Ctrl+P** to view it in a read-only editor tab. Extensions are case-insensitive. **PowerPoint previews** is an independently switchable bundled feature in **View → Extensions**.

RODE includes the native Rust [BetterOffice](https://github.com/openooxml/betteroffice) renderer. Microsoft Office, LibreOffice, a browser, Node.js and NPM are not required. With an SSH workspace, the existing authenticated file channel transfers the original bytes and the desktop renders the slides; no presentation software is installed or executed on the remote host.

## Reading a presentation

The shared reader uses a compact toolbar with a full-width divider above the document. Expand **View settings** for single/continuous mode, fit options, actual size and reload; navigation, zoom and fullscreen remain visible.

- Use the previous/next arrows, the slide-number field and **Go**, or **Left / Right** and **Page Up / Page Down** with the slide canvas focused. **Home / End** reach the first/last slide.
- Choose **Single page** or **Continuous**, with the same fit, zoom, wheel scrolling and pan controls as the [PDF reader](pdf.md). Slide colors remain those of the presentation when the editor theme changes.
- Choose **Fullscreen**, press **F11** with the editor pane focused, or run **PPTX: Toggle fullscreen**. **Escape**, **F11**, or **Exit fullscreen** restores the previous window layout. Each editor window owns its own presentation state.
- **Reload**, or **PPTX: Reload preview**, re-reads the source through its original host and policy. Zoom, reading mode and the selected slide are retained. A failed reload keeps the last successful preview.

Slides use a persistent, private desktop renderer and the existing bounded page cache. Returning to a cached slide requires no re-render or network transfer. Nearby slides are prefetched; obsolete work is cancelled. Closing the tab or changing workspace cancels its requests. The original presentation model remains inside the worker; slides are not flattened through a PDF conversion.

## Select, copy and find text

Drag across slide text to select it, or double-click a word. Press **Ctrl+C** or
choose **Copy** from the right-click menu. **Ctrl+A**, or **Select slide text** in
that menu, selects the current slide's text. Use the middle mouse button to pan
a zoomed slide. Selection works in single-slide, continuous and fullscreen views.

Choose **Find** or press **Ctrl+F** to search the presentation. **Enter / Shift+Enter**,
**F3 / Shift+F3**, or the match arrows cycle through results; **Match case** enables
case-sensitive search. Matches are highlighted and revealed automatically.
**Escape** closes search before leaving fullscreen.

The text layer uses the same native font layout, shaping clusters and transforms
as the slide renderer. Visible slides are indexed first; Find indexes remaining
slides as needed. Indexing runs on the desktop, independently of raster rendering,
without another SSH transfer or an on-disk text index. Reloading successfully
replaces both the preview and its text index.

Text inside images requires OCR, which is not bundled. Copying follows the slide's
text-box drawing order, which may differ from the intended reading order in complex
layouts. Unsupported content cannot supply selectable text.

## Fidelity and editing

This is a **read-only viewer**. Save, Save As, formatting and run commands do not write or execute PPTX content. Other source buffers retain their text and undo history when switching between slides and code. Text/shape editing and saving presentations are not implemented.

The renderer looks up the presentation's declared fonts on the **desktop running RODE**, including fonts inherited from themes, slide masters and layouts. Installed TrueType/OpenType fonts and font collections are supported, with separate regular, bold and italic faces when available. For example, a presentation requesting Verdana uses installed Verdana rather than Inter. Fonts are not needed on the SSH project host.

Missing or unreadable families use RODE's bundled Inter fallback. A toolbar warning lists unavailable declared fonts; substitution can change line wrapping, spacing and layout. Install your licensed fonts in the desktop's normal font directories (on Linux, for example, `~/.local/share/fonts`), then **Reload** the preview. RODE does not install or download Office fonts, and does not extract fonts embedded in PPTX packages. An exported PDF with embedded fonts remains useful for checking the original application's typography; native PowerPoint layout can still differ even when the fonts match.

A toolbar warning also reports embedded images the renderer could not display. Some advanced PowerPoint content may be unsupported; animations, video playback, interactive controls and password-protected presentations are not supported by this viewer. The original file is preserved.

## Resource and ownership boundaries

PDF and PPTX share the reader's **2048-pixel long-edge** raster limit, **128 MiB preview pixel budget**, bounded per-document caches, one admitted render job per reader, and a private renderer with a **1 GiB address-space limit** and **20-second deadline per operation**. Up to **10,000 slides** are accepted. BetterOffice also bounds XML parsing. On Linux, the worker reads a sealed anonymous memory snapshot. The viewing protocol exposes no source-saving or external-resource-loading operation.

The worker is a resource and lifetime boundary, not an additional filesystem sandbox. It reads installed desktop font files; it does not fetch presentation-linked fonts or other external resources. Font registration is bounded to 48 declared families, 32 MiB per font file and 96 MiB of registered font bytes. Project file reads continue to use the selected local/SSH host and its captured access policy. Rendering starts only when a presentation is opened or navigated; no external conversion service is used.

Selection and search use a separate private text worker with the same process
limits as the raster worker. Each slide allows **1 MiB of text / 100,000 cells**
within an **8 MiB** framed response. The reader retains at most **64 MiB** of text
index and **10,000 search matches**; errors or reached limits appear as partial results.

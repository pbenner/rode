# PowerPoint previews

Open a `.pptx` file from Explorer or **Ctrl+P** to view it in a read-only editor tab. Extensions are case-insensitive. **PowerPoint previews** is an independently switchable bundled feature in **View → Extensions**.

RODE includes the native Rust [BetterOffice](https://github.com/openooxml/betteroffice) renderer. Microsoft Office, LibreOffice, a browser, Node.js and NPM are not required. With an SSH workspace, the existing authenticated file channel transfers the original bytes and the desktop renders the slides; no presentation software is installed or executed on the remote host.

## Reading a presentation

- Use the previous/next arrows, the slide-number field and **Go**, or **Left / Right** and **Page Up / Page Down** with the slide canvas focused. **Home / End** reach the first/last slide.
- Choose **Single page** or **Continuous**, with the same fit, zoom, wheel scrolling and pan controls as the [PDF reader](pdf.md). Slide colors remain those of the presentation when the editor theme changes.
- Choose **Fullscreen**, press **F11** with the editor pane focused, or run **PPTX: Toggle fullscreen**. **Escape**, **F11**, or **Exit fullscreen** restores the previous window layout. Each editor window owns its own presentation state.
- **Reload**, or **PPTX: Reload preview**, re-reads the source through its original host and policy. Zoom, reading mode and the selected slide are retained. A failed reload keeps the last successful preview.

Slides use a persistent, private desktop renderer and the existing bounded page cache. Returning to a cached slide requires no re-render or network transfer. Nearby slides are prefetched; obsolete work is cancelled. Closing the tab or changing workspace cancels its requests. The original presentation model remains inside the worker; slides are not flattened through a PDF conversion.

## Fidelity and editing

This first version is a **viewer**. Save, Save As, formatting and run commands do not write or execute PPTX content. Other source buffers retain their text and undo history when switching between slides and code. Text/shape editing and saving edited presentations are not exposed yet.

The renderer uses RODE's bundled Inter font as an offline fallback. It does not install or download Office fonts. Font substitution can change line wrapping, spacing and layout. A toolbar warning reports embedded images the renderer could not display. Some advanced PowerPoint content may be unsupported; animations, video playback, interactive controls and password-protected presentations are not supported by this viewer. The original file is preserved.

## Resource and ownership boundaries

PDF and PPTX share the reader's **2048-pixel long-edge** raster limit, **128 MiB preview pixel budget**, bounded per-document caches, one admitted render job per reader, and a private renderer with a **1 GiB address-space limit** and **20-second deadline per operation**. Up to **10,000 slides** are accepted. BetterOffice also bounds XML parsing. On Linux, the worker reads a sealed anonymous memory snapshot. The viewing protocol exposes no source-saving or external-resource-loading operation.

The worker is a resource and lifetime boundary, not an additional filesystem sandbox. Project file reads continue to use the selected local/SSH host and its captured access policy. Rendering starts only when a presentation is opened or navigated; no background indexing or external conversion service is used.

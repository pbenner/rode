# PDF reader

Open a `.pdf` file from the explorer or **Ctrl+P**. It opens in a read-only tab alongside code, images, and Markdown. File extensions are case-insensitive.

PowerPoint `.pptx` files use the same reading controls with a separate native renderer; see [PowerPoint previews](powerpoint.md).

Word `.docx` files also use these controls, with native pagination and an in-memory PDF snapshot; see [Word previews](word.md).

RODE bundles the pure-Rust [Hayro](https://github.com/LaurenzV/hayro) renderer and standard PDF substitute fonts. No Poppler tools, PDFium shared library, browser, or separate PDF installation is required. Local and SSH projects use the same desktop renderer: only original file bytes travel through the existing authenticated, sandbox-aware file channel. Opening and changing pages do not run PDF software on the remote host.

## Navigation and display

The compact toolbar keeps page navigation, zoom and fullscreen visible above a full-width divider. Open **View settings** for reading mode, fit choices, actual size and reload. Close it to give the document more space. This presentation choice is local to the reader and is retained while changing pages, reloading or entering fullscreen. Pages scroll below the fixed toolbar.

- Open **View settings** and choose **Single page** or **Continuous**. Continuous mode scrolls through a vertically arranged document, including mixed portrait and landscape pages; only visible pages and nearby pages are rendered.
- Choose **Fullscreen**, press **F11** with the PDF editor pane focused, or run **PDF: Toggle fullscreen** from the command palette. The reader fills its native window and hides the surrounding editor panels, while retaining page, zoom, and reading-mode controls. **Escape**, **F11**, or **Exit fullscreen** restores the previous layout. Each editor window has its own fullscreen state; switching away from the PDF or closing/moving its tab also exits fullscreen. No additional application or workspace connection is started.
- Use the previous/next buttons, or enter a page number and press **Go** / Enter. Navigation remains available during rendering; newer destinations replace pending navigation.
- With the canvas focused, **Page Up / Page Down** switch pages in single-page mode. In continuous mode they scroll by most of a viewport; **Space** also scrolls forward, and **Up / Down** scroll in smaller steps. **Home / End** reach the document's beginning/end.
- **Fit page**, **Fit width** and **100%** are in **View settings**; **+ / −** and **Ctrl+Plus / Minus** remain directly accessible. Continuous mode preserves the reading position when zoom or window size changes.
- Scroll with the mouse wheel/trackpad or scrollbars. In single-page mode, continuing past the bottom/top turns to the next/previous page; returning through the top reveals the previous page's bottom. Dragging selects embedded text.
- **View settings → Reload**, or **PDF: Reload preview** in the command palette, re-reads the source. It preserves zoom, reading mode and selected page, clamping the page if the document has fewer pages. A failed reload retains the last successful document.

The reader retains the parsed document, font caches and a bounded cache of page pixels. Going back to a cached page reuses those pixels without another render or SSH transfer. Nearby pages are prefetched, and obsolete work is cancelled. There is no filesystem watcher. Closing the tab or replacing its document cancels outstanding work.

Save, Save As, formatting, fixes, and run commands do not write or execute PDF content. Switching tabs and themes preserves unsaved code; PDF page colors remain those of the document. Annotations, interactive forms, and password entry are not implemented. PDFs requiring a password show an error. Some complex PDFs may contain features Hayro does not yet support.

## Select, copy and find text

Drag across text to select it, double-click a word, then press **Ctrl+C** or choose
**Copy** from the right-click menu. **Ctrl+A** selects the current page's text;
the context menu also offers **Select page text**. Selection follows zoom, page
rotation and crop geometry, in single-page, continuous and fullscreen reading.
Use the middle mouse button to pan a zoomed page without changing the selection.

Choose **Find** or press **Ctrl+F** to search the document. Matching text is
highlighted; the active match is revealed automatically. Use **Enter / Shift+Enter**,
**F3 / Shift+F3**, or the previous/next buttons to cycle through matches, wrapping
at the ends. **Match case** enables case-sensitive searching. **Escape** closes
search before leaving fullscreen. Spaces and line breaks are treated alike in
phrases. Results arrive progressively while remaining pages are indexed.

Text is extracted on the desktop from the same immutable PDF bytes, without a
remote executable or another SSH transfer. Visible pages are indexed first;
opening Find indexes remaining pages while the search is open, independently
of raster rendering. No text index is written to disk. Reloading successfully
starts a fresh index; a failed reload retains the previous document and text.

Scanned pages without embedded text require OCR, which is not bundled. Font
Unicode mappings and PDF drawing order determine copied text; complex columns,
unusual fonts and ligatures can affect reading order or extraction accuracy.

## Resource limits and worker lifetime

- PDF sources have no fixed byte-size ceiling; available RAM/address space remains the practical limit. Documents support up to **10,000 pages**.
- Page rasters use up to **2048 pixels on the longer side**, or the GPU's smaller limit. **100%** means one rendered pixel per logical screen point; further zoom enlarges that raster.
- Image/PDF raster pixels share the **128 MiB preview budget** per RODE process. Each PDF's decoded-page cache retains at most four pages and 48 MiB, and cached entries can be evicted under shared-budget pressure. Displayed pages retain their reservations until no longer needed. Continuous views retain at most three page textures, including the active one.
- Original source bytes are accounted separately from the pixel budget. Transfer, interpreter, raster and texture buffers add memory beyond that budget.
- Each open PDF has one private raster renderer process started from the installed RODE executable. It parses the immutable snapshot once and returns bounded raw pixels, avoiding per-page process startup and PNG encoding/decoding. The worker has a **1 GiB address-space limit** and a **20-second deadline per operation**. Interpreter/font caches rotate after 16 renders. A cancelled or failed worker is discarded and can restart for a later request.
- Page rendering runs off the UI thread, with at most one admitted page job per reader. The renderer exits when its source closes or the owning editor connection disappears. On Linux, sealed anonymous memory snapshots avoid plaintext temporary files. Closing tabs releases their source, caches and textures.
- Text uses a separate private interpreter with the same process limits. Each page allows up to **1 MiB of text / 100,000 cells**, with an **8 MiB** framed response. The in-memory index retains at most **64 MiB** per reader and search returns up to **10,000 matches**; a reached limit or extraction error is identified as partial results. Text memory is separate from the raster budget.

Malformed documents and renderer failures report an error while preserving editor buffers and the last successful view. The worker provides resource and lifetime isolation; it is not a separate filesystem sandbox. Source access continues to use the captured project host and policy.

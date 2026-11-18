# PDF reader

Open a `.pdf` file from the explorer or **Ctrl+P**. It opens in a read-only tab alongside code, images, and Markdown. File extensions are case-insensitive.

RODE bundles the pure-Rust [Hayro](https://github.com/LaurenzV/hayro) renderer and standard PDF substitute fonts. No Poppler tools, PDFium shared library, browser, or separate PDF installation is required. Local and SSH projects use the same desktop renderer: only original file bytes travel through the existing authenticated, sandbox-aware file channel. Opening and changing pages do not run PDF software on the remote host.

## Navigation and display

- Choose **Single page** or **Continuous** in the PDF toolbar. Continuous mode scrolls through a vertically arranged document, including mixed portrait and landscape pages; only visible pages and nearby pages are rendered.
- Use the previous/next arrows, or enter a page number and press **Go** / Enter. Navigation remains available during rendering; newer destinations replace pending navigation.
- With the canvas focused, **Page Up / Page Down** switch pages in single-page mode. In continuous mode they scroll by most of a viewport; **Space** also scrolls forward, and **Up / Down** scroll in smaller steps. **Home / End** reach the document's beginning/end.
- **Fit page**, **Fit width**, **100%**, **+ / −**, and **Ctrl+Plus / Minus** control zoom. Continuous mode preserves the reading position when zoom or window size changes.
- Scroll with the mouse wheel/trackpad or scrollbars. In single-page mode, continuing past the bottom/top turns to the next/previous page; returning through the top reveals the previous page's bottom. Dragging pans a single page.
- **Reload**, or **PDF: Reload preview** in the command palette, re-reads the source. It preserves zoom, reading mode and selected page, clamping the page if the document has fewer pages. A failed reload retains the last successful document.

The reader retains the parsed document, font caches and a bounded cache of page pixels. Going back to a cached page reuses those pixels without another render or SSH transfer. Nearby pages are prefetched, and obsolete work is cancelled. There is no background indexing or filesystem watcher. Closing the tab or changing project context cancels outstanding work.

Save, Save As, formatting, fixes, and run commands do not write or execute PDF content. Switching tabs and themes preserves unsaved code; PDF page colors remain those of the document. Text selection/search, annotations, interactive forms, and password entry are not implemented. PDFs requiring a password show an error. Some complex PDFs may contain features Hayro does not yet support.

## Resource limits and worker lifetime

- PDF sources have no fixed byte-size ceiling; available RAM/address space remains the practical limit. Documents support up to **10,000 pages**.
- Page rasters use up to **2048 pixels on the longer side**, or the GPU's smaller limit. **100%** means one rendered pixel per logical screen point; further zoom enlarges that raster.
- Image/PDF raster pixels share the **128 MiB preview budget** per RODE process. Each PDF's decoded-page cache retains at most four pages and 48 MiB, and cached entries can be evicted under shared-budget pressure. Displayed pages retain their reservations until no longer needed. Continuous views retain at most three page textures, including the active one.
- Original source bytes are accounted separately from the pixel budget. Transfer, interpreter, raster and texture buffers add memory beyond that budget.
- Each open PDF has one private renderer process started from the installed RODE executable. It parses the immutable snapshot once and returns bounded raw pixels, avoiding per-page process startup and PNG encoding/decoding. The worker has a **1 GiB address-space limit** and a **20-second deadline per operation**. Interpreter/font caches rotate after 16 renders. A cancelled or failed worker is discarded and can restart for a later request.
- Page rendering runs off the UI thread, with at most one admitted page job per reader. The renderer exits when its source closes or the owning editor connection disappears. On Linux, sealed anonymous memory snapshots avoid plaintext temporary files. Closing tabs releases their source, caches and textures.

Malformed documents and renderer failures report an error while preserving editor buffers and the last successful view. The worker provides resource and lifetime isolation; it is not a separate filesystem sandbox. Source access continues to use the captured project host and policy.

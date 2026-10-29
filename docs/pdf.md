# PDF previews

Open a `.pdf` file from the explorer or **Ctrl+P**. It opens in a read-only tab alongside code, images, and Markdown. File extensions are case-insensitive.

PDF rendering uses **Poppler's `pdfinfo` and `pdftoppm` on the desktop running RODE**. Install your system's Poppler tools package if they are missing (commonly `poppler-utils`, for example `sudo apt install poppler-utils` on Debian/Ubuntu). RODE reports missing tools and does not install them automatically. SSH hosts need no PDF software: only the original bytes travel through the existing authenticated, sandbox-aware file channel. Rendering uses desktop fonts.

- Use the previous/next arrows, or enter a **Page** number and press **Go** / Enter.
- With the page canvas focused, **Page Up / Page Down** switch pages, and **Home / End** select the first/last page.
- **Fit**, **100%**, **+ / −**, and **Ctrl+Plus / Minus** control zoom. Use the mouse wheel or trackpad to scroll within a zoomed page; continue past its bottom/top to move to the next/previous page. This also works in Fit mode. Returning to a previous page starts at its bottom. Drag or use the scrollbars to pan within a page.
- **Reload**, or **PDF: Reload preview** in the command palette, re-reads the source. It preserves zoom and the selected page, clamping the page if the document has fewer pages. A failed reload retains the last successful page.

Only the displayed page is rendered and retained. Page changes use the loaded snapshot without another SSH transfer. There is no background indexing or filesystem watcher. Closing the tab or changing project context cancels an outstanding page render. Source files are always read through the current host's file channel and active sandbox; rendering on the desktop receives bytes on stdin, never the source path.

Save, Save As, formatting, fixes, and run commands do not write or execute PDF content. Switching tabs and themes preserves unsaved code; PDF page colors remain those of the document. These are raster previews: text selection/search, annotations, form interaction, and entering PDF passwords are not implemented. PDFs that require a password show a visible error.

## Resource limits

- PDF files: up to **32 MiB** and **10,000 pages**.
- Page raster: up to **2048 pixels on the longer side**, or the GPU's smaller limit. **100%** means one rendered pixel per logical screen point; zooming further enlarges that raster.
- Retained PDF source bytes and all image/PDF preview pixels share the **128 MiB preview budget** per RODE process. Close another preview if the budget is full. Closing tabs releases their source data and textures.
- Transfer, renderer output and decoding buffers add temporary memory. Each renderer process has a **1 GiB address-space limit**, bounded stdout/stderr, and a **20-second timeout**. Work runs off the UI thread; page rendering shares the existing serialized preview worker path.

Malformed/oversized documents and renderer failures produce an error while keeping existing editor buffers intact. Poppler runs only when opening/reloading a PDF or changing its displayed page. No browser, remote PDF service, network image loader, or always-running renderer is required.

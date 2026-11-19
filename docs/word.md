# Word previews

Open a `.docx` file from Explorer or **Ctrl+P** to read it in an editor tab. Extensions are case-insensitive. **Word previews** is an independently switchable bundled feature in **View → Extensions**. Older binary `.doc` files are not supported by this viewer.

RODE bundles the native Rust [rdocx](https://github.com/tensorbee/rdocx) layout engine and substitute fonts. It produces a PDF snapshot in memory, which the existing [Hayro reader](pdf.md) paints at the requested resolution. Microsoft Word, LibreOffice, a browser, Node.js, NPM and system fonts are not required. No PDF is written beside the source file.

Local and SSH projects use the same desktop renderer. The existing authenticated, policy-bound file channel transfers original DOCX bytes; no Word software or conversion command runs on the remote host.

## Reading and navigation

The shared reader uses a compact toolbar with a full-width divider above the document. Expand **View settings** for single/continuous mode, fit options, actual size and reload; navigation, zoom and fullscreen remain visible.

- Use previous/next, the page-number field and **Go**, or **Page Up / Page Down** with the page canvas focused. **Home / End** reach the first/last page.
- Choose **Single page** or **Continuous**, **Fit page**, **Fit width**, or a zoom level. Page rasters share the PDF reader's cache; returning to a cached page needs no file transfer or conversion.
- **Fullscreen**, **F11** with the editor pane focused, or **Word: Toggle fullscreen** hides the surrounding interface in that editor window. **Escape**, **F11**, or **Exit fullscreen** restores it.
- **Reload**, or **Word: Reload preview**, reads a new snapshot through the original host and access policy. The reader retains its zoom, display mode and selected page where possible. Failed reloads preserve the previous preview.

## Fidelity and source protection

This is a **read-only visual viewer**. Save, Save As, code formatting and run/build actions cannot modify or execute an open Word preview. Editing and saving Word documents are not exposed. Other open source buffers keep their unsaved text and undo history when switching between previews and code.

Native pagination renders supported paragraphs, formatting, tables, lists, images, page breaks, sections, headers, footers and notes. Embedded fonts and bundled substitutes provide an offline font path. Font substitution, complex wrapping and unsupported Office features can change appearance or page breaks; this is not a promise of exact Microsoft Word output. **Preview limitations** exposes bounded diagnostics reported by the layout engine. The original DOCX bytes are preserved regardless of rendering approximations.

Password-protected documents and legacy `.doc` files are outside this viewer's scope. Linked external assets are not downloaded; only package content is supplied to the renderer. No macros or external Office applications are executed.

## Resource boundaries

Word uses the same private, cancellable renderer, original-source leases, **2048-pixel long-edge** rasters, **128 MiB** shared pixel budget and bounded page caches as PDF and PowerPoint. Opening a document includes pagination and PDF generation within the worker's **1 GiB address-space limit** and **20-second operation deadline**. Up to **10,000 pages** are accepted. Closing a tab or changing workspace cancels its pending work. The worker is a resource and lifetime boundary, not an additional filesystem sandbox.

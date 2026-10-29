# Image previews

Open an image from the explorer or with **Ctrl+P**. It appears in a normal tab alongside your code. This works for local and SSH projects, with the same sandbox access rules as text files. Remote hosts need no image libraries; decoding happens on the desktop in a background worker.

Supported formats: **PNG, JPEG/JFIF, WebP, GIF, BMP, ICO, TIFF, PNM/PBM/PGM/PPM/PAM, and TGA**. Extensions are case-insensitive. Animated images show a static first frame; TIFF shows the first page. SVG remains editable XML text.

- **Fit** scales the image to the available space without enlarging small images.
- **100%** shows one image pixel per logical screen point. **+ / −** change the zoom; **Ctrl+Plus / Minus** also zoom while the image canvas has focus.
- Scroll or drag to pan a zoomed image. Transparent areas show a checkerboard. Photo orientation is applied when present.
- **Reload**, or **Image: Reload preview** in the command palette, reads the file again after an external change. A failed reload keeps the last successful preview. There is no filesystem watcher.

Previews are read-only: Save, Save As, formatting, and Python tools do not write image files. Switching between code and image tabs preserves unsaved text.

To bound resource use, each image file may contain at most **32 MiB**, with at most **64 MiB of decoded pixels** and **16,384 pixels per side**. At RGBA8 this permits up to 16 megapixels; higher bit depths may reach the limit earlier. Invalid or oversized images produce a visible error.

The displayed texture is reduced to at most **4096 pixels per side**, or the GPU's smaller limit. Metadata always shows the original oriented dimensions and marks a **reduced preview** when applicable. At 100% these reduced previews occupy the original logical size, so fine detail is limited by the preview resolution. Open image/PDF previews share a **128 MiB budget** per RODE process for retained pixels and PDF source bytes; close another image tab if the budget is full. Transfer and decoding buffers are temporary and add memory while loading. Textures are released when their tabs close; previews do not animate or poll in the background.

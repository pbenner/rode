# Hex editor and binary analysis

Open a binary file normally, or use **File → Open as hex / inspect binary**, the explorer's right-click menu, or **Hex: Open file / inspect binary** in the command palette. An explicit hex open also works for text, images and PDFs. Save or close a file's unsaved text edits before changing its view to hex.

The **Hex & binary analysis** bundled extension is enabled by default. Binary data stays separate from text buffers, formatters, Markdown and code-run commands.

## Editing bytes

The hex view shows offsets, hex bytes and ASCII side by side. Click a byte to select it. **Offset (hex) → Go** jumps to an absolute file offset. **Hex search** accepts byte pairs such as `48 89 E5`; turn it off to search UTF-8 text. **Find next** wraps around. The inspector shows signed/unsigned byte and 32-bit values, with selectable endianness.

Turn on **Edit bytes** before making changes. Either enter two hex digits directly on the selected byte, or enter up to 4096 bytes in the patch field and click **Overwrite**. Arrow keys, Page Up/Down and Home/End navigate the grid. Undo/Redo buttons and Ctrl+Z/Ctrl+Y restore byte patches when the grid has focus. Modified bytes use the theme accent.

Patches overwrite existing bytes and preserve the file's length. Ctrl+S saves; Ctrl+Shift+S saves a separate copy. Local and SSH saves use the same revision conflict checks, cooperating-writer locks, atomic replacement, executable permission preservation and project sandbox as source saves. An external modification prevents overwriting it and leaves your edits open. Changes made during a save remain dirty. Closing a modified binary uses the ordinary unsaved-change guard. Reload is available only for clean buffers.

## Static analysis

The analysis uses the downloaded snapshot on the desktop; no analysis tools need installing on the remote computer, and the inspected program is never executed.

- **Overview:** detected ELF, PE/COFF or Mach-O object format, architecture, bitness, endianness, entry point, SHA-256 and Shannon entropy.
- **Sections:** virtual addresses, sizes, kinds and names. Click a mapped section to jump to its file bytes.
- **Symbols:** ordinary/dynamic symbols, sizes, addresses and import markers; mapped symbols link to hex offsets.
- **Strings:** printable ASCII sequences of at least four bytes, with offsets.
- **Imports / Exports:** entries exposed by the object parser. ELF dependencies/undefined symbols may be represented in Symbols rather than the Imports table.
- **Disassembly:** native x86 16/32/64-bit decoding in Intel syntax. Choose a mode and **Analyze / decode here** to decode from the selected file offset; **Entry point** selects the mapped entry address. Mapped section addresses are used as instruction addresses. Decoding another architecture as x86 is explicitly marked as a raw interpretation.

Analysis runs on a worker from the current unsaved bytes. After a patch, a badge marks the old report as needing refresh. Changes arriving while analysis runs invalidate its result. Clicking a mapped table/instruction row returns to the corresponding hex byte. Tables support filtering.

## Limits

Hex files are limited to **8 MiB**. Snapshots, edit/save/analysis copies, bounded patch history and metadata reserve the shared document-preview budget; closing a tab releases its reservation. Tables retain at most 4096 rows (sections: 1024), displayed string names/content are capped at 256 characters, and disassembly retains at most 512 instructions from an 8192-byte window. Only visible grid/table rows are drawn; filtering is cached.

This is a static inspection and byte-patching tool. It does not include debugging, process memory access, a decompiler, control-flow graph generation, cross-reference analysis, archive/fat-binary member selection, UTF-16 string extraction or source reconstruction. Compressed debug data is not expanded. Patches do not repair object headers, relocations, checksums or signatures. Other architectures retain hex editing and object metadata; native instruction decoding currently covers x86 only.

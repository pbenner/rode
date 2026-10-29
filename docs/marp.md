# Marp presentations

Open Markdown and choose **Slides** beside **Edit / Preview**, or **View → Marp slide preview**. **Ctrl+Shift+R** toggles slides for `marp: true` front matter or `.marp.md` filenames. Ordinary Markdown keeps its existing preview shortcut.

**File → New Marp presentation** creates an unsaved starter deck. Save As defaults to `.marp.md`:

```markdown
---
marp: true
theme: default
paginate: true
---

# My presentation

---

## Second slide

- Write ordinary Markdown
- Separate slides with `---`
```

Marp renders the **current buffer**, including unsaved changes. RODE displays its PDF output as native slides: themes, layout directives, pagination, syntax highlighting and math use Marp's actual renderer. The editor has no embedded browser or preview server. Navigate with slide buttons, **PageUp / PageDown**, **Home / End**, or scroll past a slide's edge; fit, zoom and pan work as for PDFs.

**Edit** and **Ctrl+F** return to source editing with cursor/undo history retained. **Ctrl+S** saves Markdown from either view; preview/export never silently save it. Completed previews are cached when switching to Edit. Hidden or cancelled rendering stops; project changes and disabling the feature cancel work and return to source. Stale results cannot replace a newer buffer's preview. A failed refresh retains the previous successful render with an error.

## Installed tools

Conversion runs on the **project host**, within its actual sandbox and grants. Install Marp CLI yourself there, for example:

```sh
npm install --save-dev @marp-team/marp-cli
```

RODE checks the project's `node_modules/.bin/marp`, then `marp` on PATH. It never uses npx, downloads tools/browsers, uploads an executable, or falls back from SSH to desktop execution. **HTML export** needs the CLI. **Slide preview, PDF and PowerPoint export** also need an installed browser supported by Marp. Slide display needs desktop Poppler (`pdfinfo` and `pdftoppm`, normally `poppler-utils`), as for [PDF previews](pdf.md).

Use **Marp settings** in Slides or **Project settings → Marp presentations** to select the CLI executable, browser type/path and optional custom theme CSS. **Save Marp settings** persists them per host/project. Paths refer to that host; relative settings paths resolve from the project root. Open a project to save custom settings; standalone files can use default PATH tools.

The deck's directory is the base for slide assets, including unsaved snapshots and read-only projects. No temporary Markdown file is written into the project. Enable **Allow local slide assets** when a trusted deck needs local images/fonts; access still follows the sandbox. Explicit browser rendering can fetch external resources only within that policy's network access. Ordinary native Markdown preview continues to block automatic external image downloads.

RODE supplies a temporary data-only configuration, disabling HTML tags, watch, preview-window and server modes. It does not automatically execute repository `marp.config.js` / `.marprc` files. Front matter, built-in themes, inline style directives and the selected CSS remain available. Custom JavaScript engines/configuration, native browser animations and a presenter-notes panel are outside this integration. HTML exports retain Marp's browser presentation controls.

## Export

Choose **Export → HTML / PDF / PowerPoint** in Slides, or a **Marp: Export …** palette command. The dialog selects a path on the project host, with the chosen format's extension. Relative output paths resolve from the project root, or the source file's directory without a project.

**Replace existing export** is off by default. Enabling it checks the existing revision before conversion and uses locked atomic replacement. A concurrent change prevents overwriting it. Exports cannot replace their Markdown source, including through symlinks. Export records its submitted snapshot; edits made during conversion remain unsaved. Already-open explorer folders refresh after export.

HTML embeds theme styling and presentation controls; local image references still need their assets beside the export or at the referenced paths. PDF/PPTX embed rendered content. PowerPoint uses Marp's normal slide-image export, rather than editable shapes.

## Extension and limits

**Marp presentations** is a bundled extension, enabled by default; manage it with the Extensions icon or **Ctrl+Shift+X**. Disabling it cancels work and preserves source edits. Ordinary Markdown/PDF preview settings are independent.

Limits are **256 KiB of source**, **8 MiB per artifact**, a **90-second CLI timeout**, and **512 preview slides**. One conversion runs at a time per RODE process. Only the visible slide retains a page texture, under the shared preview budget. Conversion and decoding stay off the UI thread. Cancellation gives Marp a brief interval to close its owned browser before forcing its process group to stop. Existing terminals and other RODE windows are independent.

See the official [Marp CLI guide](https://github.com/marp-team/marp-cli#readme), [directives](https://marpit.marp.app/directives) and [image syntax](https://marpit.marp.app/image-syntax). Real HTML/PDF/PPTX checks used Marp CLI 4.5.1 and Firefox; an installed browser compatible with the host's sandbox remains a prerequisite.

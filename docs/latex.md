# LaTeX

RODE highlights LaTeX (`.tex`, `.ltx`, `.sty`, `.cls`, `.dtx`, `.ins`) and BibTeX (`.bib`) with embedded grammars. Editing and highlighting need no TeX installation.

## Build and preview

Open a project and source file, save your changes, then use **Build PDF** or **Build & preview** above the editor. The same actions appear in the command palette. **Code → Build current file / project** builds; **Run current file / project** builds and opens the PDF.

The compiler runs on the local or SSH project host, inside its actual sandbox. The native [PDF viewer](pdf.md) displays results using desktop Poppler tools. **Open PDF** reopens or reloads the last successful result. Failed builds preserve an already-open preview. Builds use saved files and never replace source buffers.

**Output**, or **LaTeX: Show build output**, shows compiler output and clickable source errors. Errors for unchanged open sources also appear in Problems and the editor. New edits invalidate those highlights. **Stop build** cancels owned compiler processes. Changing projects/settings, disabling LaTeX tools or closing RODE also cancels a build. RODE coordinates builds sharing an output directory across windows and SSH.

## Generic commands

Under **Project settings → LaTeX**, choose a preset or enter any build executable and arguments. The default uses an installed `latexmk` with pdfLaTeX. Presets also cover latexmk with XeLaTeX or LuaLaTeX, and direct calls to those engines. Direct presets run once; latexmk manages additional passes and bibliography tools.

Each argument occupies one line and is passed literally, without shell splitting or quote removal. Placeholders expand once:

| Placeholder | Value |
| --- | --- |
| `{file}` | Absolute main-source filename |
| `{dir}` | Main-source directory; also the build working directory |
| `{root}` | Project directory |
| `{outdir}` | Absolute output directory |

A custom wrapper accepting source and destination directory could use:

```text
{file}
{outdir}
```

Executable paths containing `/` resolve relative to the project; other names use the host's PATH. **PDF filename** overrides the default main-source basename plus `.pdf`. It resolves relative to the output directory, or may be an absolute host path.

**Main file** resolves relative to the project and overrides the current source. Otherwise RODE follows `% !TeX root = ../main.tex` comments within the first 32 lines, relative to the containing file. Cycles and more than eight redirects fail visibly. Configure a main file for bibliography or macro files that cannot build independently.

By default, artifacts are written beside the main source. An optional output directory resolves relative to the project, for example `build/latex`. Your arguments must use `{outdir}` to honor that setting. Settings are saved per host/project through the existing concurrent preference merging.

Supplied presets disable shell escape. Latexmk presets use `-norc` to avoid automatic Perl configuration loading. Edit arguments explicitly for workflows needing different behavior; the outer sandbox remains authoritative. RODE installs no engines, TeX packages or bibliography tools. The selected tools must already work on the project host and may manage their own caches.

## Formatting and issues

Optional installed `latexindent` and `chktex` tools support **Format document**, **Check document**, format/check on save, and debounced live issues. They receive the current buffer through stdin. Formatting remains version-checked and uses ordinary protected saves. Enable both LaTeX tools and Language tools in **View → Extensions** for the bundled buffer tools.

The formatter accepts latexindent-compatible arguments, reads local formatting settings and suppresses `indent.log`. The checker accepts ChkTeX-compatible arguments and checks only the current buffer without following includes. Executable paths are configurable; blank fields disable the respective tool. Missing tools leave editing/saving available. BibTeX highlighting is included; standalone bibliography formatting/linting remains available through adapters or the terminal.

## Access and limits

TeX distributions, fonts, format files, bibliography data and output paths must be visible under the [sandbox policy](sandbox.md). Distributions storing system formats in `/var/lib/texmf` may need an explicit read-only grant there. Personal TeX trees/caches need grants too. RODE never widens access or retries a sandbox build outside it. Read-only projects need an explicitly writable output location that is also accessible to subsequent PDF reads.

Builds default to 120 seconds, configurable up to 300. Source/PDF preview files are limited to 8 MiB, stdout/stderr to 8 MiB each, generated regular files to 64 MiB, and diagnostics to 2,000. The output window caches a bounded log tail; **Copy full output** copies the full captured log. PDF page/pixel limits still apply. Continuous builds, SyncTeX navigation, completion and language servers are outside this integration.

Tool references: [latexmk](https://ctan.org/pkg/latexmk), [latexindent usage](https://latexindentpl.readthedocs.io/en/stable/sec-how-to-use.html), [ChkTeX](https://www.nongnu.org/chktex/).

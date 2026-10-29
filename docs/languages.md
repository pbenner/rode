# Programming languages

RODE includes native syntax highlighting for Go, Python, Rust, C/C++, JavaScript/JSX, TypeScript/TSX, Java, C#, shell, Ruby, PHP, Lua, Julia, Fortran, R, Swift, Kotlin, Dart, Scala, Perl, Haskell, Elixir, and SQL. Web/configuration formats include HTML, CSS/SCSS/Sass, JSON, TOML, YAML, XML, Markdown, LaTeX/BibTeX, Dockerfiles, Makefiles, CMake, Git ignore files, and dotenv files.

Detection recognizes common extension variants (`.pyi`, `.pyw`, `.mjs`, `.cjs`, `.mts`, `.cts`, C++ headers, uppercase extensions) and special filenames such as `Dockerfile`, `Makefile`, and `CMakeLists.txt`. Grammars are embedded: highlighting works offline without installing tools. Files with other recognized grammar extensions can also be highlighted; unknown formats remain editable as plain text.

## Formatting and issues

Use **Code → Format document** (**Shift+Alt+F**), **Check document**, and **View → Problems** (**Ctrl+Shift+M**). Format on save, check on save, and checks after a typing pause are configurable in Project settings.

| Language | Formatter | Issues in editor |
| --- | --- | --- |
| Python, type stubs | `uvx ruff` by default | Ruff lint and syntax diagnostics; safe fixes |
| Go | `gofmt` | Syntax errors |
| Rust | `rustfmt` | Formatting only |
| JavaScript/JSX, TypeScript/TSX | `prettier` | Parser syntax errors |
| HTML, CSS/SCSS, JSON, YAML, Markdown | `prettier` | Parser syntax errors |
| C, C++, Java, C# | `clang-format` | Formatting only |
| Bash / POSIX shell | `shfmt` | Syntax errors |
| LaTeX | Optional `latexindent` | Optional ChkTeX lint; compiler issues after builds |
| Other languages | Project terminal | Project terminal |

Install the listed tools on the computer where the project runs. Project-local `node_modules/.bin/prettier` takes precedence over PATH; RODE never runs `npx` to fetch it. Ruff's explicitly configured `uvx ruff` default is the exception: uv may download Ruff into its cache. See [Python environments and Ruff configuration](project-tools.md#formatting-issues-and-python-environments).

Formatters read the current buffer through stdin and return text; they do not write source files. Project configuration such as `.prettierrc`, `.clang-format`, `.editorconfig`, and Ruff settings is respected by the corresponding tool. Parser checks do not report type errors, compiler errors, or missing imports outside Ruff's checks. Ignored files follow the tool's ignore rules. There is no LSP, completion, or go-to-definition in this release.

Live checks debounce for 900 ms, cover only the active buffer up to 512 KiB, and stop retrying an unavailable tool until a manual retry or settings change. Manual/save tools use the 8 MiB text limit. Outdated results are discarded. Missing tools or syntax errors preserve the submitted text and allow saving. A missing tool for one language does not disable other languages' checks.

## Run and build

Open a project folder, save the relevant files, then choose **Code → Run current file / project** or **Build current file / project**. Both are also available in the command palette. Output and interactive input use the integrated terminal, on the active local/SSH host inside the selected sandbox. Commands start from the project root; use an idle shell or open a fresh terminal first.

| Language | Run | Build |
| --- | --- | --- |
| Python | Selected project interpreter + current file | — |
| Go | `go run <file>` | `go build ./...` |
| Rust | `cargo run` | `cargo build` |
| JavaScript | `node <file>` | `npm run build` |
| TypeScript | Installed `tsx <file>` (project-local first) | `npm run build` |
| JSX/TSX | Project terminal / package scripts | `npm run build` |
| Java | `java <file>` (source-file mode) | `javac <file>` |
| C# | `dotnet run` | `dotnet build` |
| Shell, Ruby, PHP, Lua, Julia, R, Swift, Dart, Perl, Elixir | Installed runtime + current file | — |

Run commands do not save files or install runtimes. Build/run tools may produce artifacts or download project dependencies according to their own configuration. Cargo/.NET/npm commands require the corresponding project files at the project root. Java source mode needs a compatible JDK. For multi-file Go packages use `go run .` in the terminal. Use the terminal for C/C++ build systems, package test scripts, custom arguments, and other workflows. Use **Ctrl+C** in the terminal to stop a running program.

In a sandbox, hidden toolchains/caches remain hidden. Use [sandbox access settings](sandbox.md) to grant any additional installed tools or persistent caches you need; Go can also create temporary caches in the sandbox's otherwise empty home. No sandbox grants are widened automatically.

## Embedded grammars

Additional grammars come from the [two-face / bat collection](https://docs.rs/two-face/0.5.2/two_face/), using the existing syntect renderer and RODE theme palettes. `rode --syntax-licenses` prints their embedded acknowledgements, also documented in the [upstream license listing](https://codeberg.org/CosmicHarper/two-face/src/tag/v0.5.2+bat-0.26.1/generated/acknowledgements_full.md).

Formatter references: [gofmt](https://pkg.go.dev/cmd/gofmt), [Prettier stdin](https://prettier.io/docs/cli#--stdin-filepath), [clang-format](https://clang.llvm.org/docs/ClangFormat.html), [shfmt](https://github.com/mvdan/sh/blob/master/cmd/shfmt/shfmt.1.scd).

## LaTeX projects

[LaTeX support](latex.md) adds generic configurable TeX builds, main-file/root-comment selection, compiler diagnostics and PDF preview locally or over SSH. Use the LaTeX toolbar or palette; settings support other engines/build wrappers beyond the supplied latexmk and direct-engine presets.

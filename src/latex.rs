//! Installed LaTeX tools on the project host. Sources stay under ordinary safe
//! saves; explicit builds produce artifacts and never rewrite editor buffers.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{BufRead, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const LIMIT: usize = 8 * 1024 * 1024;
/// ChkTeX output template shared with the Python peer's diagnostic parser.
pub const CHECK_FORMAT: &str = "%l:%c:%d:%k:%n:%m\n";
/// Recognize supported TeX source extensions without reading a file.
pub fn is_source(path: Option<&str>) -> bool {
    crate::languages::detect(path)
        .is_some_and(|language| matches!(language.id.as_str(), "latex" | "bibtex"))
}

/// Persisted host/project configuration for installed TeX compiler/format/check tools.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Installed compiler or build-driver executable.
    pub program: String,
    /// Literal compiler argument template; supports the documented source/output placeholders.
    pub arguments: Vec<String>,
    /// Optional explicit main source, relative to the project when not absolute.
    pub main_file: String,
    /// Optional build-output directory, resolved on the project host.
    pub output_directory: String,
    /// Optional explicit output PDF path.
    pub pdf_file: String,
    /// Installed stdin formatter executable, normally latexindent.
    pub formatter: String,
    /// Installed checker executable, normally ChkTeX.
    pub checker: String,
    /// Maximum compiler runtime in seconds.
    pub timeout_seconds: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            program: "latexmk".into(),
            arguments: preset("latexmk", "pdflatex"),
            main_file: String::new(),
            output_directory: String::new(),
            pdf_file: String::new(),
            formatter: "latexindent".into(),
            checker: "chktex".into(),
            timeout_seconds: 120,
        }
    }
}
/// Construct literal arguments for a supported driver/engine preset; does not run or install
/// it.
pub fn preset(driver: &str, engine: &str) -> Vec<String> {
    let mut args: Vec<String> = if driver == "latexmk" {
        vec![
            "-norc".into(),
            match engine {
                "xelatex" => "-xelatex",
                "lualatex" => "-lualatex",
                _ => "-pdf",
            }
            .into(),
        ]
    } else {
        vec![]
    };
    args.extend(
        [
            "-interaction=nonstopmode",
            "-halt-on-error",
            "-file-line-error",
            "-no-shell-escape",
        ]
        .map(str::to_owned),
    );
    args.push(
        if driver == "latexmk" {
            "-outdir={outdir}"
        } else {
            "-output-directory={outdir}"
        }
        .into(),
    );
    args.push("{file}".into());
    args
}
impl Settings {
    /// Validate settings, bounds and output argument placeholders before execution.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.program.trim().is_empty(),
            "Choose a LaTeX build executable."
        );
        ensure!(
            (1..=300).contains(&self.timeout_seconds),
            "LaTeX timeout must be 1–300 seconds."
        );
        ensure!(
            self.arguments.len() <= 128
                && self.arguments.iter().map(String::len).sum::<usize>() <= 32 * 1024,
            "LaTeX arguments exceed their limit."
        );
        for value in [
            &self.program,
            &self.main_file,
            &self.output_directory,
            &self.pdf_file,
            &self.formatter,
            &self.checker,
        ]
        .into_iter()
        .chain(self.arguments.iter())
        {
            ensure!(
                value.len() <= 4096 && !value.contains('\0'),
                "Invalid LaTeX executable, path or argument."
            );
        }
        Ok(())
    }
}
/// Saved-source build context; the UI separately guards unsaved source edits.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invocation {
    /// Project working directory on the execution host.
    pub root: String,
    /// Selected source file, used to resolve main-file directives.
    pub path: String,
    /// Compiler/output settings captured for this build.
    pub settings: Settings,
}
/// Compiler diagnostic associated with a project-host source path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Issue {
    /// Source path on the project host.
    pub path: String,
    /// Normalized source location, severity and message.
    #[serde(flatten)]
    pub diagnostic: crate::project::Diagnostic,
}
/// Completed build output; a failed compiler can still return diagnostics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    /// Resolved main source path.
    pub source: String,
    /// Output PDF path when a successful build produced it; file bytes are loaded separately.
    pub pdf: Option<String>,
    /// Compiler exit code and bounded stdout/stderr.
    pub output: crate::project::Output,
    /// Bounded clickable source issues.
    pub diagnostics: Vec<Issue>,
    /// Additional nonfatal build/preview explanation.
    pub note: String,
}
fn resolve(root: &Path, value: &str) -> PathBuf {
    let path = crate::files::expand_home(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}
fn program(root: &Path, value: &str) -> String {
    if value.contains('/') || value.starts_with('~') {
        resolve(root, value).to_string_lossy().into()
    } else {
        value.into()
    }
}
/// Resolve the explicit main file or bounded TeX root-comment chain, rejecting cycles and
/// invalid source paths.
pub fn main_file(root: &Path, path: &str, settings: &Settings) -> Result<PathBuf> {
    if !settings.main_file.trim().is_empty() {
        return resolve(root, &settings.main_file)
            .canonicalize()
            .context("Cannot open the configured LaTeX main file");
    }
    let mut source = resolve(root, path);
    let mut visited = HashSet::new();
    for _ in 0..8 {
        source = source
            .canonicalize()
            .context("Cannot open the LaTeX source/main file")?;
        ensure!(
            visited.insert(source.clone()),
            "Cycle in LaTeX root comments."
        );
        let mut header = vec![];
        File::open(&source)?.take(8192).read_to_end(&mut header)?;
        let text = String::from_utf8_lossy(&header);
        let target = text.lines().take(32).find_map(|line| {
            let (key, value) = line.trim().strip_prefix('%')?.trim().split_once('=')?;
            key.trim()
                .eq_ignore_ascii_case("!tex root")
                .then(|| value.trim().trim_matches('"').trim_matches('\'').to_owned())
                .filter(|v| !v.is_empty())
        });
        match target {
            Some(target) => {
                source = resolve(
                    source.parent().context("Missing source directory")?,
                    &target,
                )
            }
            None => return Ok(source),
        }
    }
    bail!("More than eight LaTeX root redirects.")
}
/// Resolve the expected PDF path from explicit settings and the main source name.
pub fn pdf_path(root: &Path, source: &Path, settings: &Settings) -> PathBuf {
    let output = if settings.output_directory.trim().is_empty() {
        source.parent().unwrap_or(root).to_owned()
    } else {
        resolve(root, &settings.output_directory)
    };
    if settings.pdf_file.trim().is_empty() {
        output.join(format!(
            "{}.pdf",
            source.file_stem().unwrap_or_default().to_string_lossy()
        ))
    } else {
        resolve(&output, &settings.pdf_file)
    }
}
fn argument(value: &str, substitutions: &[(&str, &str)]) -> String {
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('}') else {
            break;
        };
        let key = &rest[1..end];
        if let Some((_, value)) = substitutions.iter().find(|(name, _)| *name == key) {
            output.push_str(value);
        } else {
            output.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    output
}
/// Run a cancellable saved-source build with a shared output-directory lock. Blocks until
/// completion and never writes editor buffers or installs TeX tools.
pub fn run(invocation: &Invocation, cancel: &AtomicBool) -> Result<Reply> {
    invocation.settings.validate()?;
    ensure!(!cancel.load(Ordering::Acquire), "LaTeX build cancelled.");
    let root = crate::files::expand_home(&invocation.root).canonicalize()?;
    let settings = &invocation.settings;
    let source = main_file(&root, &invocation.path, settings)?;
    ensure!(
        source.metadata()?.is_file() && source.metadata()?.len() <= LIMIT as u64,
        "LaTeX sources must be regular files up to 8 MiB."
    );
    let parent = source.parent().context("Missing LaTeX source directory")?;
    let output = if settings.output_directory.trim().is_empty() {
        parent.to_owned()
    } else {
        resolve(&root, &settings.output_directory)
    };
    fs::create_dir_all(&output)
        .context("Cannot create the LaTeX output directory under the active project policy")?;
    let output = output.canonicalize()?;
    // Directory inode locks coordinate builds across windows, SSH and bwrap
    // without creating permanent lock files in a project or private /tmp.
    let lock = File::open(&output)?;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Another RODE build is using this LaTeX output directory. Try again when it finishes."
    );
    let (source_text, parent_text, output_text, root_text) = (
        source.to_string_lossy(),
        parent.to_string_lossy(),
        output.to_string_lossy(),
        root.to_string_lossy(),
    );
    let substitutions = [
        ("file", source_text.as_ref()),
        ("dir", parent_text.as_ref()),
        ("outdir", output_text.as_ref()),
        ("root", root_text.as_ref()),
    ];
    let args: Vec<_> = settings
        .arguments
        .iter()
        .map(|arg| argument(arg, &substitutions))
        .collect();
    let mut command = Command::new(program(&root, &settings.program));
    command
        .args(args)
        .current_dir(parent)
        .env("NO_COLOR", "1")
        .env("LC_ALL", "C.UTF-8");
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 64 * 1024 * 1024,
                rlim_max: 64 * 1024 * 1024,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let result = crate::project::run_controlled(
        command,
        vec![],
        Duration::from_secs(settings.timeout_seconds),
        cancel,
        false,
    )
    .with_context(|| {
        format!(
            "LaTeX build tool '{}' failed. Install/configure it on the project host.",
            settings.program
        )
    })?;
    let diagnostics = build_diagnostics(
        &format!("{}\n{}", result.stdout, result.stderr),
        parent,
        &source,
    );
    let mut reply = Reply {
        source: source.to_string_lossy().into(),
        output: result,
        diagnostics,
        ..Default::default()
    };
    if reply.output.success() {
        let path = pdf_path(&root, &source, settings);
        let valid = (|| -> Result<()> {
            let mut file = File::open(&path)?;
            ensure!(
                file.metadata()?.is_file() && file.metadata()?.len() <= LIMIT as u64,
                "Generated PDF exceeds the 8 MiB preview limit."
            );
            let mut header = [0; 5];
            file.read_exact(&mut header)?;
            ensure!(&header == b"%PDF-", "Build output is not a PDF.");
            Ok(())
        })();
        match valid {
            Ok(()) => reply.pdf = Some(path.canonicalize()?.to_string_lossy().into()),
            Err(error) => reply.note = format!("No usable PDF at {}: {error:#}", path.display()),
        }
    }
    Ok(reply)
}
/// Parse common compiler file/line errors into bounded project-host source issues.
pub fn build_diagnostics(output: &str, directory: &Path, main: &Path) -> Vec<Issue> {
    let mut issues = vec![];
    let mut seen = HashSet::new();
    let mut pending = None;
    for line in output.lines() {
        if issues.len() >= 2000 {
            break;
        }
        let line = line.trim();
        let parts: Vec<_> = line.split(':').collect();
        let located = (1..parts.len().saturating_sub(1)).find_map(|i| {
            let row = parts[i].parse::<usize>().ok()?.max(1);
            let filename = parts[..i].join(":");
            let extension = Path::new(&filename)
                .extension()?
                .to_str()?
                .to_ascii_lowercase();
            if !["tex", "ltx", "sty", "cls", "bib", "dtx", "ins"].contains(&extension.as_str()) {
                return None;
            }
            let (column, message) =
                if let Some(column) = parts.get(i + 1).and_then(|v| v.parse::<usize>().ok()) {
                    (column.max(1), parts[i + 2..].join(":"))
                } else {
                    (1, parts[i + 1..].join(":"))
                };
            Some((
                resolve(directory, &filename),
                row,
                column,
                message.trim().to_owned(),
            ))
        });
        let located = located.or_else(|| {
            if let Some(message) = line.strip_prefix("! ") {
                pending = Some(message.to_owned());
            }
            let location = line
                .strip_prefix("l.")?
                .split_whitespace()
                .next()?
                .parse::<usize>()
                .ok()?;
            Some((main.to_owned(), location.max(1), 1, pending.take()?))
        });
        if let Some((path, row, column, mut message)) = located {
            let mut end = 4096.min(message.len());
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            let path = path
                .canonicalize()
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            if !seen.insert((path.clone(), row, message.clone())) {
                continue;
            }
            issues.push(Issue {
                path,
                diagnostic: crate::project::Diagnostic {
                    severity: if message.to_ascii_lowercase().contains("warning") {
                        "warning"
                    } else {
                        "error"
                    }
                    .into(),
                    code: "LaTeX".into(),
                    message,
                    row,
                    column,
                    end_row: row,
                    end_column: column.saturating_add(1),
                },
            });
        }
    }
    issues
}
/// Run installed stdin-only TeX formatting/checking on a buffer snapshot; source files remain
/// untouched.
pub fn analyze(
    root: &Path,
    path: &str,
    text: &str,
    settings: &Settings,
    format: bool,
    check: bool,
) -> Result<crate::project::Analysis> {
    settings.validate()?;
    ensure!(text.len() <= LIMIT, "Buffer exceeds the tool limit.");
    let source = resolve(root, path);
    let directory = source.parent().filter(|p| p.is_dir()).unwrap_or(root);
    let mut analysis = crate::project::Analysis {
        text: text.into(),
        ..Default::default()
    };
    if format && !settings.formatter.trim().is_empty() {
        let mut command = Command::new(program(root, &settings.formatter));
        command
            .current_dir(directory)
            .args(["-g=/dev/null", "-l", "-"]);
        analysis.text = crate::project::run(command, text.as_bytes().to_vec(), Duration::from_secs(20))
            .context("latexindent is unavailable. Install it on the project host or configure the LaTeX formatter path.")?.checked()?;
    }
    if check && !settings.checker.trim().is_empty() {
        let mut command = Command::new(program(root, &settings.checker));
        command
            .current_dir(directory)
            .args(["-q", "-I0", "-f", CHECK_FORMAT])
            .env("LC_ALL", "C.UTF-8");
        let output = crate::project::run(command, analysis.text.as_bytes().to_vec(), Duration::from_secs(20))
            .context("chktex is unavailable. Install it on the project host or configure the LaTeX checker path.");
        match output {
            Err(error) if format => analysis.warnings.push(format!("{error:#}")),
            Err(error) => return Err(error),
            Ok(output) => {
                analysis.diagnostics = checker_diagnostics(&output.stdout, &analysis.text);
                ensure!(
                    output.code != 1
                        && ((0..=3).contains(&output.code))
                        && (output.code == 0 || !analysis.diagnostics.is_empty()),
                    "ChkTeX failed: {}{}",
                    output.stderr,
                    output.stdout
                );
            }
        }
    }
    Ok(analysis)
}
/// Convert ChkTeX's configured output format into bounded one-based source diagnostics.
pub fn checker_diagnostics(output: &str, source: &str) -> Vec<crate::project::Diagnostic> {
    let mut result: Vec<_> = output
        .lines()
        .filter_map(|line| {
            let parts: Vec<_> = line.splitn(6, ':').collect();
            if parts.len() != 6 {
                return None;
            }
            let row = parts[0].parse::<usize>().ok()?.max(1);
            let column = parts[1].parse::<usize>().ok()?.max(1);
            Some(crate::project::Diagnostic {
                code: format!("chktex {}", parts[4]),
                message: parts[5].trim().into(),
                row,
                column,
                end_row: row,
                end_column: column.saturating_add(parts[2].parse::<usize>().unwrap_or(1).max(1)),
                severity: if parts[3].eq_ignore_ascii_case("error") {
                    "error"
                } else {
                    "warning"
                }
                .into(),
            })
        })
        .take(2000)
        .collect();
    crate::languages::normalize_columns(&mut result, source, "chktex");
    result
}
/// Drive a LaTeX helper process, propagate cancellation and decode its bounded reply.
pub fn execute(command: Command, invocation: &Invocation, cancel: &AtomicBool) -> Result<Reply> {
    invocation.settings.validate()?;
    let mut input = serde_json::to_vec(invocation)?;
    input.push(b'\n');
    let result = crate::project::run_controlled_limited(
        command,
        input,
        Duration::from_secs(invocation.settings.timeout_seconds + 10),
        cancel,
        true,
        20 * 1024 * 1024,
        Duration::from_secs(1),
    )?
    .checked()?;
    let reply: Reply =
        serde_json::from_str(&result).context("Invalid LaTeX coordinator response")?;
    ensure!(
        reply.output.stdout.len() <= LIMIT
            && reply.output.stderr.len() <= LIMIT
            && reply.diagnostics.len() <= 2000
            && reply.note.len() <= 8192,
        "LaTeX response exceeds limits."
    );
    Ok(reply)
}
/// Serve the internal `--latex-helper` stdin protocol; launch inside the prepared project
/// boundary.
pub fn coordinator() -> Result<()> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .take(256 * 1024)
        .read_line(&mut line)?;
    let invocation: Invocation = serde_json::from_str(&line)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let control = cancel.clone();
    std::thread::spawn(move || {
        let _ = std::io::stdin().lock().read(&mut [0]);
        control.store(true, Ordering::Release);
    });
    serde_json::to_writer(std::io::stdout().lock(), &run(&invocation, &cancel)?)?;
    Ok(())
}

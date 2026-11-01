//! Explicit Marp CLI snapshots. Rendering runs on the project host, under the
//! prepared policy; only a bounded PDF or export acknowledgement comes back.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufRead, Read},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Maximum submitted Markdown snapshot size in bytes.
pub const MAX_SOURCE: usize = 256 * 1024;
/// Maximum rendered/exported result size in bytes.
pub const MAX_OUTPUT: usize = 8 * 1024 * 1024;
/// Source template for a new unsaved presentation.
pub const TEMPLATE: &str = "---\nmarp: true\ntheme: default\npaginate: true\n---\n\n# My presentation\n\nWrite your slides in Markdown.\n\n---\n\n## Next slide\n\n- Edit the source\n- Choose Slides to preview\n- Export to HTML, PDF or PowerPoint\n";

/// Installed project-host Marp/browser configuration; no tool is installed automatically.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Browser implementation selected for the Marp CLI.
    #[serde(default = "auto_browser")]
    pub browser_kind: String,
    /// Installed Marp CLI executable path/name.
    pub program: String,
    /// Optional installed browser executable override.
    pub browser: String,
    /// Optional project-host custom theme CSS path.
    pub theme_css: String,
    /// Permit the renderer to read local assets within the outer sandbox's grants.
    pub allow_local_files: bool,
}
fn auto_browser() -> String {
    "auto".into()
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            browser_kind: auto_browser(),
            program: String::new(),
            browser: String::new(),
            theme_css: String::new(),
            allow_local_files: false,
        }
    }
}
impl Settings {
    /// Validate command/path bounds and supported browser choices.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            ["auto", "chrome", "edge", "firefox"].contains(&self.browser_kind.as_str()),
            "Choose an available Marp browser type."
        );
        for value in [&self.program, &self.browser, &self.theme_css] {
            ensure!(
                value.len() <= 4096 && !value.contains('\0'),
                "Invalid Marp executable or asset path."
            );
        }
        Ok(())
    }
}
/// Supported explicit Marp output formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Standalone HTML export.
    Html,
    /// PDF export or slide-preview snapshot.
    Pdf,
    /// PowerPoint export.
    Pptx,
}
impl Format {
    /// Return the conventional file extension without a leading dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Pdf => "pdf",
            Self::Pptx => "pptx",
        }
    }
}
/// Explicit export destination and overwrite authorization.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Destination {
    /// Output path on the project host.
    pub path: String,
    /// Whether replacing an existing destination was explicitly confirmed.
    pub overwrite: bool,
}
/// One source-snapshot render/export request captured from the active workspace.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Invocation {
    /// Project working directory on the execution host.
    pub root: String,
    /// Source path for resolving relative assets; unsaved decks receive a synthetic path.
    pub path: String,
    /// Current Markdown buffer snapshot, including unsaved changes.
    pub text: String,
    /// Installed tool/browser configuration.
    pub settings: Settings,
    /// Requested render/export format.
    pub format: Format,
    /// Export target; absence requests an in-memory preview snapshot.
    pub destination: Option<Destination>,
}
/// Rendered preview bytes or an explicitly written export path.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Reply {
    /// Standard base64-encoded preview output; absent for disk exports.
    pub data: Option<String>,
    /// Successfully exported host path; absent for in-memory previews.
    pub path: Option<String>,
    /// Bounded nonfatal renderer diagnostics.
    pub warnings: String,
}

/// Recognize Marp front matter without executing Markdown or loading assets.
pub fn is_deck(path: Option<&str>, text: &str) -> bool {
    if path.is_some_and(|p| p.to_ascii_lowercase().ends_with(".marp.md")) {
        return true;
    }
    let mut lines = text.trim_start_matches('\u{feff}').lines();
    if lines.next() != Some("---") {
        return false;
    }
    lines
        .take(128)
        .take_while(|line| !matches!(line.trim(), "---" | "..."))
        .any(|line| {
            line.split_once(':').is_some_and(|(key, value)| {
                key.trim() == "marp"
                    && value
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case("true")
            })
        })
}
fn resolve(root: &Path, value: &str) -> PathBuf {
    let path = crate::files::expand_home(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}
/// Encode a host path as a file URL for the renderer without interpreting shell syntax.
pub fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
            url.push(*byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}
struct Scratch(PathBuf);
impl Scratch {
    fn new(root: &Path) -> Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let path = crate::vault::scratch_parent(root)?
            .join(format!(".rode-marp-{}", rode_session::new_id()?));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Run a cancellable Marp conversion on the project host. Keeps scratch under the permitted
/// temporary/encrypted context and publishes exports atomically with overwrite guards.
pub fn run(invocation: &Invocation, cancel: &AtomicBool) -> Result<Reply> {
    use std::os::unix::process::CommandExt;
    ensure!(
        invocation.text.len() <= MAX_SOURCE,
        "Marp supports source up to 256 KiB."
    );
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Marp conversion cancelled."
    );
    invocation.settings.validate()?;
    let root = crate::files::expand_home(&invocation.root).canonicalize()?;
    let source = resolve(&root, &invocation.path);
    let parent = source
        .parent()
        .context("Marp source has no parent directory")?
        .canonicalize()?;
    let source = parent.join(source.file_name().context("Missing Marp source filename")?);
    let destination = invocation
        .destination
        .as_ref()
        .map(|destination| -> Result<_> {
            let path = resolve(&root, &destination.path);
            ensure!(
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(invocation.format.extension())),
                "Export filename must end with .{}.",
                invocation.format.extension()
            );
            let path = if path.exists() {
                path.canonicalize()?
            } else {
                path.parent()
                    .context("Export has no parent directory")?
                    .canonicalize()?
                    .join(path.file_name().context("Missing export filename")?)
            };
            ensure!(
                source.canonicalize().unwrap_or(source.clone()) != path,
                "Export cannot replace the Markdown source."
            );
            let expected = if path.exists() {
                ensure!(
                    destination.overwrite,
                    "Export already exists. Enable Replace existing export to replace this file."
                );
                Some(
                    crate::files::local(&crate::files::Request::ReadBinary {
                        path: path.to_string_lossy().into(),
                    })?
                    .revision,
                )
            } else {
                None
            };
            Ok((path, expected))
        })
        .transpose()?;
    let settings = &invocation.settings;
    let program = if settings.program.trim().is_empty() {
        let local = root.join("node_modules/.bin/marp");
        if local.is_file() {
            local.to_string_lossy().into_owned()
        } else {
            "marp".into()
        }
    } else if settings.program.contains('/') || settings.program.starts_with('~') {
        resolve(&root, &settings.program)
            .to_string_lossy()
            .into_owned()
    } else {
        settings.program.clone()
    };
    let scratch = Scratch::new(&root)?;
    let output_path = scratch
        .0
        .join(format!("slides.{}", invocation.format.extension()));
    let config_path = scratch.0.join("marp.json");
    let mut config = serde_json::json!({"html":false});
    // Marp's supported config baseUrl resolves stdin assets without writing a
    // temporary Markdown file into the project (including read-only projects).
    if invocation.format != Format::Html {
        config["baseUrl"] = file_url(&source).into();
    }
    fs::write(&config_path, serde_json::to_vec(&config)?)?;
    let mut command = Command::new(&program);
    command
        .current_dir(&parent)
        .env("TMPDIR", &scratch.0)
        .env("TEMP", &scratch.0)
        .env("TMP", &scratch.0)
        .args(["-", "--output"])
        .arg(&output_path)
        .arg("--config-file")
        .arg(&config_path)
        .args([
            "--no-html",
            "--no-watch",
            "--no-server",
            "--no-preview",
            "--no-parallel",
            "--browser-timeout",
            "30",
        ]);
    match invocation.format {
        Format::Html => {}
        Format::Pdf => {
            command.arg("--pdf");
        }
        Format::Pptx => {
            command.arg("--pptx");
        }
    }
    let browser_kind = if settings.browser_kind == "auto"
        && settings.browser.to_ascii_lowercase().contains("firefox")
    {
        "firefox"
    } else {
        &settings.browser_kind
    };
    command.arg("--browser").arg(browser_kind);
    if settings.allow_local_files {
        command.arg("--allow-local-files");
    }
    if !settings.browser.trim().is_empty() {
        command
            .arg("--browser-path")
            .arg(resolve(&root, &settings.browser));
    }
    if !settings.theme_css.trim().is_empty() {
        command
            .arg("--theme-set")
            .arg(resolve(&root, &settings.theme_css));
    }
    command.env("NO_COLOR", "1");
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                // Browser profiles may write caches larger than the retained
                // artifact. The artifact is independently bounded below.
                rlim_cur: 64 * 1024 * 1024,
                rlim_max: 64 * 1024 * 1024,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = crate::project::run_controlled_limited(command, invocation.text.as_bytes().to_vec(), Duration::from_secs(90), cancel, false, MAX_OUTPUT as u64, Duration::from_secs(2))
        .with_context(|| format!("Marp needs an installed Marp CLI on the project host (project node_modules/.bin/marp or configured executable). PDF/PPTX also need a supported browser. Cannot convert with {program}"))?;
    ensure!(
        output.success(),
        "Marp conversion failed: {}{}",
        output.stderr,
        output.stdout
    );
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Marp conversion cancelled."
    );
    let mut bytes = Vec::new();
    fs::File::open(&output_path)
        .context("Marp did not produce the requested output")?
        .take(MAX_OUTPUT as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_OUTPUT,
        "Marp output must be nonempty and at most 8 MiB."
    );
    if invocation.format == Format::Pdf {
        ensure!(bytes.starts_with(b"%PDF-"), "Marp did not produce a PDF.");
    }
    let mut warnings = output.stderr;
    let mut end = warnings.len().min(64 * 1024);
    while !warnings.is_char_boundary(end) {
        end -= 1;
    }
    warnings.truncate(end);
    if let Some((path, expected)) = destination {
        crate::files::local(&crate::files::Request::WriteBinary {
            path: path.to_string_lossy().into(),
            data: STANDARD.encode(bytes),
            expected,
        })?;
        Ok(Reply {
            path: Some(path.to_string_lossy().into()),
            warnings,
            ..Default::default()
        })
    } else {
        Ok(Reply {
            data: Some(STANDARD.encode(bytes)),
            warnings,
            ..Default::default()
        })
    }
}
/// Drive a Marp helper process and cancellation channel, decoding one bounded reply.
pub fn execute(command: Command, invocation: &Invocation, cancel: &AtomicBool) -> Result<Reply> {
    execute_with_prelude(command, invocation, cancel, vec![])
}

pub(crate) fn execute_with_prelude(
    command: Command,
    invocation: &Invocation,
    cancel: &AtomicBool,
    mut input: Vec<u8>,
) -> Result<Reply> {
    input.extend(serde_json::to_vec(invocation)?);
    input.push(b'\n');
    let output = crate::project::run_controlled_limited(
        command,
        input,
        Duration::from_secs(100),
        cancel,
        true,
        16 * 1024 * 1024,
        Duration::from_secs(3),
    )?
    .checked()?;
    let reply: Reply =
        serde_json::from_str(&output).context("Invalid Marp coordinator response")?;
    ensure!(
        reply
            .data
            .as_ref()
            .is_none_or(|data| data.len() <= MAX_OUTPUT.div_ceil(3) * 4)
            && reply.warnings.len() <= 64 * 1024,
        "Marp response exceeds limits."
    );
    Ok(reply)
}
/// Serve the internal `--marp-helper` stdin protocol inside the prepared project boundary.
pub fn coordinator() -> Result<()> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .take(2 * 1024 * 1024)
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

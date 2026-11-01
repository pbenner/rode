//! Native feature registry and an explicitly enabled, versioned tool adapter API.
//! Manifests are desktop data; executable adapters always run on the project host.
//!
//! # Design decisions
//!
//! Bundled features remain ordinary Rust modules to keep their native UI and
//! shared lifecycle guards. Disabling controls activation and background work;
//! it does not unload code or reduce the compiled binary. Installed integrations
//! use a narrow versioned executable protocol instead of a dynamic Rust ABI or
//! a general scripting runtime. The tradeoff is process startup overhead and
//! limited extension surfaces, with a single bounded execution boundary.
//!
//! Manifests come from explicit desktop installation and start disabled so
//! opening a repository cannot implicitly activate its own executable metadata.
//! The command still runs on the project host inside its actual sandbox; an
//! adapter reply can supply a version-checked buffer result, not permission to
//! bypass source-save guards or widen host access.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Supported installed-adapter wire protocol version.
pub const API_VERSION: u32 = 1;
/// Shared preference key for field-merged extension settings.
pub const STORAGE_KEY: &str = "extension_preferences";
/// Maximum installed manifest size in bytes.
pub const MANIFEST_LIMIT: usize = 64 * 1024;
/// Maximum discovered installed extension manifests.
pub const MAX_EXTENSIONS: usize = 64;

/// Bundled native capabilities; disabling one stops its activation without removing compiled
/// code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// Project-host Git tools and source-control panels.
    Git,
    /// On-demand host and GPU monitoring.
    Monitor,
    /// Project-host Python environment discovery/creation.
    Python,
    /// Registered language formatting/check/run/build tools.
    Languages,
    /// Project-scoped coding-agent chat.
    Agents,
    /// Read-only raster document previews.
    Images,
    /// Native Markdown rendering and inline images.
    Markdown,
    /// Read-only PDF document previews.
    Pdf,
    /// Hex editing and static binary analysis.
    Binary,
    /// Native notebook cells and explicitly started kernels.
    Notebooks,
    /// Marp slide previews and exports.
    Marp,
    /// LaTeX compiler/format/check tools.
    Latex,
    /// Interactive project debugging through DAP.
    Debugger,
}
impl Feature {
    /// Stable namespaced bundled-feature identifier used by preferences.
    pub fn id(self) -> &'static str {
        match self {
            Self::Git => "rode.git",
            Self::Monitor => "rode.monitor",
            Self::Python => "rode.python",
            Self::Languages => "rode.languages",
            Self::Agents => "rode.agents",
            Self::Images => "rode.images",
            Self::Markdown => "rode.markdown",
            Self::Pdf => "rode.pdf",
            Self::Binary => "rode.binary",
            Self::Notebooks => "rode.notebooks",
            Self::Marp => "rode.marp",
            Self::Latex => "rode.latex",
            Self::Debugger => "rode.debugger",
        }
    }
}
/// One installed adapter command contributed to the palette.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCommand {
    /// Stable command identifier within its extension.
    pub id: String,
    /// User-visible command title.
    pub title: String,
}
/// Installed executable capabilities declared by a validated manifest.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    /// Installed executable on the project host, not a shell command string.
    pub program: String,
    /// Literal executable arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Language identifiers that activate this tool.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Whether the adapter accepts buffer formatting operations.
    #[serde(default)]
    pub format: bool,
    /// Whether the adapter accepts buffer diagnostic operations.
    #[serde(default)]
    pub check: bool,
    /// Explicit commands contributed to the command palette.
    #[serde(default)]
    pub commands: Vec<ToolCommand>,
}
/// Desktop-installed metadata for an opt-in project-host adapter; repository files are not
/// automatically executed as manifests.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Required protocol version, currently [`API_VERSION`].
    pub api: u32,
    /// Stable namespaced extension identifier.
    pub id: String,
    /// User-visible extension name.
    pub name: String,
    /// Extension version label.
    pub version: String,
    /// Bounded description shown in the extension manager.
    pub description: String,
    /// Declared executable and activation capabilities.
    pub tool: Tool,
}
impl Manifest {
    /// Validate manifest identity, capabilities and bounded arguments without executing it.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.api == API_VERSION,
            "Unsupported extension API; RODE supports version 1."
        );
        ensure!(
            valid_id(&self.id) && !self.id.starts_with("rode."),
            "Use a unique publisher.name ID; rode.* is reserved."
        );
        ensure!(
            !self.name.trim().is_empty()
                && self.name.len() <= 80
                && self.version.len() <= 40
                && self.description.len() <= 1024,
            "Invalid extension description."
        );
        let tool = &self.tool;
        // No shell interpolation, relative project executable or implicit upload.
        ensure!(
            !tool.program.is_empty()
                && tool.program.len() <= 4096
                && !tool.program.contains('\0')
                && (!tool.program.contains('/') || Path::new(&tool.program).is_absolute()),
            "The program must be an absolute project-host path or a name on its PATH."
        );
        ensure!(
            tool.args.len() <= 64
                && tool.args.iter().map(String::len).sum::<usize>() <= 16 * 1024
                && tool.args.iter().all(|s| !s.contains('\0')),
            "Invalid adapter arguments."
        );
        ensure!(
            tool.languages.len() <= 64
                && tool
                    .languages
                    .iter()
                    .all(|s| !s.is_empty() && s.len() <= 40),
            "Invalid language list."
        );
        ensure!(
            tool.commands.len() <= 32
                && tool
                    .commands
                    .iter()
                    .all(|c| valid_id(&c.id) && !c.title.trim().is_empty() && c.title.len() <= 120),
            "Invalid commands."
        );
        let unique: std::collections::BTreeSet<_> = tool.commands.iter().map(|c| &c.id).collect();
        ensure!(
            unique.len() == tool.commands.len(),
            "Duplicate command IDs."
        );
        ensure!(
            tool.format || tool.check || !tool.commands.is_empty(),
            "Declare a formatter, checker or command."
        );
        Ok(())
    }
}
fn valid_id(id: &str) -> bool {
    (3..=80).contains(&id.len())
        && id.contains('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
}

/// One extension's field-merged preferences.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Preference {
    /// Explicit enable/disable override; absence uses the feature's default policy.
    pub enabled: Option<bool>,
    /// Namespaced JSON settings sent only to this extension.
    pub settings: BTreeMap<String, serde_json::Value>,
}
/// Preferences keyed by stable extension identifier.
pub type Preferences = BTreeMap<String, Preference>;
/// Parse and validate bounded namespaced preferences, preserving storage on failure.
pub fn preferences(text: &str) -> Result<Preferences> {
    ensure!(
        text.len() <= 256 * 1024,
        "Extension preferences exceed 256 KiB."
    );
    let prefs: Preferences = serde_json::from_str(text)?;
    ensure!(
        prefs.len() <= 128 && prefs.keys().all(|id| valid_id(id)),
        "Invalid extension preference IDs."
    );
    Ok(prefs)
}

/// One native or installed extension-manager entry.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Stable extension identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// User-visible feature description.
    pub description: String,
    /// Human-readable activation condition.
    pub activation: &'static str,
    /// Bundled capability, absent for installed adapters.
    pub feature: Option<Feature>,
    /// Validated installed adapter manifest, absent for bundled features.
    pub manifest: Option<Manifest>,
}
/// Bounded registry of bundled capabilities and desktop-installed adapter manifests.
#[derive(Default)]
pub struct Registry {
    /// Entries shown in the extension manager.
    pub entries: Vec<Entry>,
    /// Explicit per-extension overrides/settings.
    pub preferences: Preferences,
    /// Nonfatal manifest/discovery warnings.
    pub warnings: Vec<String>,
    /// Generation incremented by activation changes to invalidate stale work.
    pub revision: u64,
    /// Whether explicit preference changes await persistence.
    pub pending: bool,
}
impl Registry {
    /// Discover bounded installed manifests at startup; malformed manifests become warnings and
    /// no process starts.
    pub fn load(directory: &Path, preferences: Preferences) -> Self {
        let mut registry = Self {
            preferences,
            ..Default::default()
        };
        for (feature, name, description, activation) in [
            (
                Feature::Debugger,
                "Debugger",
                "Interactive project debugging with installed Python, LLDB and custom DAP adapters, locally or over SSH.",
                "Explicit debug action",
            ),
            (
                Feature::Git,
                "Git",
                "Status, diffs, commits, branches, worktrees and remote operations.",
                "Visible panel / command",
            ),
            (
                Feature::Monitor,
                "System monitor",
                "Read-only host CPU, memory and GPU telemetry.",
                "Visible panel only",
            ),
            (
                Feature::Python,
                "Python environments",
                "Environment discovery, selection and creation.",
                "Python document / environment action",
            ),
            (
                Feature::Languages,
                "Language tools",
                "Bundled formatters, diagnostics and run/build commands.",
                "Relevant document / command",
            ),
            (
                Feature::Agents,
                "Agent chat",
                "Codex, ACP and local-model chat on the project host.",
                "Process starts on Send",
            ),
            (
                Feature::Images,
                "Image previews",
                "Read-only raster previews with fit, zoom and pan.",
                "Opened image",
            ),
            (
                Feature::Markdown,
                "Markdown previews",
                "Native rendering from the current unsaved buffer.",
                "Preview action",
            ),
            (
                Feature::Pdf,
                "PDF previews",
                "Bounded, read-only Poppler document views.",
                "Opened PDF",
            ),
            (
                Feature::Binary,
                "Hex & binary analysis",
                "Byte patches, object headers, symbols, strings and x86 disassembly.",
                "Opened binary / hex action",
            ),
            (
                Feature::Notebooks,
                "Jupyter notebooks",
                "Code/Markdown cells, saved outputs and project-host kernels.",
                "Opened notebook / explicit Run",
            ),
            (
                Feature::Marp,
                "Marp presentations",
                "Accurate native slide previews and HTML/PDF/PowerPoint exports using installed project-host Marp tools.",
                "Explicit Slides / Export action",
            ),
            (
                Feature::Latex,
                "LaTeX tools",
                "Configurable project-host TeX builds, compiler diagnostics and native PDF preview. Optional latexindent and ChkTeX buffer tools.",
                "LaTeX source / explicit Build",
            ),
        ] {
            registry.entries.push(Entry {
                id: feature.id().into(),
                name: name.into(),
                description: description.into(),
                activation,
                feature: Some(feature),
                manifest: None,
            });
        }
        if let Ok(scan) = std::fs::read_dir(directory) {
            let mut paths: Vec<_> = scan
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .take(MAX_EXTENSIONS + 1)
                .collect();
            paths.sort();
            if paths.len() > MAX_EXTENSIONS {
                registry
                    .warnings
                    .push("Only 64 installed extension manifests are loaded.".into());
                paths.truncate(MAX_EXTENSIONS);
            }
            for path in paths {
                match read_manifest(&path) {
                    Ok(manifest) if !registry.entries.iter().any(|e| e.id == manifest.id) => {
                        registry.entries.push(Entry {
                            id: manifest.id.clone(),
                            name: manifest.name.clone(),
                            description: manifest.description.clone(),
                            activation: "Explicit command / matching enabled language tool",
                            feature: None,
                            manifest: Some(manifest),
                        })
                    }
                    Ok(_) => registry
                        .warnings
                        .push(format!("Duplicate extension: {}", path.display())),
                    Err(error) => registry
                        .warnings
                        .push(format!("{}: {error:#}", path.display())),
                }
            }
        }
        registry
    }
    /// Return the effective enabled state for an identifier.
    pub fn enabled(&self, id: &str) -> bool {
        self.entries.iter().find(|e| e.id == id).is_some_and(|e| {
            self.preferences
                .get(id)
                .and_then(|p| p.enabled)
                .unwrap_or(e.feature.is_some())
        })
    }
    /// Return whether a bundled capability may activate.
    pub fn feature(&self, feature: Feature) -> bool {
        self.enabled(feature.id())
    }
    /// Record an explicit enable/disable edit and advance the activation generation.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        self.preferences.entry(id.into()).or_default().enabled = Some(enabled);
        self.revision = self.revision.wrapping_add(1);
        self.pending = true;
    }
    /// Find the first enabled adapter matching a language and all requested format/check
    /// capabilities.
    pub fn provider(&self, language: &str, format: bool, check: bool) -> Option<&Manifest> {
        self.entries
            .iter()
            .filter(|e| self.enabled(&e.id))
            .filter_map(|e| e.manifest.as_ref())
            .find(|m| {
                m.tool.languages.iter().any(|l| l == language)
                    && (!format || m.tool.format)
                    && (!check || m.tool.check)
            })
    }
    /// Return enabled palette contributions as extension id, command id and display title.
    pub fn commands(&self) -> Vec<(String, String, String)> {
        self.entries
            .iter()
            .filter(|e| self.enabled(&e.id))
            .filter_map(|e| e.manifest.as_ref())
            .flat_map(|m| {
                m.tool.commands.iter().map(move |c| {
                    (
                        m.id.clone(),
                        c.id.clone(),
                        format!("{}: {}", m.name, c.title),
                    )
                })
            })
            .collect()
    }
    /// Capture a validated adapter's literal command, source snapshot and namespaced settings.
    pub fn invocation(
        &self,
        manifest: &Manifest,
        operation: String,
        path: Option<String>,
        text: String,
    ) -> Invocation {
        Invocation {
            api: API_VERSION,
            extension: manifest.id.clone(),
            program: manifest.tool.program.clone(),
            args: manifest.tool.args.clone(),
            operation,
            path,
            text,
            settings: self
                .preferences
                .get(&manifest.id)
                .map(|p| p.settings.clone())
                .unwrap_or_default(),
        }
    }
}
/// Resolve the desktop's installed-manifest directory under RODE configuration.
pub fn directory() -> PathBuf {
    crate::ssh::config_path()
        .parent()
        .expect("config parent")
        .join("extensions")
}
/// Read and validate a bounded manifest without executing its tool.
pub fn read_manifest(path: &Path) -> Result<Manifest> {
    let data =
        crate::persistence::read(path, MANIFEST_LIMIT)?.context("Manifest does not exist")?;
    let manifest: Manifest = serde_json::from_slice(&data)?;
    manifest.validate()?;
    Ok(manifest)
}
/// Explicitly install a validated manifest into the desktop registry; executable
/// deployment/installation remains user-managed.
pub fn install(source: &Path, directory: &Path) -> Result<()> {
    let manifest = read_manifest(source)?;
    std::fs::create_dir_all(directory)?;
    let destination = directory.join(format!("{}.json", manifest.id));
    crate::persistence::transaction(&destination, |exists| {
        ensure!(
            !exists,
            "This extension is already installed; remove its manifest before replacing it."
        );
        crate::persistence::replace(&destination, &serde_json::to_vec_pretty(&manifest)?, false)
    })
}

/// This is a one-shot request over existing project transport, not a plugin daemon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invocation {
    /// Required adapter protocol version.
    pub api: u32,
    /// Owning extension identifier.
    pub extension: String,
    /// Project-host executable captured from its validated manifest.
    pub program: String,
    /// Literal executable arguments.
    pub args: Vec<String>,
    /// Requested format/check/command operation.
    pub operation: String,
    /// Optional project-host source path used only as tool context.
    pub path: Option<String>,
    /// Unsaved source snapshot for buffer operations.
    pub text: String,
    /// Namespaced extension settings captured with this invocation.
    pub settings: BTreeMap<String, serde_json::Value>,
}
/// Bounded API v1 tool result, validated before UI application.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    /// Reply protocol version, which must match the supported API.
    pub api: u32,
    /// Optional formatted replacement snapshot; the UI must check the original buffer version.
    #[serde(default)]
    pub text: Option<String>,
    /// Bounded normalized diagnostics for the submitted source.
    #[serde(default)]
    pub diagnostics: Vec<crate::project::Diagnostic>,
    /// Bounded human-readable command output.
    #[serde(default)]
    pub output: String,
}
impl Reply {
    /// Validate reply shape, size and operation-specific fields before accepting adapter
    /// output.
    pub fn validate(&self, operation: &str) -> Result<()> {
        ensure!(self.api == API_VERSION, "Unsupported adapter response API.");
        ensure!(
            self.text
                .as_ref()
                .is_none_or(|s| s.len() <= crate::files::MAX_FILE_BYTES as usize)
                && self.output.len() <= 1024 * 1024
                && self.diagnostics.len() <= 4096,
            "Adapter response exceeds limits."
        );
        ensure!(
            operation == "format" || self.text.is_none(),
            "Only format may return replacement text."
        );
        ensure!(
            self.diagnostics.iter().all(|d| d.row > 0
                && d.column > 0
                && d.end_row >= d.row
                && d.end_column > 0
                && [d.row, d.column, d.end_row, d.end_column]
                    .iter()
                    .all(|&n| n <= crate::files::MAX_FILE_BYTES as usize + 1)
                && (d.end_row != d.row || d.end_column >= d.column)
                && d.message.len() <= 8192
                && d.code.len() <= 128
                && ["error", "warning", "info"].contains(&d.severity.as_str())),
            "Invalid adapter diagnostic (positions are 1-based Unicode scalar columns)."
        );
        Ok(())
    }
}
/// Execute a blocking one-shot adapter with bounded input/output/time; the caller supplies the
/// enclosing project sandbox.
pub fn run(root: &str, invocation: &Invocation) -> Result<Reply> {
    run_cancellable(root, invocation, &std::sync::atomic::AtomicBool::new(false))
}
pub(crate) fn run_cancellable(
    root: &str,
    invocation: &Invocation,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Reply> {
    use std::{process::Command, time::Duration};
    ensure!(
        invocation.api == API_VERSION
            && invocation.text.len() <= crate::files::MAX_FILE_BYTES as usize,
        "Invalid adapter request."
    );
    let root = crate::files::expand_home(root).canonicalize()?;
    let payload = serde_json::json!({"api": API_VERSION, "extension": invocation.extension, "operation": invocation.operation, "root": root, "path": invocation.path, "text": invocation.text, "settings": invocation.settings});
    let mut command = Command::new(&invocation.program);
    command.args(&invocation.args).current_dir(&root);
    let output = crate::project::run_controlled(
        command,
        serde_json::to_vec(&payload)?,
        Duration::from_secs(20),
        cancel,
        false,
    )?
    .checked()?;
    let reply: Reply = serde_json::from_str(&output)
        .context("Adapter must return one API v1 JSON object on stdout")?;
    reply.validate(&invocation.operation)?;
    Ok(reply)
}

/// The coordinator is launched inside the actual workspace policy. Its stdin
/// remains open as a lifetime channel; EOF cancels the adapter's process group.
pub fn coordinator() -> Result<()> {
    use std::{
        io::{BufRead, Read},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .take(64 * 1024 * 1024)
        .read_line(&mut line)?;
    let request: crate::project::Request = serde_json::from_str(&line)?;
    let crate::project::Request::Extension { root, invocation } = request else {
        anyhow::bail!("Expected an extension request");
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let control = cancel.clone();
    std::thread::spawn(move || {
        let _ = std::io::stdin().lock().read(&mut [0]);
        control.store(true, Ordering::Release);
    });
    let reply = run_cancellable(&root, &invocation, &cancel)?;
    serde_json::to_writer(std::io::stdout().lock(), &reply)?;
    Ok(())
}
pub(crate) fn execute(
    command: std::process::Command,
    request: &crate::project::Request,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Reply> {
    execute_with_prelude(command, request, cancel, vec![])
}

pub(crate) fn execute_with_prelude(
    command: std::process::Command,
    request: &crate::project::Request,
    cancel: &std::sync::atomic::AtomicBool,
    mut input: Vec<u8>,
) -> Result<Reply> {
    input.extend(serde_json::to_vec(request)?);
    input.push(b'\n');
    let output = crate::project::run_controlled(
        command,
        input,
        std::time::Duration::from_secs(25),
        cancel,
        true,
    )?
    .checked()?;
    serde_json::from_str(&output).context("Invalid extension coordinator response")
}

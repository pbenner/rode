//! Native feature registry and an explicitly enabled, versioned tool adapter API.
//! Manifests describe capabilities. Tool adapters run on the project host; native
//! panel providers run as independent desktop executables, including bundled ones.
//!
//! # Design decisions
//!
//! Distribution and execution are independent. Included and installed panel
//! executables declare the same contributions and use the same bounded process,
//! settings, rendering and window lifecycle. Included distribution only supplies
//! default enablement and sibling-executable discovery. Native controllers for
//! editor/host services retain their existing authority and document guards.
//!
//! Explicitly installed manifests start disabled. Reading a repository or a
//! catalogue cannot implicitly execute it. Project tool replies remain subject
//! to captured host policy and version-checked source-save guards. Desktop panel
//! providers use the shared data-only protocol rather than a dynamic Rust ABI.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Shared preference key for field-merged extension settings.
pub const STORAGE_KEY: &str = "extension_preferences";
/// Maximum installed manifest size in bytes.
pub const MANIFEST_LIMIT: usize = 64 * 1024;
/// Maximum discovered installed extension manifests.
pub const MAX_EXTENSIONS: usize = 64;

/// Stable preference identities of included extensions. Native service modules
/// and independent executables share these enablement records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// Explicit local/SSH file copies, independent of project navigation.
    Transfers,
    /// Explicitly bookmarked local and SSH project locations in the Explorer.
    ProjectBookmarks,
    /// Optional mise toolchain, environment and task integration; disabled by default.
    Mise,
    /// Project-host Git tools and source-control panels.
    Git,
    /// Explicitly started host and GPU monitoring, including while the panel is hidden.
    Monitor,
    /// Desktop stock search, watchlists and interactive market charts.
    Stocks,
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
    /// Read-only native PowerPoint presentation previews.
    Pptx,
    /// Read-only native Word document previews.
    Docx,
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
            Self::Transfers => "rode.transfers",
            Self::ProjectBookmarks => "rode.project-bookmarks",
            Self::Mise => "rode.mise",
            Self::Git => "rode.git",
            Self::Monitor => "rode.monitor",
            Self::Stocks => "rode.stocks",
            Self::Python => "rode.python",
            Self::Languages => "rode.languages",
            Self::Agents => "rode.agents",
            Self::Images => "rode.images",
            Self::Markdown => "rode.markdown",
            Self::Pdf => "rode.pdf",
            Self::Pptx => "rode.pptx",
            Self::Docx => "rode.docx",
            Self::Binary => "rode.binary",
            Self::Notebooks => "rode.notebooks",
            Self::Marp => "rode.marp",
            Self::Latex => "rode.latex",
            Self::Debugger => "rode.debugger",
        }
    }
}
use rode_extension_api::manifest::valid_id;
pub use rode_extension_api::manifest::{Manifest, Panel, Tool, ToolCommand};
pub use rode_extension_api::tool::{API_VERSION, Invocation, Reply};

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
    /// Included distribution identity and default enablement; independent of runtime.
    pub feature: Option<Feature>,
    /// Executable contribution, shared by included and independently installed providers.
    pub manifest: Option<Manifest>,
}
impl Entry {
    /// Panel capability is independent of whether a provider ships with RODE.
    pub fn panel(&self) -> Option<Panel<'_>> {
        self.manifest.as_ref().and_then(Manifest::panel)
    }
    /// Native controllers and project-tool contributions can invalidate project work.
    pub fn project_scoped(&self) -> bool {
        self.manifest.as_ref().is_none_or(Manifest::project_scoped)
    }
    /// Distribution affects default enablement and executable discovery only.
    pub fn included(&self) -> bool {
        self.feature.is_some()
    }
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
                Feature::ProjectBookmarks,
                "Project bookmarks",
                "Save local and SSH project locations in the Explorer and reopen them through the normal project guards.",
                "Explicit bookmark / reopen action",
            ),
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
                "Explicit Start / Stop; continues while hidden",
            ),
            (
                Feature::Stocks,
                "Stocks",
                "Global symbol search, saved watchlists and native price/volume charts. Uses desktop internet access on demand.",
                "Opened panel / explicit search or refresh",
            ),
            (
                Feature::Transfers,
                "File transfers",
                "Browse and copy local/SSH files, and share live folders from project tools.",
                "Explicit browse / copy / mount, or reopen a saved project",
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
                "Bundled Rust PDF reader with cached and continuous pages.",
                "Opened PDF",
            ),
            (
                Feature::Pptx,
                "PowerPoint previews",
                "Bundled native PPTX reader with cached slides, zoom and fullscreen.",
                "Opened PPTX",
            ),
            (
                Feature::Docx,
                "Word previews",
                "Bundled native DOCX reader with cached pages, zoom and fullscreen.",
                "Opened DOCX",
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
            (
                Feature::Mise,
                "mise",
                "Use installed mise toolchains and project environments; discover and run mise tasks on the project host.",
                "Explicit project opt-in / task action",
            ),
        ] {
            registry.entries.push(Entry {
                id: feature.id().into(),
                name: name.into(),
                description: description.into(),
                activation,
                feature: Some(feature),
                manifest: (feature == Feature::Stocks).then(|| serde_json::from_value(serde_json::json!({
                    "api":1,"id":feature.id(),"name":name,"version":"0.1.0","description":description,
                    "contributions":[{"kind":"panel","host":"desktop","protocol":2,"icon":"chart","tool":{"program":"rode-stocks","args":["--stdio"]}}]
                })).expect("bundled manifest is valid")),
            });
        }
        if let Ok(scan) = std::fs::read_dir(directory) {
            let mut paths = std::collections::BTreeSet::new();
            let mut truncated = false;
            for path in scan
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
            {
                paths.insert(path);
                if paths.len() > MAX_EXTENSIONS {
                    paths.pop_last();
                    truncated = true;
                }
            }
            if truncated {
                registry.warnings.push(
                    "Only the first 64 installed extension manifests in filename order are loaded."
                        .into(),
                );
            }
            for path in paths {
                match read_manifest(&path) {
                    Ok(manifest) if !registry.entries.iter().any(|e| e.id == manifest.id) => {
                        registry.entries.push(Entry {
                            id: manifest.id.clone(),
                            name: manifest.name.clone(),
                            description: manifest.description.clone(),
                            activation: if manifest.panel().is_some() && manifest.project_scoped() {
                                "Desktop panel / project-host tools"
                            } else if manifest.panel().is_some() {
                                "Explicit desktop panel / optional background refresh"
                            } else {
                                "Explicit command / matching enabled language tool"
                            },
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
                .unwrap_or(e.feature.is_some_and(|feature| feature != Feature::Mise))
        })
    }
    /// One contribution inventory for menus, activity buttons and execution.
    pub fn panels(&self) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(|e| self.enabled(&e.id) && e.panel().is_some())
    }
    /// Return whether a bundled capability may activate.
    pub fn feature(&self, feature: Feature) -> bool {
        self.enabled(feature.id())
    }
    /// Record an explicit enable/disable edit and advance the activation generation.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        self.preferences.entry(id.into()).or_default().enabled = Some(enabled);
        // A desktop-only panel cannot change formatter/checker selection or
        // invalidate captured project-host work.
        if self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .is_none_or(Entry::project_scoped)
        {
            self.revision = self.revision.wrapping_add(1);
        }
        self.pending = true;
    }
    /// Publish a bounded settings edit, invalidating only project-scoped work.
    /// Validation precedes mutation so a rejected edit leaves saved preferences intact.
    pub fn set_settings(
        &mut self,
        id: &str,
        settings: BTreeMap<String, serde_json::Value>,
    ) -> Result<bool> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .context("This extension is no longer installed")?;
        let project_scoped = entry.project_scoped();
        ensure!(
            serde_json::to_vec(&settings)?.len() <= rode_extension_api::SETTINGS_LIMIT,
            "Extension settings exceed 192 KiB"
        );
        if self
            .preferences
            .get(id)
            .map_or(settings.is_empty(), |p| p.settings == settings)
        {
            return Ok(false);
        }
        let mut next = self.preferences.clone();
        next.entry(id.into()).or_default().settings = settings;
        preferences(&serde_json::to_string(&next)?)?;
        self.preferences = next;
        self.pending = true;
        if project_scoped {
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(true)
    }
    /// Compare the effective ordered project providers/settings before reloading.
    /// Panel-only changes and manifest display metadata do not invalidate source-tool jobs.
    pub fn same_project_tools(&self, other: &Self) -> bool {
        let project_tools = |entry: &Entry| {
            entry
                .manifest
                .as_ref()
                .and_then(Manifest::project_tool)
                .is_some()
        };
        fn settings(prefs: &Preference) -> Option<&BTreeMap<String, serde_json::Value>> {
            (!prefs.settings.is_empty()).then_some(&prefs.settings)
        }
        // Keep each iterator's references tied to its owning registry.
        let left = self
            .entries
            .iter()
            .filter(|e| self.enabled(&e.id) && project_tools(e))
            .map(|e| {
                (
                    &e.id,
                    e.manifest.as_ref().and_then(Manifest::project_tool),
                    self.preferences.get(&e.id).and_then(settings),
                )
            });
        let right = other
            .entries
            .iter()
            .filter(|e| other.enabled(&e.id) && project_tools(e))
            .map(|e| {
                (
                    &e.id,
                    e.manifest.as_ref().and_then(Manifest::project_tool),
                    other.preferences.get(&e.id).and_then(settings),
                )
            });
        left.eq(right)
    }
    /// Find the first enabled adapter matching a language and all requested format/check
    /// capabilities.
    pub fn provider(&self, language: &str, format: bool, check: bool) -> Option<&Manifest> {
        self.entries
            .iter()
            .filter(|e| self.enabled(&e.id))
            .filter_map(|e| e.manifest.as_ref())
            .find(|m| {
                m.project_tool().is_some_and(|tool| {
                    tool.languages.iter().any(|l| l == language)
                        && (!format || tool.format)
                        && (!check || tool.check)
                })
            })
    }
    /// Return enabled palette contributions as extension id, command id and display title.
    pub fn commands(&self) -> Vec<(String, String, String)> {
        self.entries
            .iter()
            .filter(|e| self.enabled(&e.id))
            .filter_map(|e| e.manifest.as_ref())
            .flat_map(|m| {
                let mut actions: Vec<_> = m
                    .project_tool()
                    .into_iter()
                    .flat_map(|tool| &tool.commands)
                    .map(move |c| {
                        (
                            m.id.clone(),
                            c.id.clone(),
                            format!("{}: {}", m.name, c.title),
                        )
                    })
                    .collect();
                if m.panel().is_some() {
                    actions.push((
                        m.id.clone(),
                        "@panel.open".into(),
                        format!("{}: Open panel", m.name),
                    ));
                }
                actions
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
    ) -> Result<Invocation> {
        let tool = manifest
            .project_tool()
            .context("This extension has no project-tool contribution")?;
        ensure!(
            (operation == "format" && tool.format)
                || (operation == "check" && tool.check)
                || tool.commands.iter().any(|c| c.id == operation),
            "This project-tool operation is not declared"
        );
        Ok(Invocation {
            api: API_VERSION,
            extension: manifest.id.clone(),
            program: tool.program.clone(),
            args: tool.args.clone(),
            operation,
            path,
            text,
            settings: self
                .preferences
                .get(&manifest.id)
                .map(|p| p.settings.clone())
                .unwrap_or_default(),
        })
    }
}
/// Resolve the desktop's installed-manifest directory under RODE configuration.
pub fn directory() -> PathBuf {
    crate::files::expand_home(
        &std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| "~/.config".into()),
    )
    .join("rode/connections.json")
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

/// Execute a blocking one-shot adapter with bounded input/output/time; the caller supplies the
/// enclosing project sandbox.
pub fn run(root: &str, invocation: &Invocation) -> Result<Reply> {
    run_cancellable(root, invocation, &std::sync::atomic::AtomicBool::new(false))
}
/// Validate a versioned adapter invocation and run literal arguments in its host root
/// with bounded JSON I/O, deadline and scoped cancellation.
pub fn run_cancellable(
    root: &str,
    invocation: &Invocation,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Reply> {
    use std::{process::Command, time::Duration};
    ensure!(
        invocation.api == API_VERSION
            && invocation.text.len() <= crate::files::MAX_TOOL_BYTES as usize,
        "Invalid adapter request."
    );
    let root = crate::files::expand_home(root).canonicalize()?;
    let payload = rode_extension_api::tool::Request {
        api: API_VERSION,
        extension: invocation.extension.clone(),
        operation: invocation.operation.clone(),
        root: root.to_string_lossy().into_owned(),
        path: invocation.path.clone(),
        text: invocation.text.clone(),
        settings: invocation.settings.clone(),
    };
    let mut command = Command::new(&invocation.program);
    command.args(&invocation.args).current_dir(&root);
    let output = crate::command::run_controlled(
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
    crate::coordinator::serve(
        64 * 1024 * 1024,
        |request: &crate::project::Request, cancel| {
            let crate::project::Request::Extension { root, invocation } = request else {
                anyhow::bail!("Expected an extension request");
            };
            run_cancellable(root, invocation, cancel)
        },
    )
}
/// Run the prepared adapter coordinator command with a single framed project request.
pub fn execute<S: Serialize>(
    command: std::process::Command,
    request: &crate::project::Request<S>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Reply> {
    let mut input = serde_json::to_vec(request)?;
    input.push(b'\n');
    let output = crate::command::run_controlled(
        command,
        input,
        std::time::Duration::from_secs(25),
        cancel,
        true,
    )?
    .checked()?;
    serde_json::from_str(&output).context("Invalid extension coordinator response")
}

//! Reverse one recorded agent patch on its captured project host. A disposable
//! Git directory parses/applies patches without touching the project's index,
//! commits, configuration or references; non-Git project folders also work.
use crate::agent_files::Root;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::Path,
    process::Command,
    time::Duration,
};

const MAX_PATCH: usize = 512 * 1024;

/// Prepared reverse patch and exact post-turn file identities, retained in RAM.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Saved {
    /// Normalized project-relative unified patch.
    pub patch: String,
    /// Exact files, including both sides of a rename.
    pub files: Vec<(String, Option<(String, u32)>)>,
}
/// Private controller-to-helper operation; the UI sends only a turn identity.
#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    /// Validate a completed turn's patch and capture its resulting file versions.
    Prepare {
        /// Captured project root on this host.
        root: String,
        /// Complete recorded unified diff for one prompt.
        patch: String,
        /// Configuration aliases and active executables protected by the controller.
        #[serde(default)]
        protected_paths: Vec<String>,
    },
    /// Reject changed versions before reversing the complete patch.
    Apply {
        /// The original project root, never a frontend-selected replacement.
        root: String,
        /// Validated patch and exact post-turn versions.
        saved: Saved,
        /// The captured project restriction, never widened for Undo.
        read_only: bool,
        /// The same protected aliases used when the record was prepared.
        #[serde(default)]
        protected_paths: Vec<String>,
    },
}

fn command(root: &Root, repository: &Path, initialize: bool) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(format!("/proc/self/fd/{}", root.directory.as_raw_fd()))
        .args(["--no-pager", "--literal-pathspecs"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ATTR_NOSYSTEM", "1");
    if !initialize {
        command
            .arg(format!("--git-dir={}", repository.display()))
            .arg("--work-tree=.");
    }
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_NAMESPACE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(key);
    }
    command
}
fn git(root: &Root, repository: &Path, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
    let mut process = command(root, repository, args.first() == Some(&"init"));
    process.args(args);
    crate::command::run_bounded_limited(
        process,
        input.to_vec(),
        Duration::from_secs(15),
        MAX_PATCH * 2,
    )
}
fn identity(root: &Root, path: &str, remaining: &mut u64) -> Result<Option<(String, u32)>> {
    let path = root.relative(path, false)?;
    let file = match root.file(&path, libc::O_PATH) {
        Ok(file) => file,
        Err(error)
            if error.chain().any(|e| {
                e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            }) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "Undo supports regular project files only"
    );
    ensure!(
        metadata.len() <= 16 * 1024 * 1024,
        "Undo file exceeds 16 MiB"
    );
    ensure!(
        metadata.len() <= *remaining,
        "Undo files exceed 64 MiB in total"
    );
    let mut bytes = Vec::new();
    std::fs::File::open(format!("/proc/self/fd/{}", file.as_raw_fd()))?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 16 * 1024 * 1024, "Undo file exceeds 16 MiB");
    *remaining = remaining
        .checked_sub(bytes.len() as u64)
        .context("Undo files exceed 64 MiB in total")?;
    Ok(Some((
        crate::files::digest(&bytes),
        metadata.permissions().mode() & 0o777,
    )))
}

fn decode_path(value: &str) -> Result<String> {
    if value.starts_with('"') {
        // Git C-quotes UTF-8 bytes using octal escapes, whereas some agents
        // use JSON quoting. Accept both without lossy filename conversion.
        if let Ok(path) = serde_json::from_str(value) {
            return Ok(path);
        }
        let quoted = value
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .context("Invalid quoted patch path")?;
        let mut input = quoted.bytes();
        let mut path = Vec::new();
        while let Some(byte) = input.next() {
            if byte != b'\\' {
                ensure!(byte != b'"', "Invalid quoted patch path");
                path.push(byte);
                continue;
            }
            let byte = input.next().context("Truncated quoted patch path")?;
            path.push(match byte {
                b'\\' | b'"' => byte,
                b'0'..=b'3' => {
                    let second = input.next().context("Truncated octal patch path")?;
                    let third = input.next().context("Truncated octal patch path")?;
                    ensure!(
                        matches!(second, b'0'..=b'7') && matches!(third, b'0'..=b'7'),
                        "Invalid octal patch path"
                    );
                    (byte - b'0') * 64 + (second - b'0') * 8 + (third - b'0')
                }
                _ => anyhow::bail!("Unsupported quoted patch path"),
            });
        }
        Ok(String::from_utf8(path).context("Undo paths must be UTF-8")?)
    } else {
        Ok(value.split('\t').next().unwrap_or_default().into())
    }
}
fn normalize(root: &str, patch: &str) -> Result<String> {
    ensure!(
        patch.len() <= MAX_PATCH,
        "The prompt's file changes exceed the undo limit"
    );
    let relative = |value: &str| -> Result<Option<String>> {
        let value = decode_path(value)?;
        if value == "/dev/null" {
            return Ok(None);
        }
        let value = value
            .strip_prefix("a/")
            .or_else(|| value.strip_prefix("b/"))
            .unwrap_or(&value);
        let path = Path::new(value);
        let path = if path.is_absolute() {
            path.strip_prefix(root)
                .context("Patch leaves this project")?
        } else {
            path
        };
        ensure!(
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
                && !value.chars().any(char::is_control),
            "Invalid undo path"
        );
        Ok(Some(
            path.to_str().context("Undo paths must be UTF-8")?.into(),
        ))
    };
    let quote =
        |prefix: &str, value: &str| serde_json::to_string(&format!("{prefix}{value}")).unwrap();
    let mut result = String::new();
    let lines: Vec<_> = patch.split_terminator('\n').collect();
    let mut start = 0;
    while start < lines.len() {
        ensure!(
            lines[start].starts_with("diff --git "),
            "Unsupported agent patch format"
        );
        let end = (start + 1..lines.len())
            .find(|&i| lines[i].starts_with("diff --git "))
            .unwrap_or(lines.len());
        let header_end = (start + 1..end)
            .find(|&i| lines[i].starts_with("@@ "))
            .unwrap_or(end);
        let old = lines[start + 1..header_end]
            .iter()
            .find_map(|l| l.strip_prefix("--- "))
            .context("Undo needs a text diff with file headers")?;
        let new = lines[start + 1..header_end]
            .iter()
            .find_map(|l| l.strip_prefix("+++ "))
            .context("Undo needs a text diff with file headers")?;
        let old = relative(old)?;
        let new = relative(new)?;
        let name = old
            .as_deref()
            .or(new.as_deref())
            .context("Empty undo file path")?;
        result.push_str(&format!(
            "diff --git {} {}\n",
            quote("a/", old.as_deref().unwrap_or(name)),
            quote("b/", new.as_deref().unwrap_or(name))
        ));
        let mut hunk = false;
        for line in &lines[start + 1..end] {
            hunk |= line.starts_with("@@ ");
            if !hunk {
                if let Some(mode) = [
                    "new file mode ",
                    "deleted file mode ",
                    "old mode ",
                    "new mode ",
                ]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                {
                    ensure!(
                        matches!(mode, "100644" | "100755"),
                        "Undo supports regular text files only"
                    );
                }
                ensure!(
                    !line.starts_with("copy from ") && !line.starts_with("copy to "),
                    "Undo cannot safely restore this copy operation"
                );
            }
            if hunk {
                result.push_str(line);
                result.push('\n');
            } else if line.starts_with("--- ") {
                result.push_str(&format!(
                    "--- {}\n",
                    old.as_deref()
                        .map(|p| quote("a/", p))
                        .unwrap_or_else(|| "/dev/null".into())
                ));
            } else if line.starts_with("+++ ") {
                result.push_str(&format!(
                    "+++ {}\n",
                    new.as_deref()
                        .map(|p| quote("b/", p))
                        .unwrap_or_else(|| "/dev/null".into())
                ));
            } else if line.starts_with("rename from ") {
                result.push_str(&format!(
                    "rename from {}\n",
                    serde_json::to_string(old.as_deref().context("Missing rename source")?)?
                ));
            } else if line.starts_with("rename to ") {
                result.push_str(&format!(
                    "rename to {}\n",
                    serde_json::to_string(new.as_deref().context("Missing rename destination")?)?
                ));
            } else {
                result.push_str(line);
                result.push('\n');
            }
        }
        start = end;
    }
    ensure!(
        !result.is_empty(),
        "No recorded file edits in the last prompt"
    );
    Ok(result)
}

/// Execute inside the captured sandbox helper, never as an unrestricted retry.
pub fn execute(request: Request) -> Result<Saved> {
    let (path, patch, protected) = match &request {
        Request::Prepare {
            root,
            patch,
            protected_paths,
        } => (root, patch, protected_paths),
        Request::Apply {
            root,
            saved,
            protected_paths,
            ..
        } => (root, &saved.patch, protected_paths),
    };
    let root = Root::open(path, protected)?;
    let patch = normalize(path, patch)?;
    let temporary = tempfile::tempdir()?;
    let repository = temporary.path().join("git");
    git(
        &root,
        &repository,
        &[
            "init",
            "--bare",
            "--quiet",
            "--template=",
            repository.to_str().context("Invalid temporary path")?,
        ],
        &[],
    )?;
    let output = git(
        &root,
        &repository,
        &["apply", "--reverse", "--numstat", "-z", "--no-index", "-"],
        patch.as_bytes(),
    )?;
    let mut files = Vec::new();
    let mut fields = output.split(|&b| b == 0).filter(|p| !p.is_empty());
    while let Some(field) = fields.next() {
        let text = std::str::from_utf8(field)?;
        let name = text
            .splitn(3, '\t')
            .nth(2)
            .context("Invalid patch file list")?;
        if name.is_empty() {
            for _ in 0..2 {
                files.push(
                    std::str::from_utf8(fields.next().context("Invalid rename")?)?.to_owned(),
                );
            }
        } else {
            files.push(name.into());
        }
    }
    files.sort();
    files.dedup();
    ensure!(
        !files.is_empty() && files.len() <= 64,
        "Undo supports up to 64 changed files per prompt"
    );
    let mut remaining = 64 * 1024 * 1024;
    let versions = files
        .into_iter()
        .map(|p| identity(&root, &p, &mut remaining).map(|v| (p, v)))
        .collect::<Result<Vec<_>>>()?;
    if let Request::Apply {
        saved, read_only, ..
    } = &request
    {
        ensure!(!*read_only, "The project is read only");
        ensure!(
            versions == saved.files,
            "Files changed after this prompt. Undo was refused to preserve your later edits."
        );
    }
    git(
        &root,
        &repository,
        &[
            "apply",
            "--reverse",
            "--check",
            "--no-index",
            "--whitespace=nowarn",
            "-",
        ],
        patch.as_bytes(),
    )?;
    if matches!(request, Request::Apply { .. }) {
        git(
            &root,
            &repository,
            &[
                "apply",
                "--reverse",
                "--no-index",
                "--whitespace=nowarn",
                "-",
            ],
            patch.as_bytes(),
        )?;
    }
    Ok(Saved {
        patch,
        files: versions,
    })
}

/// One bounded private helper request and response; file bytes never reach the GUI.
pub fn serve() -> Result<()> {
    let mut input = Vec::new();
    std::io::stdin()
        .take((MAX_PATCH * 4 + 1) as u64)
        .read_to_end(&mut input)?;
    ensure!(
        input.len() <= MAX_PATCH * 4,
        "Undo request exceeds its size limit"
    );
    let result = execute(serde_json::from_slice(&input)?).map_err(|e| format!("{e:#}"));
    serde_json::to_writer(std::io::stdout().lock(), &result)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn patch(path: &str, old: &str, new: &str) -> String {
        format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-{old}\n+{new}\n"
        )
    }
    fn prepare(root: &Path, patch: String) -> Saved {
        execute(Request::Prepare {
            root: root.to_str().unwrap().into(),
            patch,
            protected_paths: vec![],
        })
        .unwrap()
    }
    fn apply(root: &Path, saved: Saved) -> Result<Saved> {
        execute(Request::Apply {
            root: root.to_str().unwrap().into(),
            saved,
            read_only: false,
            protected_paths: vec![],
        })
    }
    #[test]
    fn reverses_recorded_edits_without_a_repository_and_preserves_later_changes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("file.txt"), "after\n").unwrap();
        let saved = prepare(root, patch("file.txt", "before", "after"));
        std::fs::write(root.join("untouched"), "keep").unwrap();
        apply(root, saved.clone()).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("file.txt")).unwrap(),
            "before\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("untouched")).unwrap(),
            "keep"
        );
        assert!(!root.join(".git").exists());
        std::fs::write(root.join("file.txt"), "my later edit\n").unwrap();
        assert!(apply(root, saved).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("file.txt")).unwrap(),
            "my later edit\n"
        );
    }
    #[test]
    fn rejects_escaping_configuration_and_symlink_paths_before_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(directory.path().join("outside"), "after\n").unwrap();
        std::os::unix::fs::symlink(directory.path().join("outside"), root.join("link")).unwrap();
        for path in ["../outside", ".git/config", ".codex/config.toml", "link"] {
            assert!(
                execute(Request::Prepare {
                    root: root.to_str().unwrap().into(),
                    patch: patch(path, "before", "after"),
                    protected_paths: vec![],
                })
                .is_err(),
                "{path}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(directory.path().join("outside")).unwrap(),
            "after\n"
        );
    }
    #[test]
    fn conflict_in_one_file_preserves_every_file_and_read_only_is_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for name in ["a", "b"] {
            std::fs::write(root.join(name), "after\n").unwrap();
        }
        let saved = prepare(
            root,
            format!(
                "{}{}",
                patch("a", "before", "after"),
                patch("b", "before", "after")
            ),
        );
        assert!(
            execute(Request::Apply {
                root: root.to_str().unwrap().into(),
                saved: saved.clone(),
                read_only: true,
                protected_paths: vec![],
            })
            .is_err()
        );
        std::fs::write(root.join("b"), "later\n").unwrap();
        assert!(apply(root, saved).is_err());
        assert_eq!(std::fs::read_to_string(root.join("a")).unwrap(), "after\n");
    }
    #[test]
    fn restores_deleted_files_and_removes_new_and_empty_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("new.txt"), "created\n").unwrap();
        std::fs::write(root.join("empty.txt"), "").unwrap();
        let diff = concat!(
            "diff --git a/old.txt b/old.txt\ndeleted file mode 100644\n--- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-deleted\n",
            "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+created\n",
            "diff --git a/empty.txt b/empty.txt\nnew file mode 100644\n--- /dev/null\n+++ b/empty.txt\n"
        );
        let saved = prepare(root, diff.into());
        apply(root, saved).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("old.txt")).unwrap(),
            "deleted\n"
        );
        assert!(!root.join("new.txt").exists());
        assert!(!root.join("empty.txt").exists());
    }
    #[test]
    fn leaves_the_real_index_and_preexisting_uncommitted_work_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        git(&["init", "--quiet"]);
        std::fs::write(root.join("file.txt"), "staged\n").unwrap();
        git(&["add", "file.txt"]);
        let index = std::fs::read(root.join(".git/index")).unwrap();
        std::fs::write(root.join("file.txt"), "after\n").unwrap();
        apply(
            root,
            prepare(
                root,
                patch("file.txt", "preexisting unstaged work", "after"),
            ),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("file.txt")).unwrap(),
            "preexisting unstaged work\n"
        );
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(git(&["show", ":file.txt"]), b"staged\n");
    }
    #[test]
    fn normalizes_c_quoted_unicode_and_preserves_crlf_and_header_like_source() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("日本.txt"), "++ after\r\n").unwrap();
        let name = r#""a/\346\227\245\346\234\254.txt""#;
        let diff = format!(
            "diff --git {name} {name}\n--- {name}\n+++ {name}\n@@ -1 +1 @@\n--- before\r\n+++ after\r\n"
        );
        apply(root, prepare(root, diff)).unwrap();
        assert_eq!(
            std::fs::read(root.join("日本.txt")).unwrap(),
            b"-- before\r\n"
        );
    }
}

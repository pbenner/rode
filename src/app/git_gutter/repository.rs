//! Literal, bounded Git plumbing; no index writes, diff drivers or temporary files.
use super::*;
use std::path::{Component, Path};

/// A bound project-host path only; recovered display paths never reach this API.
/// Reject parent components instead of letting a lexical prefix authorize escapes.
pub(super) fn location<'a>(root: &str, path: &'a str) -> Option<(&'a str, String)> {
    let path = Path::new(path);
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return None;
    }
    let relative = path.strip_prefix(Path::new(root)).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    Some((
        path.parent()?.to_str()?,
        format!("./{}", path.file_name()?.to_str()?),
    ))
}

/// Share immutable blob text between refresh and diff-only jobs. Only a tracked
/// blob can be reused from the same HEAD; new/ignored status is checked afresh.
#[derive(Clone)]
pub(super) struct Baseline {
    commit: Option<String>,
    pub text: Arc<str>,
}

fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Resolve one immutable HEAD blob, or an empty baseline for eligible new files.
/// Missing Git/repositories, ignored files, non-regular entries, invalid UTF-8 and
/// policy-denied reads quietly omit decorations. The host bounds command output.
pub(super) fn baseline(
    root: &str,
    path: &str,
    previous: Option<&Baseline>,
    mut run: impl FnMut(&str, Vec<String>) -> Option<crate::project::Output>,
) -> Option<Baseline> {
    let (directory, pathspec) = location(root, path)?;
    let mut git = |args: &[&str]| {
        let args = [
            "--no-optional-locks",
            "--no-replace-objects",
            "-c",
            "core.fsmonitor=false",
        ]
        .into_iter()
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect();
        run(directory, args)
    };
    let head = git(&[
        "rev-parse",
        "--show-prefix",
        "--verify",
        "--quiet",
        "HEAD^{commit}",
    ])?;
    if head.code == 0 {
        // Include the repository-relative directory in the cache identity: a
        // nested repository can point at the same commit but a different file.
        let (_, commit) = head.stdout.strip_suffix('\n')?.rsplit_once('\n')?;
        if !oid(commit) {
            return None;
        }
        if let Some(previous) = previous
            && previous.commit.as_deref() == Some(head.stdout.as_str())
        {
            return Some(previous.clone());
        }
        let entry = git(&["ls-tree", "--full-name", "-z", commit, "--", &pathspec])?;
        if !entry.success() {
            return None;
        }
        if !entry.stdout.is_empty() {
            let (header, _) = entry.stdout.strip_suffix('\0')?.split_once('\t')?;
            let fields: Vec<_> = header.split(' ').collect();
            if fields.len() != 3
                || !matches!(fields[0], "100644" | "100755")
                || fields[1] != "blob"
                || !oid(fields[2])
            {
                return None;
            }
            // Inspect size before fetching a blob; oversized files stay bounded
            // even when the current buffer has been replaced by a short draft.
            let size = git(&["cat-file", "-s", fields[2]])?;
            if !size.success() || size.stdout.trim().parse::<usize>().ok()? > MAX_BYTES {
                return None;
            }
            let content = git(&["cat-file", "blob", fields[2]])?;
            return (content.success()
                && content.stdout.len() <= MAX_BYTES
                && !content.stdout.contains('\0'))
            .then(|| Baseline {
                commit: Some(head.stdout.clone()),
                text: Arc::from(content.stdout),
            });
        }
    } else if head.code != 1 {
        return None;
    }
    let new = git(&[
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
        "--",
        &pathspec,
    ])?;
    (new.success() && !new.stdout.is_empty()).then(|| Baseline {
        commit: None,
        text: Arc::from(""),
    })
}

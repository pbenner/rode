//! Bounded per-turn edit journal. Backend-provided diffs describe file edits;
//! conversation rollback and arbitrary shell-command side effects are separate.
use serde_json::Value;
use std::collections::BTreeMap;

/// Maximum reversible patch retained per prompt, independent of transcript eviction.
pub const MAX_PATCH: usize = 512 * 1024;

/// Host-owned availability for the last completed prompt; never contains file data.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct View {
    /// Completed-turn identity required by an Undo request.
    pub turn: u64,
    /// Project-relative paths represented by the recorded edit.
    pub files: Vec<String>,
    /// Why Undo is unavailable; empty when a valid record is retained.
    pub reason: String,
    /// File-refresh revision, incremented after an acknowledged undo attempt.
    pub revision: u64,
}

/// A backend edit journal, reset only when a new prompt is accepted.
#[derive(Default)]
pub(crate) struct Journal {
    aggregate: Option<String>,
    files: BTreeMap<String, (Option<String>, Option<String>)>,
    invalid: Option<&'static str>,
    pending_acp: BTreeMap<String, Value>,
}
impl Journal {
    pub fn invalidate(&mut self) {
        self.invalid = Some("The agent did not supply a complete reversible edit record");
        self.aggregate = None;
        self.files.clear();
        self.pending_acp.clear();
    }
    pub fn aggregate(&mut self, diff: &str) {
        if self.invalid.is_some() {
            return;
        }
        if diff.len() > MAX_PATCH {
            self.invalidate();
            self.invalid = Some("The prompt's file changes exceed the undo limit");
            self.aggregate = None;
        } else {
            self.aggregate = Some(diff.into());
        }
    }
    pub fn file(&mut self, path: &str, before: Option<&str>, after: Option<&str>) {
        if self.invalid.is_some() {
            return;
        }
        if (!self.files.contains_key(path) && self.files.len() >= 64) || path.len() > 4096 {
            self.invalidate();
            self.invalid = Some("The prompt's file changes exceed the undo limit");
            return;
        }
        if let Some(entry) = self.files.get_mut(path) {
            if entry.0.as_deref() == before && entry.1.as_deref() == after {
                return;
            }
            if entry.1.as_deref() == before {
                entry.1 = after.map(str::to_owned);
            } else if entry.0.as_deref() == after {
                // Native workers can finish reporting adjacent writes out of
                // order. Join only an exact chain of before/after versions.
                entry.0 = before.map(str::to_owned);
            } else {
                self.invalidate();
                return;
            }
        } else {
            self.files.insert(
                path.into(),
                (before.map(str::to_owned), after.map(str::to_owned)),
            );
        }
        if self
            .files
            .iter()
            .map(|(p, (a, b))| {
                p.len() + a.as_ref().map_or(0, String::len) + b.as_ref().map_or(0, String::len)
            })
            .sum::<usize>()
            > MAX_PATCH / 2
        {
            self.invalidate();
            self.invalid = Some("The prompt's file changes exceed the undo limit");
        }
    }
    pub fn acp(&mut self, update: &Value) {
        if self.invalid.is_some() {
            return;
        }
        // A move needs both source and destination identities, which ACP's
        // full-file diff content does not necessarily provide.
        if update["kind"] == "move" {
            self.invalidate();
            return;
        }
        let Some(id) = update["toolCallId"].as_str() else {
            return;
        };
        // Completion updates may replace displayed content with a text summary.
        // Retain the last full-file diffs independently of that presentation.
        if let Some(content) = update.get("content").filter(|content| {
            content
                .as_array()
                .is_some_and(|parts| parts.iter().any(|p| p["type"] == "diff"))
        }) {
            if content.to_string().len() > MAX_PATCH
                || (!self.pending_acp.contains_key(id) && self.pending_acp.len() >= 64)
            {
                self.invalidate();
                return;
            }
            self.pending_acp.insert(id.into(), content.clone());
            if self
                .pending_acp
                .values()
                .map(|v| v.to_string().len())
                .sum::<usize>()
                > MAX_PATCH
            {
                self.invalidate();
                return;
            }
        }
        let content = if update["status"] == "completed" {
            self.pending_acp.remove(id)
        } else if update["status"] == "failed" {
            if self.pending_acp.remove(id).is_some_and(|content| {
                content
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|p| p["type"] == "diff"))
            }) {
                self.invalidate();
            }
            None
        } else {
            None
        };
        if let Some(content) = content.as_ref().and_then(Value::as_array) {
            for part in content {
                if part["type"] != "diff" {
                    continue;
                }
                let (Some(path), Some(after)) = (part["path"].as_str(), part["newText"].as_str())
                else {
                    self.invalidate();
                    return;
                };
                if !part["oldText"].is_null() && !part["oldText"].is_string() {
                    self.invalidate();
                    return;
                }
                self.file(path, part["oldText"].as_str(), Some(after));
            }
        }
    }
    pub fn patch(&self) -> anyhow::Result<String> {
        if let Some(reason) = self.invalid {
            anyhow::bail!(reason);
        }
        anyhow::ensure!(
            self.pending_acp.is_empty(),
            "The agent left file edits unfinished; no complete undo record is available"
        );
        if let Some(diff) = self.aggregate.as_ref().filter(|diff| !diff.is_empty()) {
            anyhow::ensure!(
                self.files.is_empty(),
                "The agent reported mixed edit records that cannot safely be combined"
            );
            return Ok(diff.clone());
        }
        let mut diff = String::new();
        for (path, (before, after)) in &self.files {
            if before == after {
                continue;
            }
            let quote = |prefix: &str| serde_json::to_string(&format!("{prefix}{path}")).unwrap();
            let file_mode = if before.is_none() {
                "new file mode 100644\n"
            } else if after.is_none() {
                "deleted file mode 100644\n"
            } else {
                ""
            };
            diff.push_str(&format!(
                "diff --git {} {}\n{}--- {}\n+++ {}\n",
                quote("a/"),
                quote("b/"),
                file_mode,
                if before.is_some() {
                    quote("a/")
                } else {
                    "/dev/null".into()
                },
                if after.is_some() {
                    quote("b/")
                } else {
                    "/dev/null".into()
                }
            ));
            let count = |s: &Option<String>| s.as_ref().map_or(0, |s| s.lines().count());
            let old = count(before);
            let new = count(after);
            if old > 0 || new > 0 {
                diff.push_str(&format!(
                    "@@ -{},{} +{},{} @@\n",
                    usize::from(old > 0),
                    old,
                    usize::from(new > 0),
                    new
                ));
            }
            for (prefix, value) in [('-', before), ('+', after)] {
                if let Some(text) = value {
                    for line in text.split_inclusive('\n') {
                        diff.push(prefix);
                        diff.push_str(line);
                        if !line.ends_with('\n') {
                            diff.push_str("\n\\ No newline at end of file\n");
                        }
                    }
                }
            }
        }
        anyhow::ensure!(
            diff.len() <= MAX_PATCH,
            "The prompt's file changes exceed the undo limit"
        );
        Ok(diff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn acp_completion_only_updates_keep_the_first_source_and_latest_result() {
        let mut journal = Journal::default();
        for (id, old, new) in [
            ("one", "before\n", "middle\n"),
            ("two", "middle\n", "after\n"),
        ] {
            journal.acp(&json!({"toolCallId":id,"status":"in_progress","content":[
                {"type":"diff","path":"file.txt","oldText":old,"newText":new}]}));
            journal.acp(&json!({"toolCallId":id,"status":"completed"}));
        }
        let patch = journal.patch().unwrap();
        assert!(patch.contains("-before\n+after\n"));
        assert!(!patch.contains("middle"));
    }

    #[test]
    fn missing_or_unsupported_edit_records_never_offer_a_partial_undo() {
        let mut journal = Journal::default();
        journal.file("file.txt", Some("before"), Some("after"));
        journal.acp(&json!({"toolCallId":"move","kind":"move"}));
        assert!(journal.patch().is_err());
        journal.aggregate("small diff");
        assert!(journal.patch().is_err());
        let mut journal = Journal::default();
        journal.file("file.txt", Some("before"), Some("after"));
        journal.aggregate(&"x".repeat(MAX_PATCH + 1));
        assert!(journal.patch().is_err());
    }

    #[test]
    fn native_edits_keep_crlf_and_missing_newlines_and_ignore_empty_backend_diffs() {
        let mut journal = Journal::default();
        journal.file("file.txt", Some("before\r\nlast"), Some("after\r\nlast"));
        journal.aggregate("");
        let patch = journal.patch().unwrap();
        assert!(patch.contains("-before\r\n-last\n\\ No newline at end of file\n"));
        assert!(patch.contains("+after\r\n+last\n\\ No newline at end of file\n"));
        journal.file("file.txt", Some("after\r\nlast"), Some("before\r\nlast"));
        assert!(journal.patch().unwrap().is_empty());
    }

    #[test]
    fn out_of_order_native_edits_require_an_exact_version_chain() {
        let mut journal = Journal::default();
        journal.file("file", Some("middle"), Some("after"));
        journal.file("file", Some("before"), Some("middle"));
        let patch = journal.patch().unwrap();
        assert!(patch.contains("-before\n"));
        assert!(patch.contains("+after\n"));
        journal.file("file", Some("unrelated"), Some("another"));
        assert!(journal.patch().is_err());
    }

    #[test]
    fn incomplete_acp_edits_block_partial_undo_and_completion_summaries_keep_diffs() {
        let mut journal = Journal::default();
        journal.file("first", Some("before"), Some("after"));
        journal.acp(
            &json!({"toolCallId":"edit","status":"in_progress","content":[
            {"type":"diff","path":"second","oldText":"old","newText":"new"}]}),
        );
        assert!(journal.patch().is_err());
        journal.acp(&json!({"toolCallId":"edit","status":"completed","content":[
            {"type":"content","content":{"type":"text","text":"Saved second"}}]}));
        let patch = journal.patch().unwrap();
        assert!(patch.contains("a/first"));
        assert!(patch.contains("a/second"));
        // Replayed completion records must not discard an otherwise complete journal.
        journal.acp(&json!({"toolCallId":"edit","status":"completed","content":[
            {"type":"diff","path":"second","oldText":"old","newText":"new"}]}));
        assert_eq!(journal.patch().unwrap(), patch);
    }

    #[test]
    fn malformed_acp_sources_cannot_turn_existing_files_into_undo_deletions() {
        for part in [
            json!({"type":"diff","path":"file","oldText":7,"newText":"after"}),
            json!({"type":"diff","path":"file","oldText":"before"}),
        ] {
            let mut journal = Journal::default();
            journal.file("another", Some("before"), Some("after"));
            journal.acp(&json!({"toolCallId":"edit","status":"completed","content":[part]}));
            assert!(journal.patch().is_err());
        }
    }
}

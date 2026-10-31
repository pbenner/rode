//! Explicit backend deletions. Enumerate a bounded snapshot before a bulk mutation
//! so removing rows cannot invalidate a pagination cursor halfway through discovery.
use super::*;

const DELETE_LIMIT: usize = 4096;

pub(super) struct Deletion {
    target: Option<String>,
    protected: BTreeSet<String>,
    seen: BTreeSet<String>,
    removed: BTreeSet<String>,
    cursors: BTreeSet<String>,
    cursor: Option<String>,
    archived: bool,
    pages: usize,
    queue: VecDeque<String>,
    current: String,
    pub(super) deleted: usize,
    skipped: usize,
    checking: VecDeque<String>,
    protection_started: bool,
    protection_done: bool,
}

impl Browser {
    /// Delete one listed idle chat, or all saved chats in this captured project/backend.
    /// Bulk discovery includes unloaded pages and archived Codex chats. Open/active
    /// conversations are skipped; backend errors preserve unacknowledged rows.
    pub fn delete(&mut self, id: Option<&str>, protected: Vec<String>) -> Result<()> {
        ensure!(
            !self.busy && self.can_delete,
            "This agent cannot delete history right now"
        );
        ensure!(protected.len() <= 256, "Too many protected conversations");
        if let Some(id) = id {
            ensure!(
                self.entries.iter().any(|e| e.id == id && !e.active)
                    && !protected.iter().any(|p| p == id),
                "Select a saved conversation that is not open or active"
            );
        }
        self.deletion = Some(Deletion {
            target: id.map(str::to_owned),
            protected: protected.into_iter().collect(),
            seen: BTreeSet::new(),
            removed: BTreeSet::new(),
            cursors: BTreeSet::new(),
            cursor: None,
            archived: false,
            pages: 0,
            queue: VecDeque::new(),
            current: String::new(),
            deleted: 0,
            skipped: 0,
            checking: VecDeque::new(),
            protection_started: false,
            protection_done: false,
        });
        self.busy = true;
        self.deadline = Instant::now() + Duration::from_secs(600);
        // Codex deletes descendants too. Even an individual deletion must
        // discover active children on other pages before checking their parents.
        let result = self.delete_list();
        if let Err(error) = result {
            self.fail(&error.to_string());
            return Err(error);
        }
        Ok(())
    }

    fn delete_list(&mut self) -> Result<()> {
        let deletion = self
            .deletion
            .as_mut()
            .context("No history deletion is pending")?;
        ensure!(
            deletion.pages < 256,
            "Delete discovery exceeded 256 pages; no chats were deleted. Narrow the history in the backend first"
        );
        deletion.pages += 1;
        self.status = format!(
            "Finding saved chats to delete… {} found",
            deletion.queue.len()
        );
        let cursor = deletion.cursor.clone();
        let mut params = self.list_params(cursor);
        if self.launch.config.provider != Provider::Acp {
            params["archived"] = self.deletion.as_ref().unwrap().archived.into();
        }
        self.call(
            if self.launch.config.provider == Provider::Acp {
                "session/list"
            } else {
                "thread/list"
            },
            params,
            Call::DeleteList,
        )
    }

    pub(super) fn deleted_list(&mut self, result: &Value) -> Result<()> {
        let acp = self.launch.config.provider == Provider::Acp;
        let rows = result[if acp { "sessions" } else { "data" }]
            .as_array()
            .context("History deletion received an invalid list")?;
        ensure!(
            rows.len() <= 128,
            "History deletion page exceeds 128 rows; no chats were deleted"
        );
        let deletion = self
            .deletion
            .as_mut()
            .context("No history deletion is pending")?;
        for row in rows {
            if !in_project(row, &self.launch, acp) {
                continue;
            }
            let Some(id) = row[if acp { "sessionId" } else { "id" }]
                .as_str()
                .filter(|s| controls::identifier(s, false))
            else {
                continue;
            };
            if acp && deletion.target.as_ref().is_some_and(|target| target != id) {
                continue;
            }
            if !deletion.seen.insert(id.to_owned()) {
                continue;
            }
            ensure!(
                deletion.seen.len() <= DELETE_LIMIT,
                "Delete discovery exceeds 4096 chats; no chats were deleted. Narrow the history in the backend first"
            );
            if deletion.protected.contains(id) || row["status"]["type"] == "active" {
                deletion.protected.insert(id.into());
                deletion.skipped += 1;
            } else if deletion.target.as_ref().is_none_or(|target| target == id) {
                deletion.queue.push_back(id.into());
            }
        }
        // A single ACP delete is revalidated against fresh scoped list metadata.
        if acp && deletion.target.is_some() && !deletion.queue.is_empty() {
            return self.delete_next();
        }
        let next = cursor(result);
        ensure!(
            result["nextCursor"].is_null() || next.is_some(),
            "Invalid delete discovery cursor; no chats were deleted"
        );
        if let Some(next) = next {
            ensure!(
                deletion.cursors.insert(next.clone()),
                "Repeated delete discovery cursor; no chats were deleted"
            );
            deletion.cursor = Some(next);
            return self.delete_list();
        }
        if !acp && !deletion.archived {
            deletion.archived = true;
            deletion.cursor = None;
            deletion.cursors.clear();
            return self.delete_list();
        }
        ensure!(
            deletion.target.is_none() || !deletion.queue.is_empty(),
            "Saved conversation is no longer idle in this project; refresh history before deleting"
        );
        self.delete_next()
    }

    fn delete_next(&mut self) -> Result<()> {
        let deletion = self
            .deletion
            .as_mut()
            .context("No history deletion is pending")?;
        if self.launch.config.provider != Provider::Acp && !deletion.protection_done {
            if !deletion.protection_started {
                deletion.checking = deletion.protected.iter().cloned().collect();
                deletion.protection_started = true;
            }
            if let Some(id) = deletion.checking.pop_front() {
                deletion.current = id.clone();
                self.status = "Checking open chats and their parent conversations…".into();
                return self.call(
                    "thread/read",
                    json!({"threadId":id,"includeTurns":false}),
                    Call::DeleteProtect,
                );
            }
            let before = deletion.queue.len();
            deletion.queue.retain(|id| !deletion.protected.contains(id));
            deletion.skipped += before - deletion.queue.len();
            ensure!(
                deletion.target.is_none() || !deletion.queue.is_empty(),
                "This conversation owns an open or active child chat; close it before deleting its parent"
            );
            deletion.protection_done = true;
        }
        if let Some(id) = deletion.queue.pop_front() {
            deletion.current = id.clone();
            self.status = format!(
                "Deleting saved chats… {} deleted, {} remaining",
                deletion.deleted,
                deletion.queue.len() + 1
            );
            if self.launch.config.provider == Provider::Acp {
                self.call("session/delete", json!({"sessionId":id}), Call::Delete)
            } else {
                self.call(
                    "thread/read",
                    json!({"threadId":id,"includeTurns":false}),
                    Call::DeleteRead,
                )
            }
        } else {
            let deletion = self.deletion.take().unwrap();
            // Deletion invalidates existing pagination offsets. Refresh starts afresh.
            self.more = false;
            self.cursor = None;
            self.cursors.clear();
            let mut summary = format!("Deleted {} saved chats", deletion.deleted);
            if deletion.skipped != 0 {
                summary.push_str(&format!(
                    " · kept {} open or active chats",
                    deletion.skipped
                ));
            }
            self.deletion_result = Some(summary);
            self.entries.clear();
            self.list()
        }
    }

    pub(super) fn delete_read(&mut self, result: &Value) -> Result<()> {
        let deletion = self
            .deletion
            .as_mut()
            .context("No history deletion is pending")?;
        let thread = &result["thread"];
        ensure!(
            thread["id"] == deletion.current && in_project(thread, &self.launch, false),
            "Saved conversation changed project/provider; deletion stopped"
        );
        if thread["status"]["type"] == "active" {
            ensure!(
                deletion.target.is_none(),
                "Conversation has an active turn; finish it before deleting"
            );
            deletion.skipped += 1;
            return self.delete_next();
        }
        let id = deletion.current.clone();
        self.call("thread/delete", json!({"threadId":id}), Call::Delete)
    }

    pub(super) fn delete_protect(&mut self, result: &Value) -> Result<()> {
        let deletion = self
            .deletion
            .as_mut()
            .context("No history deletion is pending")?;
        let thread = &result["thread"];
        ensure!(
            thread["id"] == deletion.current,
            "Cannot verify an open conversation before deletion"
        );
        if let Some(parent) = thread["parentThreadId"]
            .as_str()
            .filter(|id| !id.is_empty())
        {
            ensure!(
                controls::identifier(parent, false),
                "Invalid parent conversation identity"
            );
            if deletion.protected.insert(parent.into()) {
                ensure!(
                    deletion.protected.len() <= DELETE_LIMIT,
                    "Too many related open conversations; deletion stopped"
                );
                deletion.checking.push_back(parent.into());
            }
        }
        self.delete_next()
    }

    pub(super) fn deleted(&mut self) -> Result<()> {
        let deletion = self
            .deletion
            .as_mut()
            .context("No history deletion is pending")?;
        self.entries.retain(|entry| entry.id != deletion.current);
        if deletion.removed.insert(deletion.current.clone()) {
            deletion.deleted += 1;
        }
        self.delete_next()
    }

    pub(super) fn deleted_notification(&mut self, params: &Value) {
        if let Some(id) = params["threadId"].as_str() {
            self.entries.retain(|entry| entry.id != id);
            if let Some(deletion) = &mut self.deletion {
                // Codex also deletes descendants. Do not subsequently read/delete
                // those vanished children from the original bulk snapshot.
                deletion.queue.retain(|queued| queued != id);
                if deletion.seen.contains(id) && deletion.removed.insert(id.into()) {
                    deletion.deleted += 1;
                }
            }
        }
    }

    /// A parent deletion can remove the child whose read/delete is already in
    /// flight. Consume its response before advancing, preserving one pending RPC.
    pub(super) fn deleted_response(&mut self, call: Call) -> Result<bool> {
        if matches!(call, Call::DeleteRead | Call::Delete)
            && self
                .deletion
                .as_ref()
                .is_some_and(|d| d.removed.contains(&d.current))
        {
            self.delete_next()?;
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn delete_error(&mut self, call: Call, error: &Value) -> Result<bool> {
        if !matches!(
            call,
            Call::Delete | Call::DeleteList | Call::DeleteRead | Call::DeleteProtect
        ) {
            return Ok(false);
        }
        if matches!(call, Call::Delete)
            && self.launch.config.provider == Provider::Acp
            && error["code"] == -32002
        {
            self.deleted()?;
            return Ok(true);
        }
        if matches!(call, Call::Delete) && error["code"] == -32601 {
            let deletion = self.deletion.take().unwrap();
            self.can_delete = false;
            self.busy = false;
            self.status = format!(
                "This agent does not support deleting history. Update the backend to enable it. Deleted {} chats before stopping.",
                deletion.deleted
            );
            return Ok(true);
        }
        Ok(false)
    }
}

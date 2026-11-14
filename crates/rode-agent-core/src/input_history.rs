//! Prompt recall has its own budget so replayed tools cannot evict user input.
use crate::{personality, protocol::clip};
use std::collections::VecDeque;

/// Chronological prompt recall with independent entry/encoded-byte budgets.
/// Newer prompts survive trimming; message IDs merge streamed chunks without merging duplicate submissions.
#[derive(Default)]
pub(crate) struct History {
    entries: VecDeque<Entry>,
}

struct Entry {
    id: String,
    text: String,
    budget: usize,
}

impl Entry {
    fn new(id: &str, text: &str) -> Option<Self> {
        let text = clip(personality::preview_text(text), crate::PROMPT_LIMIT);
        if text.trim().is_empty() {
            return None;
        }
        let id = clip(id, 512);
        // Control characters can expand sixfold in JSON. Count encoded text,
        // array punctuation and local IDs so recall cannot inflate IPC frames
        // past the transport budget even when its raw strings are small.
        let budget = serde_json::to_vec(&text)
            .expect("serializing a string cannot fail")
            .len()
            + id.len()
            + 3;
        Some(Self { id, text, budget })
    }
}

impl History {
    /// Preserve order and duplicate submissions; only chunks of the same message merge.
    pub fn remember(&mut self, id: &str, text: &str) {
        let Some(entry) = Entry::new(id, text) else {
            return;
        };
        if let Some(previous) = self
            .entries
            .back_mut()
            .filter(|previous| previous.id == entry.id)
        {
            *previous = entry;
        } else {
            self.entries.push_back(entry);
        }
        self.trim();
    }

    /// Codex pages arrive newest first. Add an older prompt at the front,
    /// preserving newer prompts if the independent recall budget is exhausted.
    pub fn prepend(&mut self, id: &str, text: &str) -> bool {
        let Some(entry) = Entry::new(id, text) else {
            return true;
        };
        self.entries.push_front(entry);
        let before = self.entries.len();
        self.trim();
        self.entries.len() == before
    }

    fn trim(&mut self) {
        while self.entries.len() > 128
            || self.entries.iter().map(|entry| entry.budget).sum::<usize>() > 512 * 1024
        {
            self.entries.pop_front();
        }
    }

    /// Borrow retained prompt text in chronological order for the controller's history projection.
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.text.as_str())
    }

    /// Desktop snapshots need no backend message IDs; order is sufficient for recall.
    pub fn from_texts(texts: impl Iterator<Item = String>) -> Self {
        let mut history = Self::default();
        for (index, text) in texts.enumerate() {
            history.remember(&index.to_string(), &text);
        }
        history
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_bounds_saved_prompts_without_losing_duplicates_or_streaming_chunks() {
        let mut history = History::default();
        history.remember("first", "same prompt");
        history.remember("second", "same");
        history.remember("second", "same prompt");
        assert_eq!(
            history.texts().collect::<Vec<_>>(),
            ["same prompt", "same prompt"]
        );
        assert!(history.prepend("oldest", "an older prompt"));
        assert_eq!(history.texts().next(), Some("an older prompt"));
        for n in 0..200 {
            history.remember(&n.to_string(), &"🦀".repeat(20_000));
        }
        assert!(history.entries.len() <= 128);
        assert!(
            history
                .entries
                .iter()
                .map(|entry| entry.id.len() + entry.text.len())
                .sum::<usize>()
                <= 512 * 1024
        );
        assert!(
            history
                .texts()
                .all(|text| text.len() <= crate::PROMPT_LIMIT && text.ends_with("[… truncated]"))
        );
        let before: Vec<_> = history.texts().map(str::to_owned).collect();
        assert!(!history.prepend("too-old", &"🦀".repeat(20_000)));
        assert_eq!(history.texts().collect::<Vec<_>>(), before);
    }

    #[test]
    fn escaped_prompts_fit_the_serialized_history_budget() {
        let mut history = History::default();
        let text = "\0".repeat(crate::PROMPT_LIMIT);
        for n in 0..128 {
            history.remember(&n.to_string(), &text);
        }
        let prompts: Vec<_> = history.texts().collect();
        assert!(!prompts.is_empty());
        assert!(serde_json::to_vec(&prompts).unwrap().len() <= 512 * 1024);
    }
}

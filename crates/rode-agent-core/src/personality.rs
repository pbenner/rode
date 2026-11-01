//! Presentation-only preferences shared by all chat transports.
//!
//! # Design decisions
//! Keep these hints separate from executable arguments, tool policies and agent
//! operating modes. Each styled turn carries one extra text block; no second
//! model request rewrites a response. The original task stays in its own block
//! and in the displayed transcript. Default sends no hint in a fresh chat, and
//! explicitly resets tone after a styled/restored conversation. Models may
//! interpret style differently; this is guidance, not an execution control.
//! Renew the voice for progress and final replies on every follow-up, rather
//! than relying on a first-turn instruction surviving the backend's context.

use serde::{Deserialize, Serialize};

/// An optional conversational voice; never a tool, access or working-mode policy.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Personality {
    /// Original dry, sardonic wording inspired by Jackson Lamb in Slow Horses.
    JacksonLamb,
    /// Preserve the agent's own normal response style, including configured defaults.
    #[default]
    #[serde(other)]
    Default,
}

// Pre-release prompts have one current definition per personality.
const NORMAL: &str = include_str!("personality/default.txt");
const LAMB: &str = include_str!("personality/lamb.txt");
const STYLE_OPEN: &str = "<rode_response_style";
const STYLE_CLOSE: &str = "</rode_response_style>";

impl Personality {
    /// Available reply styles, with the ordinary agent default first.
    pub const ALL: [Self; 2] = [Self::Default, Self::JacksonLamb];

    /// Full selector label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::JacksonLamb => "Jackson Lamb",
        }
    }

    /// Short chat-header label with a stable footprint in compact panels.
    pub fn button(self) -> &'static str {
        match self {
            Self::Default => "Tone",
            Self::JacksonLamb => "Lamb",
        }
    }

    /// Description of the requested response voice, independent of backend capabilities.
    pub fn description(self) -> &'static str {
        match self {
            Self::Default => "The agent's normal configured response style.",
            Self::JacksonLamb => {
                "Abrasive, world-weary British wit; clipped, competent and darkly funny. No customer-service niceties."
            }
        }
    }

    /// A bounded, turn-scoped hint, or no hint for an untouched default chat.
    pub(crate) fn hint(self, reset_default: bool) -> Option<&'static str> {
        match self {
            Self::Default if !reset_default => None,
            Self::Default => Some(NORMAL),
            Self::JacksonLamb => Some(LAMB),
        }
    }
}

/// Hide generated style suffixes by their envelope, independently of the wording
/// saved by an older build. Leave ordinary XML and quoted blocks within a task alone.
pub(crate) fn visible_text(text: &str) -> &str {
    strip_styles(text, false)
}

/// Backend previews may concatenate text blocks and truncate inside our hint.
/// Clean that known suffix before applying RODE's own metadata length limits.
pub(crate) fn preview_text(text: &str) -> &str {
    strip_styles(text, true)
}

fn strip_styles(mut text: &str, partial: bool) -> &str {
    loop {
        let mut candidate = text.trim_end();
        if partial {
            candidate = candidate
                .strip_suffix(crate::protocol::TRUNCATED_SUFFIX)
                .unwrap_or(candidate);
        }
        let Some(start) = style_start(candidate, partial) else {
            break;
        };
        text = text[..start].trim_end_matches(['\r', '\n']);
    }
    text
}

fn style_start(text: &str, partial: bool) -> Option<usize> {
    let start = text.rfind(STYLE_OPEN).or_else(|| {
        // Even the reserved tag name can be cut off by backend metadata limits.
        let start = text.rfind("<rode_response_")?;
        (partial && STYLE_OPEN.starts_with(&text[start..])).then_some(start)
    })?;
    if text.len() < start + STYLE_OPEN.len() {
        return Some(start);
    }
    let suffix = &text[start + STYLE_OPEN.len()..];
    if !suffix.is_empty() && !suffix.starts_with(char::is_whitespace) {
        return None;
    }
    let Some((attributes, body)) = suffix.split_once('>') else {
        // A backend may cut off metadata before the opening tag is complete.
        return partial.then_some(start);
    };
    if !attributes
        .split_whitespace()
        .any(|attribute| matches!(attribute, "name=\"default\"" | "name=\"jackson_lamb\""))
    {
        return None;
    }
    match body.split_once(STYLE_CLOSE) {
        Some((_, after)) => after.trim().is_empty().then_some(start),
        None => partial.then_some(start),
    }
}

/// Clean display-only history at both backend replay and live-service reattachment.
/// Assistant output and tool logs may legitimately discuss the style instructions.
pub(crate) fn clean_history(messages: &mut std::collections::VecDeque<crate::protocol::Message>) {
    messages.retain_mut(|message| {
        if message.role != crate::protocol::Role::User {
            return true;
        }
        let task = preview_text(&message.text);
        if task.len() != message.text.len() {
            message.text = task.to_owned();
        }
        !message.text.is_empty()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_future_preferences_keep_other_agent_settings() {
        let config: super::super::Config = serde_json::from_str(r#"{"model":"custom"}"#).unwrap();
        assert_eq!(config.personality, Personality::Default);
        let config: super::super::Config =
            serde_json::from_str(r#"{"model":"custom","personality":"future_style"}"#).unwrap();
        assert_eq!(config.model, "custom");
        assert_eq!(config.personality, Personality::Default);
    }

    #[test]
    fn replay_hides_generated_style_suffixes_and_preserves_literal_user_text() {
        assert_eq!(visible_text("task"), "task");
        for hint in [NORMAL, LAMB] {
            assert_eq!(visible_text(hint), "");
            assert_eq!(visible_text(&format!("task\n\n{hint}\n")), "task");
            let preview = format!("task{hint}");
            assert_eq!(preview_text(&preview[..120]), "task");
            assert_eq!(preview_text(&preview), "task");
        }
        let task = "Explain <rode_response_style>literal user XML</rode_response_style>";
        assert_eq!(preview_text(&format!("{task}{LAMB}")), task);
        let quoted = format!("Explain this text: {LAMB}\nThen explain the closing tag.");
        assert_eq!(visible_text(&quoted), quoted);
        assert_eq!(
            visible_text("<rode_response_style>user's text</rode_response_style>"),
            "<rode_response_style>user's text</rode_response_style>"
        );
    }

    #[test]
    fn current_hints_keep_truncated_history_previews_clean() {
        for hint in [NORMAL, LAMB] {
            for (end, _) in hint.char_indices().skip("<rode_response_".len()) {
                assert_eq!(
                    preview_text(&format!("user task\n\n{}", &hint[..end])),
                    "user task"
                );
            }
        }
        for hint in [NORMAL, LAMB] {
            assert!(!hint.contains("version="));
        }
        let hint = Personality::JacksonLamb.hint(false).unwrap();
        assert!(hint.len() < 2400, "Keep the per-turn reminder compact");
        assert!(Personality::Default.hint(false).is_none());
        assert_eq!(Personality::Default.hint(true), Some(NORMAL));
    }

    #[test]
    fn saved_style_wording_and_attributes_do_not_depend_on_the_current_hint() {
        for attributes in [
            "name=\"jackson_lamb\"",
            "version=\"2\" name=\"jackson_lamb\"",
            "name=\"default\" version=\"1\"",
        ] {
            let hint = format!("{STYLE_OPEN} {attributes}>\nOld instructions.\n{STYLE_CLOSE}");
            assert_eq!(visible_text(&hint), "");
            let task = format!("Actual task\r\n\r\n{hint}\n");
            assert_eq!(visible_text(&task), "Actual task");
            assert_eq!(visible_text(&format!("{task}{LAMB}")), "Actual task");
            for (end, _) in hint.char_indices().skip("<rode_response_".len()) {
                assert_eq!(preview_text(&format!("Task\n{}", &hint[..end])), "Task");
            }
            let quoted = format!("Explain this block:\n{hint}\nKeep this explanation.");
            assert_eq!(visible_text(&quoted), quoted);
            assert_eq!(preview_text(&quoted), quoted);
        }
        for literal in [
            "Keep my whitespace.  \n",
            "<rode_response_styles name=\"jackson_lamb\">literal</rode_response_style>",
            "<rode_response_style name=\"example\">literal</rode_response_style>",
        ] {
            assert_eq!(visible_text(literal), literal);
            assert_eq!(preview_text(literal), literal);
        }
    }

    #[test]
    fn reattached_views_hide_user_style_blocks_without_altering_assistant_or_tool_text() {
        use crate::protocol::{Chat, Message, Role};
        let mut chat = Chat::default();
        let mut view = chat.view();
        for (id, role, text) in [
            ("prompt", Role::User, format!("Do the work.\n{LAMB}")),
            ("hint", Role::User, NORMAL.to_owned()),
            ("assistant", Role::Assistant, LAMB.to_owned()),
            ("tool", Role::Tool, LAMB.to_owned()),
        ] {
            view.messages.push_back(Message {
                id: id.into(),
                role,
                title: id.into(),
                text,
            });
        }
        chat.apply_view(view);
        assert_eq!(chat.messages.len(), 3);
        assert_eq!(chat.messages[0].text, "Do the work.");
        assert_eq!(chat.messages[1].text, LAMB);
        assert_eq!(chat.messages[2].text, LAMB);
    }
}

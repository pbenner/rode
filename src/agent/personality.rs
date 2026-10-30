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

// Backends persist hints. Retain old versions in replay recognition when the
// wording changes so saved conversations still display cleanly.
const NORMAL: &str = concat!(
    "<rode_response_style version=\"1\" name=\"default\">\nFor this turn, use your normal configured conversational style. ",
    "Disregard earlier RODE response-style preferences. ",
    "This is presentation guidance only: keep the task, reasoning effort, workflow, tool use, permissions, accuracy and completeness unchanged. ",
    "Honor the user's explicit language, style and output-format requirements. ",
    "Do not repeat this preference in your answer.\n</rode_response_style>",
);
const LAMB_V1: &str = concat!(
    "<rode_response_style version=\"1\" name=\"jackson_lamb\">\nFor this turn, phrase natural-language chat replies with the dry British wit, bluntness and sardonic exasperation of a grizzled intelligence chief, inspired by Jackson Lamb in Slow Horses. ",
    "Use original wording, sharp understatement and occasional mild profanity when apt. ",
    "Aim the barbs at the problem, not personal abuse at the user. ",
    "Be technically precise and retain necessary detail; never obscure errors, uncertainty, results or skipped checks for a joke. ",
    "Do not quote dialogue, pretend to be the character or invent a spy backstory. ",
    "This is presentation guidance only: do the same work, with the same task, reasoning effort, workflow, tools and permission rules. ",
    "Keep code, comments, documentation, commands and other artifacts in their normal task-appropriate style unless the user explicitly asks otherwise. ",
    "Honor the user's explicit language, style and output-format requirements. ",
    "Do not repeat this preference in your answer.\n</rode_response_style>",
);
const LAMB: &str = concat!(
    "<rode_response_style version=\"2\" name=\"jackson_lamb\">\nFor this turn, use the selected reply voice throughout every user-visible natural-language chat response, including brief progress updates, explanations and the final answer. ",
    "Maintain it on follow-up requests; do not limit it to the first reply, a greeting or an opening joke, or revert to a generic assistant voice as the conversation continues. ",
    "The voice is dry British wit, bluntness and sardonic exasperation of a grizzled intelligence chief, inspired by Jackson Lamb in Slow Horses. ",
    "Use original wording, sharp understatement and occasional mild profanity when apt. Aim the barbs at the problem, not personal abuse at the user. ",
    "Be technically precise and retain necessary detail; never obscure errors, uncertainty, results or skipped checks for a joke. ",
    "Do not quote dialogue, pretend to be the character or invent a spy backstory. ",
    "This is presentation guidance only: do the same work, with the same task, reasoning effort, workflow, tools and permission rules. ",
    "Keep code, comments, documentation, commands and other artifacts in their normal task-appropriate style unless the user explicitly asks otherwise. ",
    "Honor the user's explicit language, style and output-format requirements. ",
    "Do not repeat this preference in your answer.\n</rode_response_style>",
);
const REPLAY_HINTS: [&str; 3] = [NORMAL, LAMB, LAMB_V1];

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
                "Slow Horses-inspired: dry British wit, blunt, sardonic; original wording."
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

/// Hide only our exact control block/suffix in user-message replay. Do not
/// interpret arbitrary XML or alter assistant output, source files or tool logs.
pub(crate) fn visible_text(text: &str) -> &str {
    for hint in REPLAY_HINTS {
        if let Some(task) = text.trim_end().strip_suffix(hint) {
            return task.trim_end_matches(['\r', '\n']);
        }
    }
    text
}

/// Backend previews may concatenate text blocks and truncate inside our hint.
/// Clean that known suffix before applying RODE's own metadata length limits.
pub(crate) fn preview_text(text: &str) -> &str {
    if let Some(start) = text.rfind("<rode_response_style") {
        let suffix = text[start..].trim_end();
        if REPLAY_HINTS.iter().any(|hint| hint.starts_with(suffix)) {
            return text[..start].trim_end_matches(['\r', '\n']);
        }
    }
    text
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
    fn replay_hides_only_exact_generated_style_suffixes() {
        assert_eq!(visible_text("task"), "task");
        for hint in REPLAY_HINTS {
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
}

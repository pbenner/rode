//! Bounded, self-contained user attachments. Payloads cross host boundaries;
//! desktop paths and encoded image data never enter the displayed transcript.
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Maximum number of files submitted with one prompt.
pub const MAX_ATTACHMENTS: usize = 8;
/// Maximum encoded image or document content per attachment.
pub const CONTENT_LIMIT: usize = 1024 * 1024;
/// Maximum serialized attachment list, leaving room for prompt/configuration in transport frames.
pub const TOTAL_LIMIT: usize = 2 * 1024 * 1024;

/// Immutable file contents captured when the user attaches a file.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    /// Normalized raster image, sent as native image content.
    Image {
        /// Display filename, without a desktop path.
        name: String,
        /// Supported normalized image media type.
        mime_type: String,
        /// Standard base64-encoded image bytes.
        data: String,
    },
    /// UTF-8 document or text extracted from a PDF.
    Document {
        /// Original display filename.
        name: String,
        /// Complete extracted text, never silently truncated.
        text: String,
    },
}
impl Attachment {
    /// Original filename used in the composer and transcript.
    pub fn name(&self) -> &str {
        match self {
            Self::Image { name, .. } | Self::Document { name, .. } => name,
        }
    }
    /// Whether this attachment needs image input support from the agent.
    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.name().is_empty()
                && self.name().len() <= 255
                && !self.name().chars().any(char::is_control),
            "Invalid attachment filename"
        );
        match self {
            Self::Image {
                mime_type, data, ..
            } => {
                ensure!(
                    data.len() <= CONTENT_LIMIT,
                    "Attached image exceeds 1 MiB of encoded data"
                );
                let bytes = STANDARD.decode(data)?;
                ensure!(
                    match mime_type.as_str() {
                        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                        "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
                        _ => false,
                    },
                    "Unsupported attachment image type"
                );
            }
            Self::Document { text, .. } => ensure!(
                !text.trim().is_empty() && text.len() <= CONTENT_LIMIT && !text.contains('\0'),
                "Attached documents must contain 1 byte–1 MiB of text"
            ),
        }
        Ok(())
    }

    /// Encode validated captured content as ACP or Codex prompt blocks; image bytes
    /// remain portable and document text is labeled with its original display filename.
    pub(crate) fn blocks(&self, acp: bool) -> Vec<Value> {
        match self {
            Self::Image {
                name,
                mime_type,
                data,
            } => vec![
                json!({"type":"text","text":format!("Attached image: {name}")}),
                if acp {
                    json!({"type":"image","mimeType":mime_type,"data":data})
                } else {
                    json!({"type":"image","url":format!("data:{mime_type};base64,{data}")})
                },
            ],
            Self::Document { name, text } => {
                vec![json!({"type":"text","text":format!("Attached document: {name}\n\n{text}")})]
            }
        }
    }
}

/// Validate a complete submission before accepting it or starting a process.
pub fn validate(text: &str, attachments: &[Attachment]) -> Result<()> {
    ensure!(
        !text.trim().is_empty() || !attachments.is_empty(),
        "Enter a message or attach a file."
    );
    ensure!(
        text.len() <= crate::PROMPT_LIMIT,
        "Agent messages are limited to 32 KiB."
    );
    ensure!(
        attachments.len() <= MAX_ATTACHMENTS,
        "Attach at most 8 files per message."
    );
    for attachment in attachments {
        attachment.validate()?;
    }
    ensure!(
        serde_json::to_vec(attachments)?.len() <= TOTAL_LIMIT,
        "Attachments exceed 2 MiB per message. Remove a file or send it separately."
    );
    Ok(())
}

/// Human-readable submission summary, excluding document contents and image payloads.
pub fn summary(text: &str, attachments: &[Attachment]) -> String {
    if attachments.is_empty() {
        return text.into();
    }
    format!(
        "{}Attachments: {}",
        if text.is_empty() {
            String::new()
        } else {
            format!("{text}\n\n")
        },
        attachments
            .iter()
            .map(Attachment::name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_or_oversized_payloads_and_keeps_transcripts_small() {
        let doc = Attachment::Document {
            name: "notes.md".into(),
            text: "private document contents".into(),
        };
        assert!(validate("", &[]).is_err());
        assert!(validate("", std::slice::from_ref(&doc)).is_ok());
        assert!(validate("", &vec![doc.clone(); 9]).is_err());
        assert!(
            !summary("Read this", std::slice::from_ref(&doc)).contains("private document contents")
        );
        assert_eq!(
            summary("Read this", &[doc]),
            "Read this\n\nAttachments: notes.md"
        );
        let doc = Attachment::Document {
            name: "large.txt".into(),
            text: "a".repeat(CONTENT_LIMIT + 1),
        };
        assert!(validate("", &[doc]).is_err());
        let doc = Attachment::Document {
            name: "escaped.txt".into(),
            text: "\n".repeat(CONTENT_LIMIT),
        };
        assert!(validate("", &[doc]).is_err());
        let image = Attachment::Image {
            name: "image.png".into(),
            mime_type: "image/png".into(),
            data: "invalid base64".into(),
        };
        assert!(validate("", &[image]).is_err());
    }
}

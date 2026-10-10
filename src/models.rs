//! Data models for the ntfy JSON wire format.
//!
//! These mirror the fields documented at <https://docs.ntfy.sh/subscribe/api/>.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

/// A single ntfy message / event as delivered by the `/json` or `/sse` endpoints.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NtfyMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub time: i64,
    #[serde(default)]
    pub expires: Option<i64>,
    /// One of `open`, `keepalive`, `message` or `poll_request`.
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub click: Option<String>,
    #[serde(default)]
    pub actions: Vec<NtfyAction>,
    #[serde(default)]
    pub attachment: Option<NtfyAttachment>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
}

impl NtfyMessage {
    /// Whether this event carries an actual notification payload.
    pub fn is_message(&self) -> bool {
        self.event.is_empty() || self.event == "message" || self.event == "poll_request"
    }

    /// ntfy priority clamped to the documented 1..=5 range.
    pub fn priority(&self) -> i32 {
        self.priority.unwrap_or(3).clamp(1, 5)
    }

    pub fn text(&self) -> String {
        self.message.clone().unwrap_or_default()
    }

    pub fn display_title(&self) -> String {
        if let Some(t) = &self.title {
            if !t.trim().is_empty() {
                return t.clone();
            }
        }
        if !self.topic.is_empty() {
            return format!("ntfy · {}", self.topic);
        }
        "ntfy".to_string()
    }

    /// Server side timestamp converted to the local timezone.
    pub fn received_at(&self) -> DateTime<Local> {
        if self.time > 0 {
            DateTime::from_timestamp(self.time, 0)
                .map(|dt| dt.with_timezone(&Local))
                .unwrap_or_else(Local::now)
        } else {
            Local::now()
        }
    }
}

/// An action button attached to a notification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NtfyAction {
    /// `view`, `broadcast` or `http`.
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub clear: Option<bool>,
}

/// An attachment attached to a notification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NtfyAttachment {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(rename = "type", default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub expires: Option<i64>,
}

impl NtfyAttachment {
    /// Best effort guess whether the attachment is an image we can preview.
    pub fn is_image(&self) -> bool {
        if let Some(mime) = &self.mime {
            if mime.to_ascii_lowercase().starts_with("image/") {
                return true;
            }
        }
        let name = self.name.to_ascii_lowercase();
        [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico"]
            .iter()
            .any(|ext| name.ends_with(ext))
    }
}

use std::io::Write as _;

use super::{Event, EventSubscriber};

// ── WebhookSubscriber ─────────────────────────────────────────────────────────

/// Fires-and-forgets an HTTP POST with the serialized event JSON to a URL.
/// Uses `reqwest` in a `tokio::spawn` task so it never blocks the engine.
pub struct WebhookSubscriber {
    url: String,
}

impl WebhookSubscriber {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

impl EventSubscriber for WebhookSubscriber {
    fn on_event(&self, event: &Event) {
        let url = self.url.clone();
        let body = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "WebhookSubscriber: failed to serialize event");
                return;
            }
        };

        // Fire and forget — spawn a task that doesn't block the caller
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            if let Err(e) = client
                .post(&url)
                .header("Content-Type", "application/json")
                .body(body)
                .send()
                .await
            {
                tracing::warn!(url = %url, error = %e, "WebhookSubscriber: HTTP POST failed");
            }
        });
    }
}

// ── FileSubscriber ────────────────────────────────────────────────────────────

/// Appends each event as a single JSON line (JSONL) to a file.
/// Each call opens the file in append mode, writes, and closes it — this is
/// intentionally simple and robust (no background thread needed).
pub struct FileSubscriber {
    path: String,
}

impl FileSubscriber {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

impl EventSubscriber for FileSubscriber {
    fn on_event(&self, event: &Event) {
        let line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "FileSubscriber: failed to serialize event");
                return;
            }
        };

        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut file) => {
                if let Err(e) = writeln!(file, "{}", line) {
                    tracing::warn!(path = %self.path, error = %e, "FileSubscriber: write failed");
                }
            }
            Err(e) => {
                tracing::warn!(path = %self.path, error = %e, "FileSubscriber: open failed");
            }
        }
    }
}

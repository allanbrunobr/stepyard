use std::io::Write as _;
use std::sync::Mutex;

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

// ── DashboardSubscriber ──────────────────────────────────────────────────────

/// Accumulated state for a single step being tracked
#[derive(Debug, Clone, serde::Serialize)]
struct TrackedStep {
    step_name: String,
    step_type: String,
    status: String,
    duration_ms: Option<u64>,
    tokens_in: Option<u64>,
    tokens_out: Option<u64>,
    sandboxed: bool,
    error: Option<String>,
}

/// Internal mutable state for the dashboard subscriber
struct DashboardState {
    steps: Vec<TrackedStep>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Track in-flight steps by name to correlate start → complete/fail
    pending_steps: std::collections::HashMap<String, String>,
    /// Handle to the final POST request so we can await it before process exit
    send_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Collects step-level events during workflow execution and sends a complete
/// payload to the Minion Dashboard API (`POST /api/events`) when the workflow
/// finishes.
///
/// Unlike WebhookSubscriber (which fires per-event), this subscriber batches
/// everything into the format the Dashboard API expects: one request per
/// workflow run with all steps included.
pub struct DashboardSubscriber {
    url: String,
    secret: Option<String>,
    run_id: String,
    workflow_name: String,
    target: String,
    repo: Option<String>,
    user_name: String,
    sandbox_mode: String,
    state: Mutex<DashboardState>,
}

impl DashboardSubscriber {
    pub fn new(
        url: impl Into<String>,
        secret: Option<String>,
        workflow_name: impl Into<String>,
        target: impl Into<String>,
        repo: Option<String>,
        user_name: impl Into<String>,
        sandbox_mode: impl Into<String>,
    ) -> Self {
        Self {
            url: url.into(),
            secret,
            run_id: uuid::Uuid::new_v4().to_string(),
            workflow_name: workflow_name.into(),
            target: target.into(),
            repo,
            user_name: user_name.into(),
            sandbox_mode: sandbox_mode.into(),
            state: Mutex::new(DashboardState {
                steps: Vec::new(),
                started_at: None,
                pending_steps: std::collections::HashMap::new(),
                send_handle: None,
            }),
        }
    }

    /// Build the complete payload for POST /api/events
    fn build_payload(&self, duration_ms: u64, finished_at: chrono::DateTime<chrono::Utc>) -> serde_json::Value {
        let state = self.state.lock().unwrap();
        let total_tokens: u64 = state
            .steps
            .iter()
            .map(|s| s.tokens_in.unwrap_or(0) + s.tokens_out.unwrap_or(0))
            .sum();

        let has_failure = state.steps.iter().any(|s| s.status == "failed");
        let status = if has_failure { "failed" } else { "success" };

        // Omit null fields instead of sending null (Zod .nullish() handles both)
        let mut payload = serde_json::json!({
            "run_id": self.run_id,
            "user_name": self.user_name,
            "workflow": self.workflow_name,
            "status": status,
            "duration_ms": duration_ms,
            "total_tokens": total_tokens,
            "started_at": state.started_at.map(|t| t.to_rfc3339()),
            "finished_at": finished_at.to_rfc3339(),
            "event_version": 1,
            "steps": state.steps,
        });
        // Only include optional fields when present
        if !self.target.is_empty() {
            payload["target"] = serde_json::json!(self.target);
        }
        if let Some(ref repo) = self.repo {
            payload["repo"] = serde_json::json!(repo);
        }
        payload
    }
}

impl EventSubscriber for DashboardSubscriber {
    fn on_event(&self, event: &Event) {
        match event {
            Event::WorkflowStarted { timestamp } => {
                let mut state = self.state.lock().unwrap();
                state.started_at = Some(*timestamp);
            }
            Event::StepStarted { step_name, step_type, .. } => {
                let mut state = self.state.lock().unwrap();
                state.pending_steps.insert(step_name.clone(), step_type.clone());
            }
            Event::StepCompleted { step_name, step_type, duration_ms, input_tokens, output_tokens, cost_usd: _, sandboxed, .. } => {
                let mut state = self.state.lock().unwrap();
                state.pending_steps.remove(step_name);
                state.steps.push(TrackedStep {
                    step_name: step_name.clone(),
                    step_type: step_type.clone(),
                    status: "success".to_string(),
                    duration_ms: Some(*duration_ms),
                    tokens_in: *input_tokens,
                    tokens_out: *output_tokens,
                    sandboxed: *sandboxed,
                    error: None,
                });
            }
            Event::StepFailed { step_name, step_type, error, duration_ms, sandboxed, .. } => {
                let mut state = self.state.lock().unwrap();
                state.pending_steps.remove(step_name);
                state.steps.push(TrackedStep {
                    step_name: step_name.clone(),
                    step_type: step_type.clone(),
                    status: "failed".to_string(),
                    duration_ms: Some(*duration_ms),
                    tokens_in: None,
                    tokens_out: None,
                    sandboxed: *sandboxed,
                    error: Some(error.clone()),
                });
            }
            Event::WorkflowCompleted { duration_ms, timestamp } => {
                let payload = self.build_payload(*duration_ms, *timestamp);
                let url = self.url.clone();
                let secret = self.secret.clone();

                let handle = tokio::spawn(async move {
                    let client = reqwest::Client::new();
                    let mut req = client
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .json(&payload);

                    if let Some(ref token) = secret {
                        req = req.header("Authorization", format!("Bearer {}", token));
                    }

                    match req.send().await {
                        Ok(resp) => {
                            let status = resp.status();
                            if status.is_success() {
                                tracing::info!(url = %url, status = %status, "Dashboard: event sent successfully");
                            } else {
                                let body = resp.text().await.unwrap_or_default();
                                tracing::warn!(url = %url, status = %status, body = %body, "Dashboard: API returned error");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(url = %url, error = %e, "Dashboard: HTTP POST failed");
                        }
                    }
                });
                // Store handle so flush() can await it
                let mut state = self.state.lock().unwrap();
                state.send_handle = Some(handle);
            }
            // Sandbox events — not needed for dashboard payload
            _ => {}
        }
    }
}

impl DashboardSubscriber {
    /// Wait for the pending dashboard POST to complete.
    /// Call this before process exit to ensure the event is delivered.
    pub async fn flush(&self) {
        let handle = {
            let mut state = self.state.lock().unwrap();
            state.send_handle.take()
        };
        if let Some(h) = handle {
            let _ = h.await;
        }
    }
}

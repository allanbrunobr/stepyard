use std::io::Write as _;
use std::path::Path;
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
/// payload to the Stepyard Dashboard API (`POST /api/events`) when the workflow
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
    dispatch_log_path: Option<String>,
    artifact_paths: Vec<String>,
    #[allow(dead_code)]
    sandbox_mode: String,
    state: Mutex<DashboardState>,
}

impl DashboardSubscriber {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        url: impl Into<String>,
        secret: Option<String>,
        workflow_name: impl Into<String>,
        target: impl Into<String>,
        repo: Option<String>,
        user_name: impl Into<String>,
        sandbox_mode: impl Into<String>,
        artifact_paths: Vec<String>,
    ) -> Self {
        Self {
            url: url.into(),
            secret,
            run_id: std::env::var("STEPYARD_DASHBOARD_RUN_ID")
                .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string()),
            workflow_name: workflow_name.into(),
            target: target.into(),
            repo,
            user_name: user_name.into(),
            dispatch_log_path: std::env::var("STEPYARD_DISPATCH_LOG_PATH").ok(),
            artifact_paths,
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
    fn build_payload(
        &self,
        duration_ms: u64,
        finished_at: chrono::DateTime<chrono::Utc>,
    ) -> serde_json::Value {
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
        if let Some(ref dispatch_log_path) = self.dispatch_log_path {
            payload["dispatch_log_path"] = serde_json::json!(dispatch_log_path);
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
            Event::StepStarted {
                step_name,
                step_type,
                ..
            } => {
                let mut state = self.state.lock().unwrap();
                state
                    .pending_steps
                    .insert(step_name.clone(), step_type.clone());
            }
            Event::StepCompleted {
                step_name,
                step_type,
                duration_ms,
                input_tokens,
                output_tokens,
                cost_usd: _,
                sandboxed,
                ..
            } => {
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
            Event::StepFailed {
                step_name,
                step_type,
                error,
                duration_ms,
                sandboxed,
                ..
            } => {
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
            Event::WorkflowCompleted {
                duration_ms,
                timestamp,
            } => {
                let payload = self.build_payload(*duration_ms, *timestamp);
                let url = self.url.clone();
                let secret = self.secret.clone();
                let run_id = self.run_id.clone();
                let artifact_paths = self.artifact_paths.clone();

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

                    if !artifact_paths.is_empty() {
                        upload_dashboard_artifacts(
                            &client,
                            &url,
                            secret.as_deref(),
                            &run_id,
                            &artifact_paths,
                        )
                        .await;
                    }
                });
                // Store handle so flush() can await it
                let mut state = self.state.lock().unwrap();
                state.send_handle = Some(handle);
            }
            Event::StepTimeoutFired { .. } => {
                // Story 1.3: handled explicitly so the workspace
                // `non_exhaustive_omitted_patterns` lint has a real
                // opportunity to fire if a future variant is missed.
                // Dashboard payload is emitted by the subsequent
                // StepFailed event, so nothing to accumulate here.
            }
            Event::IdleTimeoutFired { .. } => {
                // Story 5.1: handled explicitly for the same reason as
                // StepTimeoutFired. Dashboard payload remains driven by the
                // subsequent StepFailed event.
            }
            Event::SignalReceived { .. } => {
                // Story 2.3: signals surface as a subsequent StepFailed event
                // carrying TerminationReason::SignalReceived — so the
                // dashboard payload is built from that, not here. Matched
                // explicitly so `non_exhaustive_omitted_patterns` fires if a
                // future variant is forgotten.
            }
            Event::CompletionSignaled { .. } => {
                // Story 5.5: the completion signal is an audit/control-flow
                // event. The dashboard's terminal payload is still emitted by
                // WorkflowCompleted, and the matched stdout line is
                // deliberately not carried here.
            }
            Event::WorkspacePrepared { .. } => {
                // Workspace lifecycle is an audit-log concern for now. The
                // dashboard's step payload is still driven by StepStarted /
                // StepCompleted / StepFailed, so there is no state to mutate.
            }
            Event::BranchCreated { .. } => {
                // Branch creation pairs with WorkspacePrepared and does not
                // affect the dashboard step summary yet.
            }
            Event::WorkspacePruned { .. } => {
                // Startup prune is audit-log-only for now.
            }
            Event::MergeAttempted { .. } => {
                // Workspace merge lifecycle is audit-log-only for now.
            }
            Event::MergeConflict { .. } => {
                // Merge conflicts do not alter the step summary payload here.
            }
            // Sandbox events — not needed for dashboard payload
            _ => {}
        }
    }
}

fn dashboard_artifacts_url(events_url: &str, run_id: &str) -> String {
    let trimmed = events_url.trim_end_matches('/');
    let api_base = trimmed.strip_suffix("/events").unwrap_or(trimmed);
    format!("{api_base}/workflows/{run_id}/artifacts")
}

async fn upload_dashboard_artifacts(
    client: &reqwest::Client,
    events_url: &str,
    secret: Option<&str>,
    run_id: &str,
    paths: &[String],
) {
    let url = dashboard_artifacts_url(events_url, run_id);
    for raw_path in paths {
        let path = Path::new(raw_path);
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(name) if !name.is_empty() => name,
            _ => {
                tracing::warn!(path = %raw_path, "Dashboard: artifact path has no valid filename");
                continue;
            }
        };

        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(path = %raw_path, error = %e, "Dashboard: artifact read failed");
                continue;
            }
        };

        let payload = serde_json::json!({
            "name": name,
            "content_base64": base64_encode(&bytes),
            "content_type": "application/octet-stream",
        });
        let mut req = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&payload);
        if let Some(token) = secret {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    tracing::info!(url = %url, path = %raw_path, status = %status, "Dashboard: artifact uploaded");
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!(url = %url, path = %raw_path, status = %status, body = %body, "Dashboard: artifact upload rejected");
                }
            }
            Err(e) => {
                tracing::warn!(url = %url, path = %raw_path, error = %e, "Dashboard: artifact upload failed");
            }
        }
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);

        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

impl DashboardSubscriber {
    /// Wait for the pending dashboard POST to complete.
    /// Call this before process exit to ensure the event is delivered.
    #[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::{base64_encode, dashboard_artifacts_url};

    #[test]
    fn derives_artifact_url_from_events_endpoint() {
        assert_eq!(
            dashboard_artifacts_url("http://localhost:3001/api/events", "run-1"),
            "http://localhost:3001/api/workflows/run-1/artifacts"
        );
        assert_eq!(
            dashboard_artifacts_url("http://localhost:3001/api/events/", "run-1"),
            "http://localhost:3001/api/workflows/run-1/artifacts"
        );
    }

    #[test]
    fn base64_encoder_matches_standard_fixtures() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello world"), "aGVsbG8gd29ybGQ=");
    }
}

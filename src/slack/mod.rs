//! Minion Slack Bot — listens for @minion mentions and dispatches workflows
//!
//! Enable with: cargo install minion-engine --features slack
//!
//! Configuration: ~/.minion/config.toml or environment variables:
//!   SLACK_BOT_TOKEN      — xoxb-... Bot User OAuth Token
//!   SLACK_SIGNING_SECRET — from Slack App → Basic Information → Signing Secret
//!   MINION_WORKFLOWS_DIR — path to workflows/ directory (default: ./workflows)

use std::env;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::post,
    Router,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::process::Command;
use tracing::{error, info, warn};

type HmacSha256 = Hmac<Sha256>;

// ── Slack Event Types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum SlackRequest {
    #[serde(rename = "url_verification")]
    UrlVerification { challenge: String },
    #[serde(rename = "event_callback")]
    EventCallback { event: SlackEvent },
}

#[derive(Debug, Deserialize)]
struct SlackEvent {
    #[serde(rename = "type")]
    event_type: String,
    text: Option<String>,
    channel: Option<String>,
    ts: Option<String>,
    user: Option<String>,
    #[serde(default)]
    bot_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct SlackMessage {
    channel: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<String>,
}

// ── Workflow Routing ────────────────────────────────────────────────────────

struct WorkflowMatch {
    workflow: String,
    target: String,
    description: String,
}

/// Extract the trailing number from a GitHub URL (e.g. ".../pull/12" → "12")
fn extract_github_number(input: &str) -> String {
    if let Some(caps) = regex::Regex::new(r"/(\d+)/?$").unwrap().captures(input) {
        return caps[1].to_string();
    }
    input.to_string()
}

fn route_message(text: &str) -> Option<WorkflowMatch> {
    let text = text.to_lowercase();

    // Remove bot mention like <@U12345>
    let clean = regex::Regex::new(r"<@[A-Z0-9]+>")
        .unwrap()
        .replace_all(&text, "")
        .trim()
        .to_string();

    // Remove Slack URL formatting: <https://url> → https://url
    let clean = regex::Regex::new(r"<(https?://[^>|]+)(?:\|[^>]*)?>")
        .unwrap()
        .replace_all(&clean, "$1")
        .to_string();

    // fix issue #N or fix issue URL
    if let Some(caps) = regex::Regex::new(r"fix\s+issue\s+[#]?(\S+)")
        .unwrap()
        .captures(&clean)
    {
        return Some(WorkflowMatch {
            workflow: "fix-issue.yaml".to_string(),
            target: extract_github_number(&caps[1]),
            description: "Fix GitHub issue".to_string(),
        });
    }

    // review pr #N or review PR URL
    if let Some(caps) = regex::Regex::new(r"review\s+(?:pr|pull\s*request)\s+[#]?(\S+)")
        .unwrap()
        .captures(&clean)
    {
        return Some(WorkflowMatch {
            workflow: "code-review.yaml".to_string(),
            target: extract_github_number(&caps[1]),
            description: "Code review".to_string(),
        });
    }

    // security audit <repo-or-path>
    if let Some(caps) = regex::Regex::new(r"security\s+audit\s+(\S+)")
        .unwrap()
        .captures(&clean)
    {
        return Some(WorkflowMatch {
            workflow: "security-audit.yaml".to_string(),
            target: caps[1].to_string(),
            description: "Security audit".to_string(),
        });
    }

    // generate docs <repo-or-path>
    if let Some(caps) = regex::Regex::new(r"generate\s+docs?\s+(\S+)")
        .unwrap()
        .captures(&clean)
    {
        return Some(WorkflowMatch {
            workflow: "generate-docs.yaml".to_string(),
            target: caps[1].to_string(),
            description: "Generate documentation".to_string(),
        });
    }

    // fix ci <pr-url>
    if let Some(caps) = regex::Regex::new(r"fix\s+ci\s+(\S+)")
        .unwrap()
        .captures(&clean)
    {
        return Some(WorkflowMatch {
            workflow: "fix-ci.yaml".to_string(),
            target: caps[1].to_string(),
            description: "Fix CI failures".to_string(),
        });
    }

    None
}

// ── App State ───────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    bot_token: String,
    signing_secret: String,
    workflows_dir: String,
    http: reqwest::Client,
}

// ── Signature Verification ──────────────────────────────────────────────────

fn verify_slack_signature(secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let timestamp = match headers.get("x-slack-request-timestamp") {
        Some(v) => v.to_str().unwrap_or(""),
        None => return false,
    };
    let signature = match headers.get("x-slack-signature") {
        Some(v) => v.to_str().unwrap_or(""),
        None => return false,
    };

    if let Ok(ts) = timestamp.parse::<u64>() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        if now.abs_diff(ts) > 300 {
            warn!("Slack request timestamp too old");
            return false;
        }
    }

    let sig_basestring = format!("v0:{}:{}", timestamp, String::from_utf8_lossy(body));
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key");
    mac.update(sig_basestring.as_bytes());
    let expected = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

    signature == expected
}

// ── Slack API Helpers ───────────────────────────────────────────────────────

async fn post_message(state: &AppState, msg: &SlackMessage) {
    let resp = state
        .http
        .post("https://slack.com/api/chat.postMessage")
        .bearer_auth(&state.bot_token)
        .json(msg)
        .send()
        .await;

    match resp {
        Ok(r) => {
            if !r.status().is_success() {
                error!("Slack API error: {}", r.status());
            }
        }
        Err(e) => error!("Failed to post Slack message: {}", e),
    }
}

// ── Workflow Execution ──────────────────────────────────────────────────────

async fn run_workflow(state: Arc<AppState>, channel: String, thread_ts: String, wf: WorkflowMatch) {
    let workflow_path = format!("{}/{}", state.workflows_dir, wf.workflow);

    post_message(
        &state,
        &SlackMessage {
            channel: channel.clone(),
            text: format!(
                "🚀 Starting *{}* — `{}`\nTarget: `{}`\nWorkflow: `{}`",
                wf.description, wf.workflow, wf.target, workflow_path
            ),
            thread_ts: Some(thread_ts.clone()),
        },
    )
    .await;

    let minion_bin = which_minion();

    info!(
        workflow = %wf.workflow,
        target = %wf.target,
        bin = %minion_bin,
        "Launching workflow"
    );

    let enhanced_path = format!(
        "{}/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:{}",
        env::var("HOME").unwrap_or_default(),
        env::var("PATH").unwrap_or_default()
    );

    let result = Command::new(&minion_bin)
        .args(["execute", &workflow_path, "--", &wf.target])
        .env("PATH", &enhanced_path)
        .envs(std::env::vars())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    let (status_emoji, summary) = match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = if stdout.len() > 1500 {
                format!("...{}", &stdout[stdout.len() - 1500..])
            } else {
                stdout.to_string()
            };

            if output.status.success() {
                (
                    "✅",
                    format!("Workflow completed successfully!\n```\n{}\n```", combined),
                )
            } else {
                let err_tail = if stderr.len() > 1000 {
                    format!("...{}", &stderr[stderr.len() - 1000..])
                } else {
                    stderr.to_string()
                };
                (
                    "❌",
                    format!(
                        "Workflow failed (exit code {})\n```\n{}\n```\nStderr:\n```\n{}\n```",
                        output.status.code().unwrap_or(-1),
                        combined,
                        err_tail
                    ),
                )
            }
        }
        Err(e) => ("💥", format!("Failed to spawn minion: {}", e)),
    };

    post_message(
        &state,
        &SlackMessage {
            channel,
            text: format!(
                "{} *{}* finished\n{}",
                status_emoji, wf.description, summary
            ),
            thread_ts: Some(thread_ts),
        },
    )
    .await;
}

fn which_minion() -> String {
    if let Ok(home) = env::var("HOME") {
        let cargo_bin = format!("{}/.cargo/bin/minion", home);
        if std::path::Path::new(&cargo_bin).exists() {
            return cargo_bin;
        }
    }
    "minion".to_string()
}

// ── HTTP Handler ────────────────────────────────────────────────────────────

async fn slack_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !verify_slack_signature(&state.signing_secret, &headers, &body) {
        warn!("Invalid Slack signature");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let request: SlackRequest = serde_json::from_slice(&body).map_err(|e| {
        error!("Failed to parse Slack event: {}", e);
        StatusCode::BAD_REQUEST
    })?;

    match request {
        SlackRequest::UrlVerification { challenge } => {
            info!("Slack URL verification challenge received");
            Ok(Json(serde_json::json!({ "challenge": challenge })))
        }
        SlackRequest::EventCallback { event } => {
            if event.bot_id.is_some() {
                return Ok(Json(serde_json::json!({"ok": true})));
            }

            if event.event_type == "app_mention" {
                if let (Some(text), Some(channel), Some(ts)) = (event.text, event.channel, event.ts)
                {
                    info!(
                        user = ?event.user,
                        channel = %channel,
                        text = %text,
                        "Received app_mention"
                    );

                    match route_message(&text) {
                        Some(wf) => {
                            let state = Arc::clone(&state);
                            let ch = channel.clone();
                            let thread = ts.clone();
                            tokio::spawn(async move {
                                run_workflow(state, ch, thread, wf).await;
                            });
                        }
                        None => {
                            let state_ref = &*state;
                            post_message(
                                state_ref,
                                &SlackMessage {
                                    channel,
                                    text: "🤔 I didn't understand that command. Try:\n\
                                        • `@minion fix issue #10`\n\
                                        • `@minion review pr #42`\n\
                                        • `@minion security audit <repo>`\n\
                                        • `@minion generate docs <repo>`\n\
                                        • `@minion fix ci <pr-url>`"
                                        .to_string(),
                                    thread_ts: Some(ts),
                                },
                            )
                            .await;
                        }
                    }
                }
            }

            Ok(Json(serde_json::json!({"ok": true})))
        }
    }
}

async fn health() -> &'static str {
    "minion-slack ok"
}

// ── Public entry point ──────────────────────────────────────────────────────

/// Load config from ~/.minion/config.toml, falling back to env vars.
fn load_slack_config() -> (String, String, String) {
    // Try config file first
    let config_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".minion/config.toml");

    let (file_token, file_secret, file_dir) = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path).unwrap_or_default();
        let parsed: toml::Value =
            toml::from_str(&content).unwrap_or(toml::Value::Table(Default::default()));
        let slack = parsed.get("slack");
        (
            slack
                .and_then(|s| s.get("bot_token"))
                .and_then(|v| v.as_str())
                .map(String::from),
            slack
                .and_then(|s| s.get("signing_secret"))
                .and_then(|v| v.as_str())
                .map(String::from),
            parsed
                .get("core")
                .and_then(|c| c.get("workflows_dir"))
                .and_then(|v| v.as_str())
                .map(String::from),
        )
    } else {
        (None, None, None)
    };

    let token = env::var("SLACK_BOT_TOKEN")
        .ok()
        .or(file_token)
        .expect("SLACK_BOT_TOKEN must be set (env var or ~/.minion/config.toml)");

    let secret = env::var("SLACK_SIGNING_SECRET")
        .ok()
        .or(file_secret)
        .expect("SLACK_SIGNING_SECRET must be set (env var or ~/.minion/config.toml)");

    let workflows_dir = env::var("MINION_WORKFLOWS_DIR")
        .ok()
        .or(file_dir)
        .unwrap_or_else(|| "./workflows".to_string());

    (token, secret, workflows_dir)
}

/// Start the Slack bot server on the given port.
pub async fn start_server(port: u16) -> anyhow::Result<()> {
    let (bot_token, signing_secret, workflows_dir) = load_slack_config();

    info!(workflows_dir = %workflows_dir, port = port, "Starting Minion Slack Bot");

    println!();
    println!("\x1b[1m🤖 Minion Slack Bot\x1b[0m");
    println!("  Workflows: {}", workflows_dir);
    println!("  Port:      {}", port);
    println!();
    println!("\x1b[2mWaiting for Slack events... (Ctrl+C to stop)\x1b[0m");
    println!();

    let state = Arc::new(AppState {
        bot_token,
        signing_secret,
        workflows_dir,
        http: reqwest::Client::new(),
    });

    let app = Router::new()
        .route("/slack/events", post(slack_events))
        .route("/health", axum::routing::get(health))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Listening on {}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}

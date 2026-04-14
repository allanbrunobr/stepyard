//! Remote dispatch client — talks to the Dashboard API's `/api/workflows/dispatch`
//! endpoint so users can trigger workflows on a VPS without opening SSH.
//!
//! Config: `~/.minion/remote.toml`
//!
//! ```toml
//! url = "http://187.45.254.82:3001"
//! secret = "<API_SECRET>"
//! default_repo = "allanbrunobr/minion-engine"   # optional
//! ```
//!
//! Story 5.2 (Epic 5 — Remote-First Execution).

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

#[derive(Args, Debug)]
pub struct RemoteArgs {
    #[command(subcommand)]
    pub command: RemoteCommand,
}

#[derive(Subcommand, Debug)]
pub enum RemoteCommand {
    /// Dispatch a workflow to the remote engine
    Exec {
        /// Workflow name (basename without `.yaml`), e.g. `fix-issue`
        workflow: String,

        /// GitHub repo to clone inside the sandbox (OWNER/REPO). Falls back to
        /// `default_repo` in remote.toml if omitted.
        #[arg(long)]
        repo: Option<String>,

        /// Branch to check out inside the container. Forwarded as `--var branch=<value>`.
        #[arg(long)]
        branch: Option<String>,

        /// Workflow variable (repeatable). Format: `KEY=VALUE`.
        #[arg(long = "var", value_name = "KEY=VALUE")]
        vars: Vec<String>,

        /// Target argument. Passed after `--`.
        #[arg(last = true, required = true)]
        target: Vec<String>,
    },

    /// Show recent remote runs (reads `/api/workflows`)
    Status {
        /// Filter by workflow name
        #[arg(long)]
        workflow: Option<String>,

        /// Max rows to display
        #[arg(long, default_value_t = 10)]
        limit: u32,
    },

    /// Print a pointer to the dashboard UI for this run
    /// (log streaming lands in Story 5.3).
    Logs {
        /// Run UUID returned by `exec` / shown by `status`
        run_id: String,
    },
}

#[derive(Debug, Deserialize)]
struct RemoteConfig {
    url: String,
    secret: String,
    #[serde(default)]
    default_repo: Option<String>,
}

fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".minion/remote.toml")
}

fn load_config() -> Result<RemoteConfig> {
    let path = config_path();
    if !path.exists() {
        bail!(
            "Remote config not found at {}\n\n\
             Create it with the following template:\n\n\
             ─── {} ───\n\
             url = \"http://<vps-host>:3001\"\n\
             secret = \"<API_SECRET from dashboard .env>\"\n\
             # default_repo = \"OWNER/REPO\"   # optional\n",
            path.display(),
            path.display()
        );
    }
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let cfg: RemoteConfig = toml::from_str(&body)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    if cfg.url.is_empty() || cfg.secret.is_empty() {
        bail!("url and secret must be set in {}", path.display());
    }
    Ok(cfg)
}

fn parse_kv_vars(raw: &[String]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for entry in raw {
        let (k, v) = entry
            .split_once('=')
            .ok_or_else(|| anyhow!("--var expects KEY=VALUE, got `{}`", entry))?;
        if k.is_empty() {
            bail!("--var key cannot be empty in `{}`", entry);
        }
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
struct DispatchRequest<'a> {
    workflow: &'a str,
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    vars: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct DispatchResponse {
    dispatched_at: String,
    pid: u32,
    workflow: String,
    target: String,
}

pub async fn run(args: RemoteArgs) -> Result<()> {
    let cfg = load_config()?;
    match args.command {
        RemoteCommand::Exec {
            workflow,
            repo,
            branch,
            vars,
            target,
        } => exec_cmd(&cfg, workflow, repo, branch, vars, target).await,
        RemoteCommand::Status { workflow, limit } => status_cmd(&cfg, workflow, limit).await,
        RemoteCommand::Logs { run_id } => logs_cmd(&cfg, &run_id),
    }
}

async fn exec_cmd(
    cfg: &RemoteConfig,
    workflow: String,
    repo: Option<String>,
    branch: Option<String>,
    vars: Vec<String>,
    target: Vec<String>,
) -> Result<()> {
    let target_joined = target.join(" ");
    let repo = repo.or_else(|| cfg.default_repo.clone());
    let vars_map = parse_kv_vars(&vars)?;

    let body = DispatchRequest {
        workflow: &workflow,
        target: target_joined,
        repo,
        branch,
        vars: vars_map,
    };

    let url = format!("{}/api/workflows/dispatch", cfg.url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .bearer_auth(&cfg.secret)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("HTTP POST {} failed", url))?;

    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Dispatch failed ({}): {}", status, text);
    }

    let parsed: DispatchResponse = serde_json::from_str(&text)
        .with_context(|| format!("Unexpected response body: {}", text))?;

    println!(
        "✓ Dispatched `{}` (target: {})\n  pid={}  dispatched_at={}\n  track: {}/workflows",
        parsed.workflow,
        parsed.target,
        parsed.pid,
        parsed.dispatched_at,
        cfg.url.trim_end_matches('/')
    );
    Ok(())
}

#[derive(Debug, Deserialize)]
struct WorkflowListEntry {
    run_id: String,
    workflow: String,
    target: Option<String>,
    status: String,
    started_at: String,
    #[serde(default)]
    duration_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WorkflowListResponse {
    data: Vec<WorkflowListEntry>,
}

async fn status_cmd(cfg: &RemoteConfig, workflow: Option<String>, limit: u32) -> Result<()> {
    let base = cfg.url.trim_end_matches('/');
    let mut url = format!("{}/api/workflows?limit={}", base, limit);
    if let Some(w) = workflow {
        url.push_str(&format!("&workflow={}", w));
    }

    let client = reqwest::Client::new();
    let res = client
        .get(&url)
        .bearer_auth(&cfg.secret)
        .send()
        .await
        .with_context(|| format!("HTTP GET {} failed", url))?;

    if !res.status().is_success() {
        bail!("Status query failed ({}): {}", res.status(), res.text().await.unwrap_or_default());
    }

    let body: WorkflowListResponse = res.json().await.context("Invalid list response JSON")?;
    if body.data.is_empty() {
        println!("(no runs)");
        return Ok(());
    }
    println!(
        "{:<38}  {:<22}  {:<18}  {:<10}  {}",
        "RUN_ID", "WORKFLOW", "TARGET", "STATUS", "STARTED"
    );
    for entry in body.data {
        let target = entry.target.unwrap_or_default();
        let target = if target.len() > 18 { &target[..18] } else { &target };
        let workflow = if entry.workflow.len() > 22 {
            &entry.workflow[..22]
        } else {
            &entry.workflow
        };
        println!(
            "{:<38}  {:<22}  {:<18}  {:<10}  {}",
            entry.run_id, workflow, target, entry.status, entry.started_at
        );
    }
    Ok(())
}

fn logs_cmd(cfg: &RemoteConfig, run_id: &str) -> Result<()> {
    let base = cfg.url.trim_end_matches('/');
    // TODO (Story 5.3): stream SSE from /api/workflows/:run_id/logs/stream
    println!(
        "Log streaming lands in Story 5.3. For now, open the dashboard:\n  {}/workflows",
        base
    );
    println!("Run id: {}", run_id);
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kv_vars_accepts_multiple() {
        let got = parse_kv_vars(&["foo=1".into(), "bar=two".into()]).unwrap();
        assert_eq!(got.get("foo"), Some(&"1".to_string()));
        assert_eq!(got.get("bar"), Some(&"two".to_string()));
    }

    #[test]
    fn parse_kv_vars_rejects_malformed() {
        let err = parse_kv_vars(&["no-equals".into()]).unwrap_err();
        assert!(err.to_string().contains("KEY=VALUE"));
    }

    #[test]
    fn parse_kv_vars_rejects_empty_key() {
        let err = parse_kv_vars(&["=value".into()]).unwrap_err();
        assert!(err.to_string().contains("key cannot be empty"));
    }

    #[test]
    fn dispatch_request_serialises_with_repo_and_branch() {
        let req = DispatchRequest {
            workflow: "fix-issue",
            target: "42".into(),
            repo: Some("owner/repo".into()),
            branch: Some("bmad/task-1".into()),
            vars: HashMap::from([("foo".into(), "bar".into())]),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["workflow"], "fix-issue");
        assert_eq!(json["target"], "42");
        assert_eq!(json["repo"], "owner/repo");
        assert_eq!(json["branch"], "bmad/task-1");
        assert_eq!(json["vars"]["foo"], "bar");
    }

    #[test]
    fn dispatch_request_omits_optional_fields_when_none() {
        let req = DispatchRequest {
            workflow: "hello",
            target: "world".into(),
            repo: None,
            branch: None,
            vars: HashMap::new(),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("repo").is_none());
        assert!(json.get("branch").is_none());
        assert!(json.get("vars").is_none());
    }
}

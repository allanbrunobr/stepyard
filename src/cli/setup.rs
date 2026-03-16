use std::path::PathBuf;

use anyhow::Context;
use dialoguer::{Confirm, Input};

/// Config file structure (~/.minion/config.toml)
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct MinionConfig {
    #[serde(default)]
    core: CoreConfig,
    #[serde(default)]
    slack: Option<SlackConfig>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct CoreConfig {
    anthropic_api_key: Option<String>,
    workflows_dir: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
struct SlackConfig {
    bot_token: Option<String>,
    signing_secret: Option<String>,
    port: Option<u16>,
}

fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".minion")
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn load_config() -> MinionConfig {
    let path = config_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        toml::from_str(&content).unwrap_or_default()
    } else {
        MinionConfig::default()
    }
}

fn save_config(config: &MinionConfig) -> anyhow::Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create {}", dir.display()))?;
    let content = toml::to_string_pretty(config)?;
    std::fs::write(config_path(), content)?;
    Ok(())
}

pub async fn run_setup() -> anyhow::Result<()> {
    println!();
    println!("\x1b[1m🔧 Minion Engine Setup\x1b[0m");
    println!("\x1b[2m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\x1b[0m");
    println!();

    let mut config = load_config();

    // ── Step 1: Check requirements ───────────────────────────────────────
    println!("\x1b[1mStep 1/4 — Checking requirements\x1b[0m");
    println!();

    check_requirement("ANTHROPIC_API_KEY", std::env::var("ANTHROPIC_API_KEY").is_ok());
    check_requirement("gh CLI", which("gh"));
    check_requirement("Docker", which("docker"));
    println!();

    // ── Step 2: API Key ──────────────────────────────────────────────────
    println!("\x1b[1mStep 2/4 — Anthropic API Key\x1b[0m");
    println!();

    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        println!("  \x1b[32m✓\x1b[0m ANTHROPIC_API_KEY already set in environment");
    } else if config.core.anthropic_api_key.is_some() {
        println!("  \x1b[32m✓\x1b[0m ANTHROPIC_API_KEY found in ~/.minion/config.toml");
    } else {
        let set_key = Confirm::new()
            .with_prompt("  Set ANTHROPIC_API_KEY now?")
            .default(true)
            .interact()?;

        if set_key {
            let key: String = Input::new()
                .with_prompt("  ANTHROPIC_API_KEY")
                .interact_text()?;
            if !key.is_empty() {
                config.core.anthropic_api_key = Some(key);
                println!("  \x1b[32m✓\x1b[0m Saved to ~/.minion/config.toml");
            }
        } else {
            println!("  \x1b[33m⚠\x1b[0m  Skipped — set it later: export ANTHROPIC_API_KEY=\"sk-ant-...\"");
        }
    }
    println!();

    // ── Step 3: Workflows directory ──────────────────────────────────────
    println!("\x1b[1mStep 3/4 — Workflows directory\x1b[0m");
    println!();

    let default_dir = "./workflows".to_string();
    let current_dir = config
        .core
        .workflows_dir
        .clone()
        .unwrap_or_else(|| default_dir.clone());

    let wf_dir: String = Input::new()
        .with_prompt("  Workflows directory")
        .default(current_dir)
        .interact_text()?;

    config.core.workflows_dir = Some(wf_dir.clone());

    // Create directory if it doesn't exist
    let wf_path = PathBuf::from(&wf_dir);
    if !wf_path.exists() {
        let create = Confirm::new()
            .with_prompt(format!("  Directory '{}' doesn't exist. Create it?", wf_dir))
            .default(true)
            .interact()?;
        if create {
            std::fs::create_dir_all(&wf_path)?;
            println!("  \x1b[32m✓\x1b[0m Created {}", wf_dir);
        }
    } else {
        println!("  \x1b[32m✓\x1b[0m Directory exists: {}", wf_dir);
    }
    println!();

    // ── Step 4: Slack Bot ────────────────────────────────────────────────
    println!("\x1b[1mStep 4/4 — Slack Bot Integration\x1b[0m");
    println!();

    #[cfg(feature = "slack")]
    {
        let setup_slack = Confirm::new()
            .with_prompt("  Configure Slack Bot?")
            .default(false)
            .interact()?;

        if setup_slack {
            println!();
            println!("  \x1b[2mYou'll need these from https://api.slack.com/apps:\x1b[0m");
            println!("  \x1b[2m  • Bot User OAuth Token (xoxb-...)\x1b[0m");
            println!("  \x1b[2m  • Signing Secret (from Basic Information)\x1b[0m");
            println!();

            let existing = config.slack.clone().unwrap_or_default();

            let token: String = Input::new()
                .with_prompt("  SLACK_BOT_TOKEN")
                .default(existing.bot_token.unwrap_or_default())
                .interact_text()?;

            let secret: String = Input::new()
                .with_prompt("  SLACK_SIGNING_SECRET")
                .default(existing.signing_secret.unwrap_or_default())
                .interact_text()?;

            let port: u16 = Input::new()
                .with_prompt("  Bot port")
                .default(existing.port.unwrap_or(9000))
                .interact_text()?;

            config.slack = Some(SlackConfig {
                bot_token: Some(token),
                signing_secret: Some(secret),
                port: Some(port),
            });

            println!("  \x1b[32m✓\x1b[0m Slack config saved");
        }
    }

    #[cfg(not(feature = "slack"))]
    {
        println!("  \x1b[33m⚠\x1b[0m  Slack support not compiled.");
        println!("  To enable, reinstall with:");
        println!("    \x1b[1mcargo install minion-engine --features slack\x1b[0m");
    }

    println!();

    // ── Save config ──────────────────────────────────────────────────────
    save_config(&config)?;

    // ── Summary ──────────────────────────────────────────────────────────
    println!("\x1b[2m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\x1b[0m");
    println!("\x1b[32m✓ Setup complete!\x1b[0m Config saved to {}", config_path().display());
    println!();
    println!("\x1b[1mNext steps:\x1b[0m");
    println!("  minion list                                    List workflows");
    println!("  minion execute workflows/code-review.yaml -- 42  Run a workflow");

    #[cfg(feature = "slack")]
    if config.slack.is_some() {
        println!("  minion slack start                             Start Slack bot");
        println!();
        println!("\x1b[1mSlack setup:\x1b[0m");
        println!("  1. Start ngrok:  ngrok http 9000");
        println!("  2. Set Request URL in Slack App → Event Subscriptions:");
        println!("     https://<your-ngrok>.ngrok-free.app/slack/events");
        println!("  3. Subscribe to bot event: app_mention");
        println!("  4. Invite bot to channel: /invite @YourBot");
        println!("  5. Run: minion slack start");
    }

    println!();

    Ok(())
}

fn check_requirement(name: &str, ok: bool) {
    if ok {
        println!("  \x1b[32m✓\x1b[0m {name}");
    } else {
        println!("  \x1b[31m✗\x1b[0m {name}");
    }
}

fn which(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

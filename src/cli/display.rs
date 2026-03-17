// Public API — new items used after worktree merge; suppress dead_code lint until then
#![allow(dead_code)]

use std::time::Duration;

use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};

/// Controls how the CLI renders output
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OutputMode {
    #[default]
    Normal,
    Quiet,
    Verbose,
    Json,
}

pub fn step_start(name: &str, step_type: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap(),
    );
    pb.set_message(format!("{} [{}]", name, step_type.dimmed()));
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

pub fn step_ok(pb: &ProgressBar, name: &str, duration: Duration) {
    pb.finish_and_clear();
    println!(
        "  {} {} {}",
        "✓".green(),
        name,
        format!("({:.1}s)", duration.as_secs_f64()).dimmed()
    );
}

pub fn step_fail(pb: &ProgressBar, name: &str, message: &str) {
    pb.finish_and_clear();
    println!("  {} {} — {}", "✗".red(), name, message.red());
}

pub fn step_skip(pb: &ProgressBar, name: &str, message: &str) {
    pb.finish_and_clear();
    println!("  {} {} {}", "→".yellow(), name, message.dimmed());
}

pub fn iteration(current: usize, max: usize) {
    println!("    {} Iteration {}/{}", "↻".cyan(), current + 1, max);
}

pub fn agent_progress(text: &str) {
    if !text.is_empty() {
        for line in text.lines().take(3) {
            println!("    {}", line.dimmed());
        }
    }
}

pub fn tool_use(tool: &str, _input: &str) {
    println!("    {} [tool: {}]", "⚙".blue(), tool);
}

pub fn workflow_start(name: &str) {
    println!("{} {}", "▶".cyan().bold(), name.bold());
}

pub fn workflow_done(duration: Duration, step_count: usize) {
    println!(
        "\n{} Done — {} steps in {:.1}s",
        "✓".green().bold(),
        step_count,
        duration.as_secs_f64()
    );
}

pub fn workflow_failed(step_name: &str, message: &str) {
    println!(
        "\n{} Failed at step '{}': {}",
        "✗".red().bold(),
        step_name,
        message
    );
}

/// Display a map item position, e.g. "Item 2/5: filename.rs"
pub fn map_item(current: usize, total: usize, name: &str) {
    println!(
        "  {} Item {}/{}: {}",
        "◆".cyan(),
        current,
        total,
        name.bold()
    );
}

/// Display a parallel sub-step with indentation
pub fn parallel_step(name: &str) {
    println!("    {} {}", "⟶".blue(), name);
}

/// Display a rich workflow summary including token usage and cost
pub fn workflow_summary(
    steps: usize,
    duration: Duration,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
) {
    println!(
        "\n{} Summary — {} steps in {:.1}s",
        "✓".green().bold(),
        steps,
        duration.as_secs_f64()
    );
    println!(
        "   {} Tokens: {} in / {} out",
        "·".dimmed(),
        input_tokens,
        output_tokens
    );
    println!("   {} Cost:   ${:.4}", "·".dimmed(), cost_usd);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_mode_default_is_normal() {
        let mode = OutputMode::default();
        assert_eq!(mode, OutputMode::Normal);
    }

    #[test]
    fn test_output_mode_variants() {
        let modes = [
            OutputMode::Normal,
            OutputMode::Quiet,
            OutputMode::Verbose,
            OutputMode::Json,
        ];
        // Ensure Clone and PartialEq work
        for m in &modes {
            assert_eq!(m, &m.clone());
        }
    }

    #[test]
    fn test_map_item_does_not_panic() {
        // Just verify it runs without panicking
        map_item(1, 5, "some_file.rs");
        map_item(5, 5, "last_file.rs");
    }

    #[test]
    fn test_parallel_step_does_not_panic() {
        parallel_step("compile");
        parallel_step("lint");
    }

    #[test]
    fn test_workflow_summary_does_not_panic() {
        workflow_summary(10, Duration::from_secs(42), 1234, 567, 0.0023);
    }
}

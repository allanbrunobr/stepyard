use std::time::Duration;

use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};

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
    println!(
        "    {} Iteration {}/{}",
        "↻".cyan(),
        current + 1,
        max
    );
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

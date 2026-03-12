use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::steps::StepOutput;

/// Persisted workflow execution state for resume support
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkflowState {
    pub workflow: String,
    pub session_id: Option<String>,
    pub timestamp: String,
    pub steps: HashMap<String, StepOutput>,
}

impl WorkflowState {
    pub fn new(workflow: &str) -> Self {
        Self {
            workflow: workflow.to_string(),
            session_id: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
            steps: HashMap::new(),
        }
    }

    /// Build a timestamped state file path: /tmp/minion-<workflow>-<timestamp>.state.json
    pub fn state_file_path(workflow: &str) -> PathBuf {
        let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let slug = workflow.replace(' ', "_");
        PathBuf::from(format!("/tmp/minion-{slug}-{ts}.state.json"))
    }

    /// Find the most recently modified state file for a workflow in /tmp
    pub fn find_latest(workflow: &str) -> Option<PathBuf> {
        let prefix = format!("minion-{}-", workflow.replace(' ', "_"));
        let suffix = ".state.json";

        std::fs::read_dir("/tmp")
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&prefix) && n.ends_with(suffix))
                    .unwrap_or(false)
            })
            .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
    }

    /// Persist state to disk
    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load state from disk
    pub fn load(path: &PathBuf) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&json)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::{CmdOutput, StepOutput};
    use std::time::Duration;
    use tempfile::NamedTempFile;

    fn cmd_output(stdout: &str) -> StepOutput {
        StepOutput::Cmd(CmdOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code: 0,
            duration: Duration::ZERO,
        })
    }

    #[test]
    fn save_and_load_roundtrip() {
        let mut state = WorkflowState::new("test-workflow");
        state.steps.insert("step1".to_string(), cmd_output("hello"));
        state.steps.insert("step2".to_string(), cmd_output("world"));
        state.session_id = Some("abc123".to_string());

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        state.save(&path).unwrap();
        let loaded = WorkflowState::load(&path).unwrap();

        assert_eq!(loaded.workflow, "test-workflow");
        assert_eq!(loaded.session_id, Some("abc123".to_string()));
        assert_eq!(loaded.steps["step1"].text(), "hello");
        assert_eq!(loaded.steps["step2"].text(), "world");
    }

    #[test]
    fn state_file_path_contains_workflow_name() {
        let path = WorkflowState::state_file_path("fix-issue");
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("minion-fix-issue-"));
        assert!(name.ends_with(".state.json"));
    }

    #[test]
    fn resume_skips_previous_steps() {
        // Simulate a resume: steps before the resume point get outputs from state
        let mut state = WorkflowState::new("my-workflow");
        state.steps.insert("fetch".to_string(), cmd_output("issue data"));
        state.steps.insert("plan".to_string(), cmd_output("the plan"));

        let resume_from = "implement";
        let step_names = ["fetch", "plan", "implement", "test"];

        let mut skipped = vec![];
        let mut to_execute = vec![];
        let mut found = false;

        for name in &step_names {
            if *name == resume_from {
                found = true;
            }
            if found {
                to_execute.push(*name);
            } else {
                skipped.push(*name);
            }
        }

        assert!(found, "Resume step must be found");
        assert_eq!(skipped, ["fetch", "plan"]);
        assert_eq!(to_execute, ["implement", "test"]);

        // Skipped steps get outputs from state
        for name in &skipped {
            assert!(state.steps.contains_key(*name), "State must have output for {name}");
        }
    }
}

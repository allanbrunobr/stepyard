use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::StepConfig;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::StepDef;

use super::{AgentOutput, StepExecutor, StepOutput};

pub struct TemplateStepExecutor {
    prompts_dir: String,
}

impl TemplateStepExecutor {
    pub fn new(prompts_dir: Option<&str>) -> Self {
        Self {
            prompts_dir: prompts_dir.unwrap_or("prompts").to_string(),
        }
    }
}

#[async_trait]
impl StepExecutor for TemplateStepExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        _config: &StepConfig,
        ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let file_path = PathBuf::from(&self.prompts_dir)
            .join(format!("{}.md.tera", step.name));

        let template_content = tokio::fs::read_to_string(&file_path)
            .await
            .map_err(|e| {
                StepError::Fail(format!(
                    "Template file not found: '{}': {}",
                    file_path.display(),
                    e
                ))
            })?;

        let rendered = ctx.render_template(&template_content)?;

        Ok(StepOutput::Agent(AgentOutput {
            response: rendered,
            session_id: None,
            stats: super::AgentStats::default(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::workflow::schema::StepType;
    use tokio::fs;

    fn make_step(name: &str) -> StepDef {
        StepDef {
            name: name.to_string(),
            step_type: StepType::Template,
            run: None,
            prompt: None,
            condition: None,
            on_pass: None,
            on_fail: None,
            message: None,
            scope: None,
            max_iterations: None,
            initial_value: None,
            items: None,
            parallel: None,
            steps: None,
            config: HashMap::new(),
            outputs: None,
            output_type: None,
        }
    }

    #[tokio::test]
    async fn template_renders_with_context() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let prompts_dir = tmp.path().to_str().unwrap().to_string();

        // Write a .md.tera file
        let template_path = tmp.path().join("greet.md.tera");
        fs::write(&template_path, "Hello {{ target }}!").await.unwrap();

        let step = make_step("greet");
        let executor = TemplateStepExecutor::new(Some(&prompts_dir));
        let config = StepConfig::default();
        let ctx = Context::new("world".to_string(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        assert_eq!(result.text(), "Hello world!");
    }

    #[tokio::test]
    async fn template_file_not_found_descriptive_error() {
        let step = make_step("nonexistent");
        let executor = TemplateStepExecutor::new(Some("/nonexistent/dir"));
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Template file not found") || err.contains("nonexistent"),
            "Error should describe the missing file: {}", err
        );
    }
}

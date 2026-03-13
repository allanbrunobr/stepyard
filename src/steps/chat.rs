use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::config::StepConfig;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::StepDef;

use super::{ChatOutput, StepExecutor, StepOutput};

pub struct ChatExecutor;

#[async_trait]
impl StepExecutor for ChatExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        config: &StepConfig,
        ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let provider = config.get_str("provider").unwrap_or("anthropic");
        let model = config.get_str("model").unwrap_or(match provider {
            "openai" => "gpt-4o-mini",
            _ => "claude-3-haiku-20240307",
        });
        let max_tokens = config.get_u64("max_tokens").unwrap_or(1024);
        let temperature = config
            .values
            .get("temperature")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let api_key_env = config.get_str("api_key_env").unwrap_or(match provider {
            "openai" => "OPENAI_API_KEY",
            _ => "ANTHROPIC_API_KEY",
        });
        let timeout = config
            .get_duration("timeout")
            .unwrap_or(Duration::from_secs(120));

        // Allow base URL override for testing
        let anthropic_base = config
            .get_str("anthropic_base_url")
            .unwrap_or("https://api.anthropic.com");
        let openai_base = config
            .get_str("openai_base_url")
            .unwrap_or("https://api.openai.com");

        let api_key = std::env::var(api_key_env).map_err(|_| {
            StepError::Fail(format!(
                "API key not found: environment variable '{}' is not set",
                api_key_env
            ))
        })?;

        let prompt_template = step
            .prompt
            .as_ref()
            .ok_or_else(|| StepError::Fail("chat step missing 'prompt' field".into()))?;

        let prompt = ctx.render_template(prompt_template)?;

        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| StepError::Fail(format!("Failed to create HTTP client: {e}")))?;

        match provider {
            "openai" => {
                let url = format!("{}/v1/chat/completions", openai_base);
                call_openai(&client, &api_key, model, &prompt, max_tokens, temperature, &url).await
            }
            _ => {
                let url = format!("{}/v1/messages", anthropic_base);
                call_anthropic(&client, &api_key, model, &prompt, max_tokens, temperature, &url).await
            }
        }
    }
}

async fn call_anthropic(
    client: &Client,
    api_key: &str,
    model: &str,
    prompt: &str,
    max_tokens: u64,
    temperature: f64,
    url: &str,
) -> Result<StepOutput, StepError> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "messages": [{"role": "user", "content": prompt}]
    });

    let response = client
        .post(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| StepError::Fail(format!("Anthropic API request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(StepError::Fail(format!(
            "Anthropic API error ({}): {}",
            status, text
        )));
    }

    #[derive(Deserialize)]
    struct AnthropicResponse {
        model: String,
        content: Vec<AnthropicContent>,
        usage: AnthropicUsage,
    }
    #[derive(Deserialize)]
    struct AnthropicContent {
        text: String,
    }
    #[derive(Deserialize)]
    struct AnthropicUsage {
        input_tokens: u64,
        output_tokens: u64,
    }

    let resp: AnthropicResponse = response
        .json()
        .await
        .map_err(|e| StepError::Fail(format!("Failed to parse Anthropic response: {e}")))?;

    let text = resp
        .content
        .into_iter()
        .map(|c| c.text)
        .collect::<Vec<_>>()
        .join("\n");

    Ok(StepOutput::Chat(ChatOutput {
        response: text,
        model: resp.model,
        input_tokens: resp.usage.input_tokens,
        output_tokens: resp.usage.output_tokens,
    }))
}

async fn call_openai(
    client: &Client,
    api_key: &str,
    model: &str,
    prompt: &str,
    max_tokens: u64,
    temperature: f64,
    url: &str,
) -> Result<StepOutput, StepError> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "messages": [{"role": "user", "content": prompt}]
    });

    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| StepError::Fail(format!("OpenAI API request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(StepError::Fail(format!(
            "OpenAI API error ({}): {}",
            status, text
        )));
    }

    #[derive(Deserialize)]
    struct OpenAIResponse {
        model: String,
        choices: Vec<OpenAIChoice>,
        usage: OpenAIUsage,
    }
    #[derive(Deserialize)]
    struct OpenAIChoice {
        message: OpenAIMessage,
    }
    #[derive(Deserialize)]
    struct OpenAIMessage {
        content: String,
    }
    #[derive(Deserialize)]
    struct OpenAIUsage {
        prompt_tokens: u64,
        completion_tokens: u64,
    }

    let resp: OpenAIResponse = response
        .json()
        .await
        .map_err(|e| StepError::Fail(format!("Failed to parse OpenAI response: {e}")))?;

    let text = resp
        .choices
        .into_iter()
        .map(|c| c.message.content)
        .collect::<Vec<_>>()
        .join("\n");

    Ok(StepOutput::Chat(ChatOutput {
        response: text,
        model: resp.model,
        input_tokens: resp.usage.prompt_tokens,
        output_tokens: resp.usage.completion_tokens,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_step(prompt: &str) -> StepDef {
        StepDef {
            name: "test_chat".to_string(),
            step_type: crate::workflow::schema::StepType::Chat,
            run: None,
            prompt: Some(prompt.to_string()),
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
            async_exec: None,
        }
    }

    #[tokio::test]
    async fn chat_missing_api_key_friendly_error() {
        // Use a custom env var name that definitely won't be set
        let step = StepDef {
            name: "test_chat".to_string(),
            step_type: crate::workflow::schema::StepType::Chat,
            run: None,
            prompt: Some("Hello".to_string()),
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
            async_exec: None,
        };
        // Override the api_key_env to a definitely-unset var
        let mut config_values = HashMap::new();
        config_values.insert(
            "api_key_env".to_string(),
            serde_json::Value::String("DEFINITELY_NOT_SET_API_KEY_XYZ123".to_string()),
        );
        let config = StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());
        let result = ChatExecutor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("DEFINITELY_NOT_SET_API_KEY_XYZ123"),
            "Error should mention env var name: {}", err
        );
    }

    #[tokio::test]
    async fn chat_missing_prompt_field_error() {
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
        let step = StepDef {
            name: "test".to_string(),
            step_type: crate::workflow::schema::StepType::Chat,
            run: None,
            prompt: None,  // missing!
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
            async_exec: None,
        };
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());
        let result = ChatExecutor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("prompt"), "Error should mention prompt: {}", err);
    }

    #[tokio::test]
    async fn chat_mock_anthropic_response() {
        // Use a wiremock server to mock the Anthropic API
        use wiremock::{MockServer, Mock, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;
        let response_body = serde_json::json!({
            "model": "claude-3-haiku-20240307",
            "content": [{"type": "text", "text": "Hello from mock!"}],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .mount(&mock_server)
            .await;

        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        let step = make_step("Hello");
        let mut config_values = HashMap::new();
        config_values.insert(
            "anthropic_base_url".to_string(),
            serde_json::Value::String(mock_server.uri()),
        );
        let config = StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = ChatExecutor.execute(&step, &config, &ctx).await.unwrap();
        assert_eq!(result.text(), "Hello from mock!");
        if let StepOutput::Chat(o) = result {
            assert_eq!(o.model, "claude-3-haiku-20240307");
            assert_eq!(o.input_tokens, 10);
            assert_eq!(o.output_tokens, 5);
        } else {
            panic!("Expected Chat output");
        }
    }
}

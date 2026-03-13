use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::steps::StepOutput;

/// A single chat message (user or assistant turn)
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Ordered history of messages for a named chat session
#[derive(Debug, Clone, Default)]
pub struct ChatHistory {
    pub messages: Vec<ChatMessage>,
}

/// Shared chat session store — Arc so child contexts inherit the same store
type ChatSessionStore = Arc<Mutex<HashMap<String, ChatHistory>>>;

/// Tree-structured context that stores step outputs
pub struct Context {
    steps: HashMap<String, StepOutput>,
    variables: HashMap<String, serde_json::Value>,
    parent: Option<Arc<Context>>,
    pub scope_value: Option<serde_json::Value>,
    pub scope_index: usize,
    pub session_id: Option<String>,
    /// Shared chat session store — inherited by child contexts via Arc clone
    chat_sessions: ChatSessionStore,
}

impl Context {
    pub fn new(target: String, vars: HashMap<String, serde_json::Value>) -> Self {
        let mut variables = vars;
        variables.insert("target".to_string(), serde_json::Value::String(target));

        Self {
            steps: HashMap::new(),
            variables,
            parent: None,
            scope_value: None,
            scope_index: 0,
            session_id: None,
            chat_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Store a step output
    pub fn store(&mut self, name: &str, output: StepOutput) {
        if let StepOutput::Agent(ref agent) = output {
            if let Some(ref sid) = agent.session_id {
                self.session_id = Some(sid.clone());
            }
        }
        self.steps.insert(name.to_string(), output);
    }

    /// Get a step output (looks in parent if not found locally)
    pub fn get_step(&self, name: &str) -> Option<&StepOutput> {
        self.steps
            .get(name)
            .or_else(|| self.parent.as_ref().and_then(|p| p.get_step(name)))
    }

    /// Get a variable
    pub fn get_var(&self, name: &str) -> Option<&serde_json::Value> {
        self.variables
            .get(name)
            .or_else(|| self.parent.as_ref().and_then(|p| p.get_var(name)))
    }

    /// Get session ID (searches parent chain)
    pub fn get_session(&self) -> Option<&str> {
        self.session_id
            .as_deref()
            .or_else(|| self.parent.as_ref().and_then(|p| p.get_session()))
    }

    /// Create a child context for a scope
    pub fn child(parent: Arc<Context>, scope_value: Option<serde_json::Value>, index: usize) -> Self {
        Self {
            steps: HashMap::new(),
            variables: HashMap::new(),
            parent: Some(parent.clone()),
            scope_value,
            scope_index: index,
            session_id: parent.session_id.clone(),
            chat_sessions: Arc::clone(&parent.chat_sessions),
        }
    }

    /// Return all stored messages for a chat session (empty vec if session doesn't exist)
    pub fn get_chat_messages(&self, session: &str) -> Vec<ChatMessage> {
        let guard = self.chat_sessions.lock().expect("chat_sessions lock poisoned");
        guard.get(session).map(|h| h.messages.clone()).unwrap_or_default()
    }

    /// Append messages to a chat session, creating the session if it doesn't exist
    pub fn append_chat_messages(&self, session: &str, messages: Vec<ChatMessage>) {
        let mut guard = self.chat_sessions.lock().expect("chat_sessions lock poisoned");
        let history = guard.entry(session.to_string()).or_insert_with(ChatHistory::default);
        history.messages.extend(messages);
    }

    /// Convert to Tera template context
    pub fn to_tera_context(&self) -> tera::Context {
        let mut ctx = tera::Context::new();

        // Add variables
        if let Some(parent) = &self.parent {
            ctx = parent.to_tera_context();
        }
        for (k, v) in &self.variables {
            ctx.insert(k, v);
        }

        // Add steps as a map
        let mut steps_map = HashMap::new();
        // First add parent steps
        if let Some(parent) = &self.parent {
            collect_steps(parent, &mut steps_map);
        }
        // Then local steps (override parent)
        for (name, output) in &self.steps {
            steps_map.insert(name.clone(), step_output_to_value(output));
        }
        ctx.insert("steps", &steps_map);

        // Add scope
        if let Some(sv) = &self.scope_value {
            let mut scope_map = HashMap::new();
            scope_map.insert("value".to_string(), sv.clone());
            scope_map.insert("index".to_string(), serde_json::json!(self.scope_index));
            ctx.insert("scope", &scope_map);
        }

        ctx
    }

    /// Render a template string with this context
    pub fn render_template(&self, template: &str) -> Result<String, crate::error::StepError> {
        let tera_ctx = self.to_tera_context();
        tera::Tera::one_off(template, &tera_ctx, false)
            .map_err(|e| crate::error::StepError::Template(format!("{e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::{CmdOutput, StepOutput};
    use std::time::Duration;

    fn cmd_output(stdout: &str, exit_code: i32) -> StepOutput {
        StepOutput::Cmd(CmdOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code,
            duration: Duration::ZERO,
        })
    }

    #[test]
    fn store_and_retrieve() {
        let mut ctx = Context::new("123".to_string(), HashMap::new());
        ctx.store("step1", cmd_output("hello", 0));
        let out = ctx.get_step("step1").unwrap();
        assert_eq!(out.text(), "hello");
        assert_eq!(out.exit_code(), 0);
    }

    #[test]
    fn parent_context_inheritance() {
        let mut parent = Context::new("456".to_string(), HashMap::new());
        parent.store("parent_step", cmd_output("from parent", 0));
        let child = Context::child(Arc::new(parent), None, 0);
        let out = child.get_step("parent_step").unwrap();
        assert_eq!(out.text(), "from parent");
    }

    #[test]
    fn target_variable_resolves() {
        let ctx = Context::new("42".to_string(), HashMap::new());
        let result = ctx.render_template("{{ target }}").unwrap();
        assert_eq!(result, "42");
    }

    #[test]
    fn render_template_with_step_stdout() {
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store("fetch", cmd_output("some output", 0));
        let result = ctx.render_template("{{ steps.fetch.stdout }}").unwrap();
        assert_eq!(result, "some output");
    }

    #[test]
    fn render_scope_value() {
        let parent = Context::new("".to_string(), HashMap::new());
        let child = Context::child(Arc::new(parent), Some(serde_json::json!("my_value")), 0);
        let result = child.render_template("{{ scope.value }}").unwrap();
        assert_eq!(result, "my_value");
    }

    #[test]
    fn render_template_with_step_exit_code() {
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store("prev", cmd_output("output", 0));
        let result = ctx.render_template("{{ steps.prev.exit_code }}").unwrap();
        assert_eq!(result, "0");
    }

    #[test]
    fn agent_session_id_accessible_in_template() {
        use crate::steps::{AgentOutput, AgentStats, StepOutput};
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store(
            "scan",
            StepOutput::Agent(AgentOutput {
                response: "done".to_string(),
                session_id: Some("sess-abc".to_string()),
                stats: AgentStats::default(),
            }),
        );
        let result = ctx.render_template("{{ steps.scan.session_id }}").unwrap();
        assert_eq!(result, "sess-abc");
    }

    #[test]
    fn cmd_step_session_id_is_empty_string() {
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store("build", cmd_output("output", 0));
        let result = ctx.render_template("{{ steps.build.session_id }}").unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn agent_session_id_none_renders_empty_string() {
        use crate::steps::{AgentOutput, AgentStats, StepOutput};
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store(
            "scan",
            StepOutput::Agent(AgentOutput {
                response: "done".to_string(),
                session_id: None,
                stats: AgentStats::default(),
            }),
        );
        let result = ctx.render_template("{{ steps.scan.session_id }}").unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn child_inherits_parent_steps() {
        let mut parent = Context::new("test".to_string(), HashMap::new());
        parent.store("a", cmd_output("alpha", 0));
        let mut child = Context::child(Arc::new(parent), None, 0);
        child.store("b", cmd_output("beta", 0));
        // Child can see parent step
        assert!(child.get_step("a").is_some());
        // Child can see own step
        assert!(child.get_step("b").is_some());
    }
}

fn collect_steps(ctx: &Context, map: &mut HashMap<String, serde_json::Value>) {
    if let Some(parent) = &ctx.parent {
        collect_steps(parent, map);
    }
    for (name, output) in &ctx.steps {
        map.insert(name.clone(), step_output_to_value(output));
    }
}

fn step_output_to_value(output: &StepOutput) -> serde_json::Value {
    // Serialize to JSON value for template access
    let mut val = serde_json::to_value(output).unwrap_or(serde_json::Value::Null);
    // Story 2.3: ensure session_id is always a string (empty if not an agent output or no session)
    if let serde_json::Value::Object(ref mut map) = val {
        let sid = match map.get("session_id") {
            Some(serde_json::Value::String(s)) => serde_json::Value::String(s.clone()),
            _ => serde_json::Value::String(String::new()),
        };
        map.insert("session_id".to_string(), sid);
    }
    val
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::prompts::detector::StackInfo;
use crate::steps::{ParsedValue, StepOutput};

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
    parsed_outputs: HashMap<String, ParsedValue>,
    variables: HashMap<String, serde_json::Value>,
    parent: Option<Arc<Context>>,
    pub scope_value: Option<serde_json::Value>,
    pub scope_index: usize,
    pub session_id: Option<String>,
    /// Shared chat session store — inherited by child contexts via Arc clone
    chat_sessions: ChatSessionStore,
    /// Detected stack info for prompt resolution (Story 11.5/11.6)
    pub stack_info: Option<StackInfo>,
    /// Directory where prompt template files live (defaults to "prompts")
    pub prompts_dir: PathBuf,
}

impl Context {
    pub fn new(target: String, vars: HashMap<String, serde_json::Value>) -> Self {
        let mut variables = vars;
        variables.insert("target".to_string(), serde_json::Value::String(target));

        Self {
            steps: HashMap::new(),
            parsed_outputs: HashMap::new(),
            variables,
            parent: None,
            scope_value: None,
            scope_index: 0,
            session_id: None,
            chat_sessions: Arc::new(Mutex::new(HashMap::new())),
            stack_info: None,
            prompts_dir: PathBuf::from("prompts"),
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

    /// Insert a variable into this context
    pub fn insert_var(&mut self, name: impl Into<String>, value: serde_json::Value) {
        self.variables.insert(name.into(), value);
    }

    /// Get a variable
    pub fn get_var(&self, name: &str) -> Option<&serde_json::Value> {
        self.variables
            .get(name)
            .or_else(|| self.parent.as_ref().and_then(|p| p.get_var(name)))
    }

    /// Get session ID (searches parent chain)
    #[allow(dead_code)]
    pub fn get_session(&self) -> Option<&str> {
        self.session_id
            .as_deref()
            .or_else(|| self.parent.as_ref().and_then(|p| p.get_session()))
    }

    /// Store a parsed value for a step
    pub fn store_parsed(&mut self, name: &str, parsed: ParsedValue) {
        self.parsed_outputs.insert(name.to_string(), parsed);
    }

    /// Get a parsed value for a step (looks in parent if not found locally)
    pub fn get_parsed(&self, name: &str) -> Option<&ParsedValue> {
        self.parsed_outputs
            .get(name)
            .or_else(|| self.parent.as_ref().and_then(|p| p.get_parsed(name)))
    }

    /// Create a child context for a scope
    #[allow(dead_code)]
    pub fn child(
        parent: Arc<Context>,
        scope_value: Option<serde_json::Value>,
        index: usize,
    ) -> Self {
        let stack_info = parent.stack_info.clone();
        let prompts_dir = parent.prompts_dir.clone();
        Self {
            steps: HashMap::new(),
            parsed_outputs: HashMap::new(),
            variables: HashMap::new(),
            parent: Some(parent.clone()),
            scope_value,
            scope_index: index,
            session_id: parent.session_id.clone(),
            chat_sessions: Arc::clone(&parent.chat_sessions),
            stack_info,
            prompts_dir,
        }
    }

    /// Get all variables (local + parent chain) merged into a flat HashMap
    pub fn all_variables(&self) -> HashMap<String, serde_json::Value> {
        let mut result = HashMap::new();
        if let Some(ref parent) = self.parent {
            result = parent.all_variables();
        }
        result.extend(self.variables.clone());
        result
    }

    /// Get the stack_info from this context or any parent
    pub fn get_stack_info(&self) -> Option<&StackInfo> {
        self.stack_info
            .as_ref()
            .or_else(|| self.parent.as_ref().and_then(|p| p.get_stack_info()))
    }

    /// Get the Tera-ready value for a step by name (used by from() preprocessing).
    /// Returns None if the step doesn't exist in this context or any parent.
    pub fn get_from_value(&self, name: &str) -> Option<serde_json::Value> {
        let step = self.get_step(name)?;
        let parsed = self.get_parsed(name);
        Some(step_output_to_value_with_parsed(step, parsed))
    }

    /// Check if a dotted path variable exists in this context (used for strict accessor)
    pub fn var_exists(&self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('.').collect();
        if parts.is_empty() {
            return false;
        }
        let root = parts[0];
        if let Some(step) = self.get_step(root) {
            if parts.len() == 1 {
                return true;
            }
            let val = step_output_to_value_with_parsed(step, self.get_parsed(root));
            return check_json_path(&val, &parts[1..]);
        }
        if let Some(var) = self.get_var(root) {
            if parts.len() == 1 {
                return true;
            }
            return check_json_path(var, &parts[1..]);
        }
        false
    }

    /// Return all stored messages for a chat session (empty vec if session doesn't exist)
    pub fn get_chat_messages(&self, session: &str) -> Vec<ChatMessage> {
        let guard = self
            .chat_sessions
            .lock()
            .expect("chat_sessions lock poisoned");
        guard
            .get(session)
            .map(|h| h.messages.clone())
            .unwrap_or_default()
    }

    /// Append messages to a chat session, creating the session if it doesn't exist
    pub fn append_chat_messages(&self, session: &str, messages: Vec<ChatMessage>) {
        let mut guard = self
            .chat_sessions
            .lock()
            .expect("chat_sessions lock poisoned");
        let history = guard.entry(session.to_string()).or_default();
        history.messages.extend(messages);
    }

    /// Convert to Tera template context
    pub fn to_tera_context(&self) -> tera::Context {
        let mut ctx = tera::Context::new();

        // Add variables (parent first, then override with local)
        if let Some(parent) = &self.parent {
            ctx = parent.to_tera_context();
        }
        for (k, v) in &self.variables {
            ctx.insert(k, v);
        }

        // Build full steps map (parent + local)
        let mut steps_map: HashMap<String, serde_json::Value> = HashMap::new();
        if let Some(parent) = &self.parent {
            collect_steps_with_parsed(parent, &mut steps_map);
        }
        for (name, output) in &self.steps {
            let parsed = self.parsed_outputs.get(name);
            let val = step_output_to_value_with_parsed(output, parsed);
            steps_map.insert(name.clone(), val);
        }

        // Insert steps both under "steps" and directly by name for flexible access
        for (name, val) in &steps_map {
            ctx.insert(name.as_str(), val);
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
        // Pre-process template: handle ?, !, and from("name") → __from_name__ substitution
        let pre = crate::engine::template::preprocess_template(template, self)?;

        // Build base Tera context (steps, vars, scope)
        let mut tera_ctx = self.to_tera_context();

        // Inject from() lookup variables into the Tera context
        for (k, v) in &pre.injected {
            tera_ctx.insert(k.as_str(), v);
        }

        // Build Tera instance and render
        let mut tera = tera::Tera::default();
        tera.add_raw_template("__tmpl__", &pre.template)
            .map_err(|e| crate::error::StepError::Template(format!("{e}")))?;

        tera.render("__tmpl__", &tera_ctx)
            .map_err(|e| crate::error::StepError::Template(format!("{e}")))
    }
}

fn collect_steps_with_parsed(ctx: &Context, map: &mut HashMap<String, serde_json::Value>) {
    if let Some(parent) = &ctx.parent {
        collect_steps_with_parsed(parent, map);
    }
    for (name, output) in &ctx.steps {
        let parsed = ctx.parsed_outputs.get(name);
        map.insert(
            name.clone(),
            step_output_to_value_with_parsed(output, parsed),
        );
    }
}

fn step_output_to_value_with_parsed(
    output: &StepOutput,
    parsed: Option<&ParsedValue>,
) -> serde_json::Value {
    let mut val = serde_json::to_value(output).unwrap_or(serde_json::Value::Null);

    if let serde_json::Value::Object(ref mut map) = val {
        // Add "output" key for template access (typed output parsing)
        let output_val = match parsed {
            Some(ParsedValue::Json(j)) => j.clone(),
            Some(ParsedValue::Lines(lines)) => serde_json::json!(lines),
            Some(ParsedValue::Integer(n)) => serde_json::json!(n),
            Some(ParsedValue::Boolean(b)) => serde_json::json!(b),
            Some(ParsedValue::Text(t)) => serde_json::Value::String(t.clone()),
            None => serde_json::Value::String(output.text().to_string()),
        };
        map.insert("output".to_string(), output_val);

        // Story 2.3: ensure session_id is always a string (empty if not an agent output)
        let sid = match map.get("session_id") {
            Some(serde_json::Value::String(s)) => serde_json::Value::String(s.clone()),
            _ => serde_json::Value::String(String::new()),
        };
        map.insert("session_id".to_string(), sid);
    }

    val
}

fn check_json_path(val: &serde_json::Value, path: &[&str]) -> bool {
    if path.is_empty() {
        return true;
    }
    match val {
        serde_json::Value::Object(map) => {
            if let Some(next) = map.get(path[0]) {
                check_json_path(next, &path[1..])
            } else {
                false
            }
        }
        _ => false,
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

    #[test]
    fn output_key_defaults_to_text() {
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store("fetch", cmd_output("hello world", 0));
        // Without parsed value, {{fetch.output}} returns the raw text
        let result = ctx.render_template("{{ fetch.output }}").unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn output_key_with_json_parsed_value() {
        use crate::steps::ParsedValue;
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store("scan", cmd_output(r#"{"count": 5}"#, 0));
        ctx.store_parsed("scan", ParsedValue::Json(serde_json::json!({"count": 5})));
        // JSON parsed value allows dot-path access
        let result = ctx.render_template("{{ scan.output.count }}").unwrap();
        assert_eq!(result, "5");
    }

    #[test]
    fn output_key_with_lines_parsed_value() {
        use crate::steps::ParsedValue;
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store("files", cmd_output("a.rs\nb.rs\nc.rs", 0));
        ctx.store_parsed(
            "files",
            ParsedValue::Lines(vec!["a.rs".into(), "b.rs".into(), "c.rs".into()]),
        );
        // Lines parsed value renders with | length filter
        let result = ctx.render_template("{{ files.output | length }}").unwrap();
        assert_eq!(result, "3");
    }

    #[test]
    fn step_accessible_directly_by_name() {
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store("greet", cmd_output("hi", 0));
        // Steps are also accessible directly by name (not just via steps.)
        let result = ctx.render_template("{{ greet.output }}").unwrap();
        assert_eq!(result, "hi");
    }

    #[test]
    fn from_accesses_step_by_name() {
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store("global-config", cmd_output("prod", 0));
        // from("name") syntax allows accessing any step by name
        let result = ctx
            .render_template(r#"{{ from("global-config").output }}"#)
            .unwrap();
        assert_eq!(result, "prod");
    }

    #[test]
    fn from_fails_for_nonexistent_step() {
        let ctx = Context::new("".to_string(), HashMap::new());
        let err = ctx
            .render_template(r#"{{ from("nonexistent").output }}"#)
            .unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "expected 'not found' error, got: {err}"
        );
    }

    #[test]
    fn from_with_json_dot_access() {
        use crate::steps::ParsedValue;
        let mut ctx = Context::new("".to_string(), HashMap::new());
        ctx.store("scan", cmd_output(r#"{"issues": [1, 2]}"#, 0));
        ctx.store_parsed(
            "scan",
            ParsedValue::Json(serde_json::json!({"issues": [1, 2]})),
        );
        // from() with JSON output allows deep dot-path access
        let result = ctx
            .render_template(r#"{{ from("scan").output.issues | length }}"#)
            .unwrap();
        assert_eq!(result, "2");
    }

    #[test]
    fn from_traverses_parent_scope() {
        let mut parent = Context::new("".to_string(), HashMap::new());
        parent.store("root-step", cmd_output("root-value", 0));
        let child = Context::child(Arc::new(parent), None, 0);
        // from() inside child scope can access parent scope steps
        let result = child
            .render_template(r#"{{ from("root-step").output }}"#)
            .unwrap();
        assert_eq!(result, "root-value");
    }

    #[test]
    fn from_safe_accessor_returns_empty_when_step_missing() {
        let ctx = Context::new("".to_string(), HashMap::new());
        // from("nonexistent").output? should return "" not fail
        let result = ctx
            .render_template(r#"{{ from("nonexistent").output? }}"#)
            .unwrap();
        assert_eq!(result, "");
    }
}

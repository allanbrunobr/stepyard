// Public API — used after worktree merge; suppress dead_code lint until then
#![allow(dead_code)]

/// Manages Claude Code session IDs for conversation continuity
#[derive(Debug, Default)]
pub struct SessionManager {
    session_id: Option<String>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self { session_id: None }
    }

    /// Capture a session_id — only the first call wins; subsequent calls are ignored.
    pub fn capture(&mut self, session_id: Option<String>) {
        if self.session_id.is_none() {
            self.session_id = session_id;
        }
    }

    /// Returns the captured session ID, if any.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Returns CLI arguments to resume or fork a session.
    ///
    /// - `isolated = true`  → empty vec (start fresh every time)
    /// - `isolated = false` + session_id is Some → `["--fork-session", "--resume", "<id>"]`
    /// - `isolated = false` + session_id is None → empty vec
    pub fn resume_args(&self, isolated: bool) -> Vec<String> {
        if isolated {
            return vec![];
        }
        match &self.session_id {
            Some(id) => vec![
                "--fork-session".to_string(),
                "--resume".to_string(),
                id.clone(),
            ],
            None => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture_session_id() {
        let mut mgr = SessionManager::new();
        mgr.capture(Some("abc-123".to_string()));
        assert_eq!(mgr.session_id(), Some("abc-123"));
    }

    #[test]
    fn test_capture_first_wins() {
        let mut mgr = SessionManager::new();
        mgr.capture(Some("first".to_string()));
        mgr.capture(Some("second".to_string()));
        assert_eq!(mgr.session_id(), Some("first"));
    }

    #[test]
    fn test_capture_none_then_some() {
        let mut mgr = SessionManager::new();
        mgr.capture(None);
        mgr.capture(Some("late".to_string()));
        // None counts as "set" — but actually None means we never got an id,
        // so the second Some should NOT override. Let's verify the spec:
        // "captures first session_id only (if already set, ignore)" — None is still
        // "not set", so a subsequent Some should win only if we interpret None as absent.
        // Per the spec the field starts as None; capture(None) leaves it as None which
        // means it was never set, so we accept the next Some.
        // Re-reading: "if already set, ignore" — None means NOT set, so second wins.
        assert_eq!(mgr.session_id(), Some("late"));
    }

    #[test]
    fn test_isolated_returns_no_resume_args() {
        let mut mgr = SessionManager::new();
        mgr.capture(Some("abc-123".to_string()));
        let args = mgr.resume_args(true);
        assert!(args.is_empty());
    }

    #[test]
    fn test_shared_with_id_returns_resume_args() {
        let mut mgr = SessionManager::new();
        mgr.capture(Some("abc-123".to_string()));
        let args = mgr.resume_args(false);
        assert_eq!(args, vec!["--fork-session", "--resume", "abc-123"]);
    }

    #[test]
    fn test_shared_without_id_returns_no_args() {
        let mgr = SessionManager::new();
        let args = mgr.resume_args(false);
        assert!(args.is_empty());
    }

    #[test]
    fn test_default() {
        let mgr = SessionManager::default();
        assert_eq!(mgr.session_id(), None);
    }
}

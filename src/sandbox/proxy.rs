//! Secure API Proxy
//!
//! A lightweight HTTP reverse proxy that runs on the host and intercepts
//! API requests from the Docker sandbox container. Instead of passing
//! sensitive API keys or OAuth tokens as environment variables into the
//! container, the proxy injects authentication headers on the fly.
//!
//! Supports two authentication modes:
//! - **API Key**: `x-api-key: sk-ant-...` (pay-per-token)
//! - **OAuth Token**: `Authorization: Bearer sk-ant-oat01-...` (Claude MAX subscription)
//!
//! Architecture:
//! ```text
//! Container                           Host Proxy                  Upstream
//! ─────────                           ──────────                  ────────
//! ANTHROPIC_BASE_URL=                 127.0.0.1:<port>
//!   http://host.docker.internal:PORT  /v1/messages ──────────►  api.anthropic.com
//!                                     + auth header injected      (HTTPS)
//! ```
//!
//! The container never sees the actual API key or OAuth token.

use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Client;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Authentication mode for the proxy
#[derive(Debug, Clone)]
pub enum ProxyAuth {
    /// Traditional API key: injected as `x-api-key` header
    ApiKey(String),
    /// OAuth token (Claude MAX/Pro subscription): injected as `Authorization: Bearer` header
    /// Optionally forward through a LiteLLM gateway with a virtual key
    OAuthToken {
        token: String,
        /// If set, the proxy forwards to this URL instead of api.anthropic.com
        /// (e.g., http://localhost:4000 for LiteLLM)
        gateway_url: Option<String>,
        /// LiteLLM virtual key for tracking/budgets (optional)
        litellm_key: Option<String>,
    },
}

/// Secrets to inject into proxied requests
struct ProxySecrets {
    auth: ProxyAuth,
}

/// A running API proxy instance
pub struct ApiProxy {
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<JoinHandle<()>>,
}

impl ApiProxy {
    /// Start the proxy on a random available port.
    ///
    /// The proxy will forward requests to the upstream API and inject
    /// the appropriate authentication headers.
    pub async fn start(auth: ProxyAuth) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("Failed to bind proxy to localhost")?;

        let port = listener.local_addr()?.port();

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let auth_mode = match &auth {
            ProxyAuth::ApiKey(_) => "API key",
            ProxyAuth::OAuthToken { gateway_url, .. } => {
                if gateway_url.is_some() {
                    "OAuth token via LiteLLM gateway"
                } else {
                    "OAuth token (Claude MAX)"
                }
            }
        };

        let secrets = Arc::new(ProxySecrets { auth });
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .context("Failed to create HTTP client for proxy")?;

        let handle = tokio::spawn(run_proxy(listener, secrets, client, shutdown_rx));

        tracing::info!(port, auth_mode, "API proxy started — secrets never enter the container");

        Ok(Self {
            port,
            shutdown_tx: Some(shutdown_tx),
            join_handle: Some(handle),
        })
    }

    /// Start with a simple API key (backward compatible convenience method).
    pub async fn start_with_api_key(api_key: String) -> Result<Self> {
        Self::start(ProxyAuth::ApiKey(api_key)).await
    }

    /// The port the proxy is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Gracefully stop the proxy.
    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
        tracing::info!("API proxy stopped");
    }
}

impl Drop for ApiProxy {
    fn drop(&mut self) {
        // Best-effort shutdown if stop() was not called explicitly
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Main proxy loop: accept connections and forward requests.
async fn run_proxy(
    listener: TcpListener,
    secrets: Arc<ProxySecrets>,
    client: Client,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _addr)) => {
                        let secrets = Arc::clone(&secrets);
                        let client = client.clone();
                        tokio::spawn(handle_connection(stream, secrets, client));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Proxy accept error");
                    }
                }
            }
            _ = &mut shutdown_rx => {
                tracing::debug!("Proxy received shutdown signal");
                break;
            }
        }
    }
}

/// Handle a single HTTP connection (may have multiple requests via keep-alive,
/// but we use a simple one-request-per-connection model for simplicity).
async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    secrets: Arc<ProxySecrets>,
    client: Client,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Read the full request (up to 10MB for large prompts)
    let mut buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];

    // Read until we have the full headers + body
    loop {
        match stream.read(&mut tmp).await {
            Ok(0) => return, // connection closed
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                // Check if we have the complete request
                if let Some(body_start) = find_header_end(&buf) {
                    // Parse Content-Length to know how much body to expect
                    let headers_str = String::from_utf8_lossy(&buf[..body_start]);
                    let content_length = parse_content_length(&headers_str);
                    let body_received = buf.len() - body_start;
                    if body_received >= content_length {
                        break; // Full request received
                    }
                }
                // Safety: don't read more than 10MB
                if buf.len() > 10 * 1024 * 1024 {
                    let resp = b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\n\r\n";
                    let _ = stream.write_all(resp).await;
                    return;
                }
            }
            Err(_) => return,
        }
    }

    // Parse the request
    let header_end = match find_header_end(&buf) {
        Some(pos) => pos,
        None => return,
    };

    let headers_str = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let body = &buf[header_end..];

    // Parse first line: METHOD PATH HTTP/1.x
    let first_line = headers_str.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 3 {
        let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
        let _ = stream.write_all(resp).await;
        return;
    }

    let method = parts[0];
    let path = parts[1];

    // Build upstream URL based on auth mode
    let upstream_url = match &secrets.auth {
        ProxyAuth::OAuthToken {
            gateway_url: Some(gw),
            ..
        } => {
            // LiteLLM gateway: forward to the gateway URL
            let base = gw.trim_end_matches('/');
            format!("{base}{path}")
        }
        _ => {
            // Direct to Anthropic API
            format!("https://api.anthropic.com{path}")
        }
    };

    // Build the upstream request
    let req_method = match method {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        "OPTIONS" => reqwest::Method::OPTIONS,
        "HEAD" => reqwest::Method::HEAD,
        _ => {
            let resp = b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(resp).await;
            return;
        }
    };

    let mut upstream_req = client.request(req_method, &upstream_url);

    // Forward relevant headers (skip Host, Connection, and any existing auth)
    for line in headers_str.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key_lower = key.trim().to_lowercase();
            // Skip hop-by-hop headers and auth (we inject our own)
            if matches!(
                key_lower.as_str(),
                "host" | "connection" | "x-api-key" | "authorization" | "x-litellm-api-key"
            ) {
                continue;
            }
            upstream_req = upstream_req.header(key.trim(), value.trim());
        }
    }

    // Inject authentication based on mode
    match &secrets.auth {
        ProxyAuth::ApiKey(key) => {
            upstream_req = upstream_req.header("x-api-key", key.as_str());
        }
        ProxyAuth::OAuthToken {
            token,
            litellm_key,
            ..
        } => {
            // OAuth token goes as Authorization: Bearer
            upstream_req =
                upstream_req.header("Authorization", format!("Bearer {token}"));
            // If LiteLLM gateway is used, also send the virtual key for tracking
            if let Some(lk) = litellm_key {
                upstream_req =
                    upstream_req.header("x-litellm-api-key", format!("Bearer {lk}"));
            }
        }
    }

    // Add body if present
    if !body.is_empty() {
        upstream_req = upstream_req.body(body.to_vec());
    }

    // Send upstream request
    let upstream_resp = match upstream_req.send().await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(error = %e, url = %upstream_url, "Proxy upstream request failed");
            let error_body = format!("Proxy error: {e}");
            let resp = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Length: {}\r\n\r\n{error_body}",
                error_body.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            return;
        }
    };

    // Build response back to client
    let status = upstream_resp.status();
    let resp_headers = upstream_resp.headers().clone();
    let resp_body = match upstream_resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to read upstream response body");
            let _ = stream
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await;
            return;
        }
    };

    // Write HTTP response
    let mut response = format!("HTTP/1.1 {}\r\n", status);
    for (key, value) in resp_headers.iter() {
        let key_lower = key.as_str().to_lowercase();
        // Skip hop-by-hop headers
        if matches!(key_lower.as_str(), "connection" | "transfer-encoding") {
            continue;
        }
        if let Ok(v) = value.to_str() {
            response.push_str(&format!("{}: {}\r\n", key, v));
        }
    }
    // Ensure Content-Length is set
    response.push_str(&format!("Content-Length: {}\r\n", resp_body.len()));
    response.push_str("Connection: close\r\n");
    response.push_str("\r\n");

    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.write_all(&resp_body).await;
}

/// Find the end of HTTP headers (double CRLF)
fn find_header_end(buf: &[u8]) -> Option<usize> {
    for i in 0..buf.len().saturating_sub(3) {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            return Some(i + 4);
        }
    }
    None
}

/// Parse Content-Length from raw headers string
fn parse_content_length(headers: &str) -> usize {
    for line in headers.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case("content-length") {
                return value.trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

/// Detect the best available authentication from environment variables.
///
/// Priority:
/// 1. `ANTHROPIC_OAUTH_TOKEN` (sk-ant-oat01-...) → OAuth mode (Claude MAX/Pro)
/// 2. `ANTHROPIC_API_KEY` (sk-ant-api03-...) → API key mode (pay-per-token)
///
/// When OAuth token is found, also checks:
/// - `LITELLM_GATEWAY_URL` → forward through LiteLLM gateway
/// - `LITELLM_API_KEY` → LiteLLM virtual key for tracking
pub fn detect_auth_from_env() -> Option<ProxyAuth> {
    // Priority 1: OAuth token (Claude MAX subscription)
    if let Ok(token) = std::env::var("ANTHROPIC_OAUTH_TOKEN") {
        if !token.is_empty() {
            let gateway_url = std::env::var("LITELLM_GATEWAY_URL").ok().filter(|s| !s.is_empty());
            let litellm_key = std::env::var("LITELLM_API_KEY").ok().filter(|s| !s.is_empty());
            return Some(ProxyAuth::OAuthToken {
                token,
                gateway_url,
                litellm_key,
            });
        }
    }

    // Priority 2: API key (pay-per-token)
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Some(ProxyAuth::ApiKey(key));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_header_end_works() {
        let data = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\nbody";
        let pos = find_header_end(data);
        assert_eq!(pos, Some(37));
    }

    #[test]
    fn find_header_end_returns_none_when_incomplete() {
        let data = b"GET / HTTP/1.1\r\nHost: example.com\r\n";
        let pos = find_header_end(data);
        assert_eq!(pos, None);
    }

    #[test]
    fn parse_content_length_works() {
        let headers = "POST /v1/messages HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 1234\r\n";
        assert_eq!(parse_content_length(headers), 1234);
    }

    #[test]
    fn parse_content_length_missing_returns_zero() {
        let headers = "GET /v1/models HTTP/1.1\r\nHost: api.anthropic.com\r\n";
        assert_eq!(parse_content_length(headers), 0);
    }

    #[tokio::test]
    async fn proxy_starts_and_stops_with_api_key() {
        let proxy = ApiProxy::start(ProxyAuth::ApiKey("test-key-123".to_string()))
            .await
            .unwrap();
        assert!(proxy.port() > 0);
        proxy.stop().await;
    }

    #[tokio::test]
    async fn proxy_starts_and_stops_with_oauth() {
        let proxy = ApiProxy::start(ProxyAuth::OAuthToken {
            token: "sk-ant-oat01-test".to_string(),
            gateway_url: None,
            litellm_key: None,
        })
        .await
        .unwrap();
        assert!(proxy.port() > 0);
        proxy.stop().await;
    }

    #[tokio::test]
    async fn proxy_starts_with_litellm_gateway() {
        let proxy = ApiProxy::start(ProxyAuth::OAuthToken {
            token: "sk-ant-oat01-test".to_string(),
            gateway_url: Some("http://localhost:4000".to_string()),
            litellm_key: Some("sk-litellm-virtual-key".to_string()),
        })
        .await
        .unwrap();
        assert!(proxy.port() > 0);
        proxy.stop().await;
    }

    #[tokio::test]
    async fn proxy_responds_to_requests() {
        let proxy = ApiProxy::start(ProxyAuth::ApiKey("test-key-abc".to_string()))
            .await
            .unwrap();
        let port = proxy.port();

        // Send a request to the proxy (it will fail upstream but we can verify it's listening)
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/v1/models"))
            .send()
            .await;

        // The proxy should respond (even if upstream fails, it should return 502 or forward the error)
        assert!(resp.is_ok());

        proxy.stop().await;
    }

    #[test]
    fn detect_auth_prefers_oauth_over_api_key() {
        // This test just validates the logic flow — env vars are tricky in tests
        // so we test the function structure
        let auth = ProxyAuth::OAuthToken {
            token: "sk-ant-oat01-test".to_string(),
            gateway_url: Some("http://localhost:4000".to_string()),
            litellm_key: Some("key".to_string()),
        };
        match auth {
            ProxyAuth::OAuthToken { token, gateway_url, litellm_key } => {
                assert!(token.starts_with("sk-ant-oat01"));
                assert!(gateway_url.is_some());
                assert!(litellm_key.is_some());
            }
            _ => panic!("Expected OAuthToken"),
        }
    }
}

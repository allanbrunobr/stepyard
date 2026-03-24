//! Secure API Key Proxy
//!
//! A lightweight HTTP reverse proxy that runs on the host and intercepts
//! API requests from the Docker sandbox container. Instead of passing
//! sensitive API keys as environment variables into the container, the
//! proxy injects authentication headers on the fly.
//!
//! Architecture:
//! ```text
//! Container                           Host Proxy                  Upstream
//! ─────────                           ──────────                  ────────
//! ANTHROPIC_BASE_URL=                 127.0.0.1:<port>
//!   http://host.docker.internal:PORT  /v1/messages ──────────►  api.anthropic.com
//!                                     + x-api-key: sk-ant-...    (HTTPS)
//! ```
//!
//! The container never sees the actual API key.

use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Client;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Secrets to inject into proxied requests
struct ProxySecrets {
    anthropic_api_key: String,
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
    /// The proxy will forward requests to `api.anthropic.com` and inject
    /// the `x-api-key` header with the provided API key.
    pub async fn start(anthropic_api_key: String) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("Failed to bind proxy to localhost")?;

        let port = listener.local_addr()?.port();

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let secrets = Arc::new(ProxySecrets { anthropic_api_key });
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .context("Failed to create HTTP client for proxy")?;

        let handle = tokio::spawn(run_proxy(listener, secrets, client, shutdown_rx));

        tracing::info!(port, "API key proxy started — secrets never enter the container");

        Ok(Self {
            port,
            shutdown_tx: Some(shutdown_tx),
            join_handle: Some(handle),
        })
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
        tracing::info!("API key proxy stopped");
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

    // Build upstream URL — all paths go to api.anthropic.com
    let upstream_url = format!("https://api.anthropic.com{path}");

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
                "host" | "connection" | "x-api-key" | "authorization"
            ) {
                continue;
            }
            upstream_req = upstream_req.header(key.trim(), value.trim());
        }
    }

    // Inject the API key
    upstream_req = upstream_req.header("x-api-key", &secrets.anthropic_api_key);

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
    async fn proxy_starts_and_stops() {
        let proxy = ApiProxy::start("test-key-123".to_string()).await.unwrap();
        assert!(proxy.port() > 0);
        proxy.stop().await;
    }

    #[tokio::test]
    async fn proxy_responds_to_requests() {
        let proxy = ApiProxy::start("test-key-abc".to_string()).await.unwrap();
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
}

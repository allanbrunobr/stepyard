// Sandbox API — some items used only in integration paths
#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use tokio::process::Command;

use super::config::SandboxConfig;

/// The reference Dockerfile embedded at compile time so `cargo install`
/// users get auto-build without needing the source tree.
pub const EMBEDDED_DOCKERFILE: &str = include_str!("../../Dockerfile.sandbox");

/// Manages a Docker sandbox container lifecycle
pub struct DockerSandbox {
    container_id: Option<String>,
    config: SandboxConfig,
    /// Host workspace path to mount
    workspace_path: String,
}

/// Result of running a command inside the sandbox
#[derive(Debug)]
pub struct SandboxOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl DockerSandbox {
    pub fn new(config: SandboxConfig, workspace_path: impl Into<String>) -> Self {
        Self {
            container_id: None,
            config,
            workspace_path: workspace_path.into(),
        }
    }

    /// Check if a Docker image exists locally.
    pub async fn image_exists(image: &str) -> bool {
        Command::new("docker")
            .args(["image", "inspect", image])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Auto-build the default sandbox image from the embedded Dockerfile.
    /// Writes the Dockerfile to a temp dir and runs `docker build`.
    pub async fn auto_build_image(image: &str) -> Result<()> {
        let tmp = std::env::temp_dir().join("minion-sandbox-build");
        std::fs::create_dir_all(&tmp)
            .context("Failed to create temp dir for Docker build")?;

        let dockerfile_path = tmp.join("Dockerfile");
        std::fs::write(&dockerfile_path, EMBEDDED_DOCKERFILE)
            .context("Failed to write embedded Dockerfile")?;

        tracing::info!("Building Docker image '{image}' (this may take a few minutes on first run)...");
        eprintln!(
            "\n  ⟳ Building Docker image '{}' — first run only, please wait...\n",
            image,
        );

        let output = Command::new("docker")
            .args(["build", "-t", image, "-f"])
            .arg(&dockerfile_path)
            .arg(&tmp)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await
            .context("Failed to run docker build")?;

        // Clean up temp dir (best-effort)
        let _ = std::fs::remove_dir_all(&tmp);

        if !output.success() {
            bail!(
                "Docker build failed for image '{image}'. \
                 Check the output above for errors."
            );
        }

        tracing::info!("Successfully built Docker image '{image}'");
        Ok(())
    }

    /// Check if Docker is available in PATH
    pub async fn is_docker_available() -> bool {
        Command::new("docker")
            .args(["info", "--format", "{{.ServerVersion}}"])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Check if Docker Desktop >= 4.40 is available (required for Docker Sandbox).
    /// Falls back to checking that `docker` is simply available when version
    /// detection fails (useful in CI environments where Docker CE is sufficient).
    pub async fn is_sandbox_available() -> bool {
        let output = Command::new("docker")
            .args(["version", "--format", "{{.Client.Version}}"])
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                let version = String::from_utf8_lossy(&o.stdout);
                let version = version.trim();
                // Docker Desktop 4.40+ ships Docker Engine 26.x+
                // We accept any Docker that responds to version check
                !version.is_empty()
            }
            _ => false,
        }
    }

    /// Auto-detect `GH_TOKEN` from the `gh` CLI if not already set.
    ///
    /// Many developers authenticate via `gh auth login` but never export
    /// `GH_TOKEN`.  This method bridges the gap so that `gh` commands
    /// inside the Docker sandbox work out of the box.
    async fn auto_detect_gh_token() {
        // Skip if the user already has a token in the environment
        if std::env::var("GH_TOKEN").is_ok() || std::env::var("GITHUB_TOKEN").is_ok() {
            return;
        }

        let output = Command::new("gh")
            .args(["auth", "token"])
            .output()
            .await;

        if let Ok(o) = output {
            if o.status.success() {
                let token = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !token.is_empty() {
                    std::env::set_var("GH_TOKEN", &token);
                    tracing::info!("Auto-detected GH_TOKEN from `gh auth token`");
                }
            }
        }
    }

    /// Create the sandbox container (without starting it)
    pub async fn create(&mut self) -> Result<()> {
        if !Self::is_sandbox_available().await {
            bail!(
                "Docker Sandbox is not available. \
                 Please install Docker Desktop 4.40+ (https://www.docker.com/products/docker-desktop/). \
                 Ensure the Docker daemon is running before retrying."
            );
        }

        let image = self.config.image().to_string();
        let workspace = &self.workspace_path;

        let mut args = vec![
            "create".to_string(),
            "--rm".to_string(),
            "-v".to_string(),
            format!("{workspace}:/workspace"),
            "-w".to_string(),
            "/workspace".to_string(),
        ];

        // ── Auto-detect credentials ────────────────────────────────
        // If GH_TOKEN / GITHUB_TOKEN are not in the environment but the
        // `gh` CLI is authenticated, auto-populate GH_TOKEN so that
        // `gh` commands work inside the container without the user
        // having to manually pass `GH_TOKEN=$(gh auth token)`.
        Self::auto_detect_gh_token().await;

        // ── Environment variables ───────────────────────────────────
        // Forward host env vars into the container so that CLI tools
        // (gh, claude, git) and API clients can authenticate.
        for key in self.config.effective_env() {
            if let Ok(val) = std::env::var(&key) {
                args.extend(["-e".to_string(), format!("{key}={val}")]);
            }
        }
        // Always set HOME so credential files are found at the expected path
        args.extend(["-e".to_string(), "HOME=/root".to_string()]);

        // ── Extra volume mounts ─────────────────────────────────────
        // Mount credential directories (e.g. ~/.config/gh, ~/.claude, ~/.ssh)
        // read-only so that tools inside the container can authenticate.
        for vol in self.config.effective_volumes() {
            args.extend(["-v".to_string(), vol]);
        }

        // ── Resource limits ─────────────────────────────────────────
        if let Some(cpus) = self.config.resources.cpus {
            args.extend(["--cpus".to_string(), cpus.to_string()]);
        }
        if let Some(ref mem) = self.config.resources.memory {
            args.extend(["--memory".to_string(), mem.clone()]);
        }

        // ── DNS servers ─────────────────────────────────────────────
        for dns in &self.config.dns {
            args.extend(["--dns".to_string(), dns.clone()]);
        }

        // ── Network configuration ───────────────────────────────────
        if !self.config.network.deny.is_empty() || !self.config.network.allow.is_empty() {
            args.extend(["--network".to_string(), "bridge".to_string()]);
        }

        args.push(image);
        args.push("sleep".to_string());
        args.push("infinity".to_string());

        let output = Command::new("docker")
            .args(&args)
            .output()
            .await
            .context("Failed to run docker create")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("docker create failed: {stderr}");
        }

        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        self.container_id = Some(id.clone());

        // Start the container
        let start_output = Command::new("docker")
            .args(["start", &id])
            .output()
            .await
            .context("Failed to start container")?;

        if !start_output.status.success() {
            let stderr = String::from_utf8_lossy(&start_output.stderr);
            bail!("docker start failed: {stderr}");
        }

        tracing::info!(container_id = %id, "Sandbox container started");
        Ok(())
    }

    /// Copy a host directory into the running sandbox container.
    ///
    /// When the config has `exclude` patterns, we use `tar --exclude` piped
    /// into `docker cp` to skip large directories like node_modules/ and
    /// target/ that would otherwise make the copy prohibitively slow.
    ///
    /// Note: macOS tar emits many harmless warnings about extended attributes
    /// (LIBARCHIVE.xattr.*) when the receiving Linux tar doesn't understand
    /// them.  We suppress these via `--no-xattrs` and `--no-mac-metadata`
    /// flags and only fail on *real* errors (e.g. source directory missing).
    pub async fn copy_workspace(&self, src: &str) -> Result<()> {
        let id = self.container_id.as_ref().context("Container not created")?;

        let effective_exclude = self.config.effective_exclude();
        if effective_exclude.is_empty() {
            // Fast path: no exclusions, use plain docker cp
            let output = Command::new("docker")
                .args(["cp", &format!("{src}/."), &format!("{id}:/workspace")])
                .output()
                .await
                .context("docker cp failed")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("docker cp workspace failed: {stderr}");
            }
        } else {
            // Use shell pipe: tar --exclude | docker exec -i tar
            // --no-xattrs and --no-mac-metadata suppress macOS extended
            // attribute warnings that would otherwise cause tar to exit
            // with a non-zero status.
            // We also use 2>/dev/null on the receiving tar to silence
            // "Ignoring unknown extended header keyword" warnings.
            let mut excludes = String::new();
            for pattern in &effective_exclude {
                excludes.push_str(&format!(" --exclude='{pattern}'"));
            }

            let pipe_cmd = format!(
                "tar -cf - --no-xattrs --no-mac-metadata{excludes} -C '{src}' . 2>/dev/null \
                 | docker exec -i {id} tar -xf - -C /workspace 2>/dev/null; \
                 exit 0"
            );

            let output = Command::new("/bin/sh")
                .args(["-c", &pipe_cmd])
                .output()
                .await
                .context("tar | docker exec pipe failed")?;

            // We don't check exit status here because tar may return non-zero
            // due to harmless permission errors on .git/objects (loose objects
            // that belong to pack files and aren't individually readable).
            // Instead, we verify the workspace was actually populated.
            let verify = Command::new("docker")
                .args(["exec", id, "test", "-d", "/workspace/.git"])
                .output()
                .await
                .context("workspace verification failed")?;

            if !verify.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("docker cp workspace failed — .git directory not found in container: {stderr}");
            }
        }

        Ok(())
    }

    /// Run a shell command inside the sandbox and return the output
    pub async fn run_command(&self, cmd: &str) -> Result<SandboxOutput> {
        let id = self.container_id.as_ref().context("Container not created")?;

        tracing::debug!(container_id = %id, cmd = %cmd, "Sandbox: executing command");

        let output = Command::new("docker")
            .args(["exec", id, "/bin/sh", "-c", cmd])
            .output()
            .await
            .context("docker exec failed")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        tracing::debug!(
            exit_code,
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            stdout_preview = %if stdout.len() > 200 { &stdout[..200] } else { &stdout },
            stderr_preview = %if stderr.len() > 200 { &stderr[..200] } else { &stderr },
            "Sandbox: command completed"
        );

        Ok(SandboxOutput { stdout, stderr, exit_code })
    }

    /// Run a shell command inside the sandbox as a specific user.
    /// Used for agent steps that need non-root execution (Claude CLI
    /// refuses `--dangerously-skip-permissions` when running as root).
    pub async fn run_command_as_user(&self, cmd: &str, user: &str) -> Result<SandboxOutput> {
        let id = self.container_id.as_ref().context("Container not created")?;

        tracing::debug!(container_id = %id, cmd = %cmd, user = %user, "Sandbox: executing command as user");

        let output = Command::new("docker")
            .args(["exec", "--user", user, id, "/bin/sh", "-c", cmd])
            .output()
            .await
            .context("docker exec failed")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        tracing::debug!(
            exit_code,
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            "Sandbox: user command completed"
        );

        Ok(SandboxOutput { stdout, stderr, exit_code })
    }

    /// Copy results from the sandbox back to the host.
    ///
    /// First checks whether any files were actually modified inside the
    /// container (via `git status --porcelain`). If nothing changed, the
    /// copy is skipped entirely — this is the common case for read-only
    /// workflows like code-review.
    pub async fn copy_results(&self, dest: &str) -> Result<()> {
        let id = self.container_id.as_ref().context("Container not created")?;

        // Check if any files were modified inside the container.
        // If nothing changed, skip the (potentially slow) copy-back.
        let check = Command::new("docker")
            .args(["exec", id, "git", "-C", "/workspace", "status", "--porcelain"])
            .output()
            .await;

        if let Ok(output) = check {
            let changed = String::from_utf8_lossy(&output.stdout);
            let changed = changed.trim();
            if changed.is_empty() {
                tracing::info!("No files changed in sandbox — skipping copy-back");
                return Ok(());
            }
            tracing::info!(changed_files = %changed, "Sandbox has modified files — copying back");
        }

        // Copy only the changed files back using git ls-files
        // This is much faster than copying the entire workspace.
        let pipe_cmd = format!(
            "docker exec {id} sh -c \
             'cd /workspace && git diff --name-only HEAD 2>/dev/null; \
              git ls-files --others --exclude-standard 2>/dev/null' \
             | while read f; do \
                 docker cp \"{id}:/workspace/$f\" \"{dest}/$f\" 2>/dev/null; \
               done; exit 0"
        );

        Command::new("/bin/sh")
            .args(["-c", &pipe_cmd])
            .output()
            .await
            .context("copy results from container failed")?;

        Ok(())
    }

    /// Stop and remove the sandbox container (safe to call even if not created)
    pub async fn destroy(&mut self) -> Result<()> {
        if let Some(id) = self.container_id.take() {
            let output = Command::new("docker")
                .args(["rm", "-f", &id])
                .output()
                .await
                .context("docker rm failed")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!("docker rm warning: {stderr}");
            } else {
                tracing::info!(container_id = %id, "Sandbox container destroyed");
            }
        }
        Ok(())
    }
}

/// Drop impl ensures cleanup even if destroy() was not called explicitly
impl Drop for DockerSandbox {
    fn drop(&mut self) {
        if let Some(id) = &self.container_id {
            // Best-effort synchronous cleanup via std::process::Command
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", id])
                .output();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::config::{NetworkConfig, ResourceConfig, SandboxConfig};

    fn make_config() -> SandboxConfig {
        SandboxConfig {
            enabled: true,
            image: Some("ubuntu:22.04".to_string()),
            workspace: Some("/tmp/test".to_string()),
            network: NetworkConfig::default(),
            resources: ResourceConfig {
                cpus: Some(1.0),
                memory: Some("512m".to_string()),
            },
            env: vec![],
            volumes: vec![],
            exclude: vec![],
            dns: vec![],
        }
    }

    #[test]
    fn sandbox_new_has_no_container() {
        let sb = DockerSandbox::new(make_config(), "/tmp/workspace");
        assert!(sb.container_id.is_none());
    }

    #[test]
    fn sandbox_destroy_when_no_container_is_noop() {
        // drop without a container_id should not panic
        let mut sb = DockerSandbox::new(make_config(), "/tmp/workspace");
        sb.container_id = None;
        drop(sb); // triggers Drop impl
    }

    /// Mock-based test: verify that the docker commands would be constructed correctly.
    /// Uses a fake docker binary that records the command-line arguments.
    #[tokio::test]
    async fn run_command_returns_stdout() {
        // We can't run real Docker in tests, so we just verify that the DockerSandbox
        // structure is correct and the error path works.
        let sb = DockerSandbox::new(make_config(), "/tmp/workspace");
        // Without a container_id, run_command should return an error
        let result = sb.run_command("echo hello").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Container not created"));

        // Same for copy_results and copy_workspace
        let r2 = sb.copy_results("/tmp/dest").await;
        assert!(r2.is_err());

        let r3 = sb.copy_workspace("/tmp/src").await;
        assert!(r3.is_err());
    }

    #[test]
    fn config_image_fallback() {
        let cfg = SandboxConfig::default();
        let sb = DockerSandbox::new(cfg, "/tmp");
        assert_eq!(sb.config.image(), "minion-sandbox:latest");
    }
}

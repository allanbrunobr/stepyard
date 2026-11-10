//! Moving a project in and out of a session container.
//!
//! The harness copies the project into the container before the first step
//! that runs there, and copies the files that changed back to the host when
//! the workflow ends. Both directions go through the container CLI
//! (`docker` / `podman`), so they also work with a remote daemon, where a
//! bind mount would point at the wrong machine.

use crate::SandboxError;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

/// The CLI binary and container name for one session's container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerHandle {
    /// `docker` or `podman`.
    pub binary: String,
    /// Container name, `stepyard-session-<uuid>`.
    pub name: String,
}

/// A path that changed inside the container, relative to the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedPath {
    /// Added or modified: copy it to the host.
    Modified(String),
    /// Deleted: remove it on the host.
    Deleted(String),
}

/// What [`ContainerHandle::copy_out`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyOutReport {
    pub copied: Vec<String>,
    pub deleted: Vec<String>,
    /// Paths rejected because they are absolute or leave the workspace.
    pub skipped: Vec<String>,
}

impl ContainerHandle {
    /// Argv for running `program args…` in the container with stdin attached.
    ///
    /// Env values are not in the argv: each key is passed as `-e KEY`, so the
    /// CLI reads the value from its own environment. The caller sets those
    /// values on the CLI process. This keeps secrets out of `ps` output.
    ///
    /// With `user`, `HOME` is set to `/home/<user>`: the image sets
    /// `HOME=/root`, and `docker exec --user` keeps it, so tools that write
    /// under `$HOME` (the Claude CLI does) would fail with `EACCES`. A `HOME`
    /// key in `env_keys` is dropped in that case for the same reason.
    pub fn exec_argv(
        &self,
        user: Option<&str>,
        workdir: &str,
        env_keys: &[String],
        program: &str,
        args: &[String],
    ) -> Vec<String> {
        let mut argv = vec!["exec".to_string(), "-i".to_string()];
        if let Some(user) = user {
            argv.push("--user".into());
            argv.push(user.into());
        }
        argv.push("-w".into());
        argv.push(workdir.into());
        if let Some(user) = user {
            argv.push("-e".into());
            argv.push(format!("HOME=/home/{user}"));
        }
        let mut keys: Vec<&String> = env_keys
            .iter()
            .filter(|k| user.is_none() || k.as_str() != "HOME")
            .collect();
        keys.sort();
        for key in keys {
            argv.push("-e".into());
            argv.push(key.clone());
        }
        argv.push(self.name.clone());
        argv.push(program.into());
        argv.extend(args.iter().cloned());
        argv
    }

    /// Run `sh -c script` in the container as its default user.
    pub async fn sh(&self, script: &str) -> Result<std::process::Output, SandboxError> {
        Command::new(&self.binary)
            .args(["exec", &self.name, "sh", "-c", script])
            .output()
            .await
            .map_err(|e| SandboxError::ExecFailed(format!("{} exec: {e}", self.binary)))
    }

    /// Copy `host_dir` into `container_dir`, skipping `exclude` patterns
    /// (tar `--exclude` syntax, matched against any path component).
    ///
    /// The archive is streamed from a host `tar` into `tar -x` in the
    /// container. Files get the container user's ownership
    /// (`--no-same-owner`), not the host UID.
    pub async fn copy_in(
        &self,
        host_dir: &Path,
        container_dir: &str,
        exclude: &[String],
    ) -> Result<(), SandboxError> {
        let mut tar = Command::new("tar");
        // macOS: no AppleDouble `._*` files in the archive.
        tar.env("COPYFILE_DISABLE", "1");
        tar.arg("-cf").arg("-");
        if cfg!(target_os = "macos") {
            tar.args(["--no-mac-metadata", "--no-xattrs"]);
        }
        for pattern in exclude {
            tar.arg(format!("--exclude={pattern}"));
        }
        tar.arg("-C").arg(host_dir).arg(".");
        tar.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut tar_child = tar
            .spawn()
            .map_err(|e| SandboxError::ExecFailed(format!("tar: {e}")))?;
        let tar_stdout: Stdio = tar_child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::ExecFailed("tar stdout pipe missing".into()))?
            .try_into()
            .map_err(|e| SandboxError::ExecFailed(format!("tar stdout pipe: {e}")))?;

        let script = format!(
            "mkdir -p {dir} && tar -xf - --no-same-owner -C {dir}",
            dir = shell_quote(container_dir)
        );
        let extract = Command::new(&self.binary)
            .args(["exec", "-i", &self.name, "sh", "-c", &script])
            .stdin(tar_stdout)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        let (tar_out, extract_out) = tokio::join!(tar_child.wait_with_output(), extract);
        let extract_out = extract_out
            .map_err(|e| SandboxError::ExecFailed(format!("{} exec tar: {e}", self.binary)))?;
        if !extract_out.status.success() {
            return Err(SandboxError::ExecFailed(format!(
                "copying the project into the container failed: {}",
                String::from_utf8_lossy(&extract_out.stderr).trim()
            )));
        }
        match tar_out {
            Ok(out) if out.status.success() => {}
            // Unreadable files make tar exit non-zero after writing
            // everything else. The caller verifies the result.
            Ok(out) => tracing::warn!(
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "tar reported errors while archiving the project"
            ),
            Err(e) => return Err(SandboxError::ExecFailed(format!("tar: {e}"))),
        }
        Ok(())
    }

    /// Files that differ from `HEAD` in `container_dir`, including untracked
    /// files that `.gitignore` does not exclude.
    pub async fn changed_paths(
        &self,
        container_dir: &str,
    ) -> Result<Vec<ChangedPath>, SandboxError> {
        let script = format!(
            "git -C {} status --porcelain=v1 -z --untracked-files=all --no-renames",
            shell_quote(container_dir)
        );
        let out = self.sh(&script).await?;
        if !out.status.success() {
            return Err(SandboxError::ExecFailed(format!(
                "git status in the container failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(parse_porcelain_z(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Copy `changes` from `container_dir` to `host_dir`: modified files are
    /// copied, deleted files are removed. Paths that are absolute or contain
    /// `..` are skipped. A symlink, or a path that walks through one, is an
    /// error: `docker cp` would otherwise plant that link on the host.
    pub async fn copy_out(
        &self,
        container_dir: &str,
        changes: &[ChangedPath],
        host_dir: &Path,
    ) -> Result<CopyOutReport, SandboxError> {
        let mut report = CopyOutReport::default();
        for change in changes {
            let (ChangedPath::Modified(rel) | ChangedPath::Deleted(rel)) = change;
            let Some(dest) = safe_join(host_dir, rel) else {
                report.skipped.push(rel.clone());
                continue;
            };
            if rel.contains('\0') || rel.contains('\n') || host_has_symlink_parent(host_dir, rel) {
                return Err(SandboxError::ExecFailed(format!(
                    "refusing to copy {rel}: a parent on the host is a symlink or the path contains a control character"
                )));
            }
            match change {
                ChangedPath::Deleted(_) => match tokio::fs::remove_file(&dest).await {
                    Ok(()) => report.deleted.push(rel.clone()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(SandboxError::ExecFailed(format!(
                            "removing {} on the host: {e}",
                            dest.display()
                        )))
                    }
                },
                ChangedPath::Modified(_) => {
                    if host_final_is_symlink(host_dir, rel) {
                        return Err(SandboxError::ExecFailed(format!(
                            "refusing to copy {rel}: the host path is a symlink"
                        )));
                    }
                    self.ensure_container_path_stays_inside(container_dir, rel)
                        .await?;
                    if let Some(parent) = dest.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(|e| {
                            SandboxError::ExecFailed(format!(
                                "creating {} on the host: {e}",
                                parent.display()
                            ))
                        })?;
                    }
                    let src = format!(
                        "{}:{}/{}",
                        self.name,
                        container_dir.trim_end_matches('/'),
                        rel
                    );
                    let out = Command::new(&self.binary)
                        .arg("cp")
                        .arg(&src)
                        .arg(&dest)
                        .output()
                        .await
                        .map_err(|e| {
                            SandboxError::ExecFailed(format!("{} cp: {e}", self.binary))
                        })?;
                    if !out.status.success() {
                        return Err(SandboxError::ExecFailed(format!(
                            "copying {rel} out of the container failed: {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        )));
                    }
                    if host_final_is_symlink(host_dir, rel) {
                        let _ = tokio::fs::remove_file(&dest).await;
                        return Err(SandboxError::ExecFailed(format!(
                            "refusing to copy {rel}: the copied path is a symlink"
                        )));
                    }
                    report.copied.push(rel.clone());
                }
            }
        }
        Ok(report)
    }

    /// Refuse a container path that is a symlink or that walks through one.
    ///
    /// The relative path is a positional argument of a fixed script. It is
    /// not interpolated into the script text.
    async fn ensure_container_path_stays_inside(
        &self,
        container_dir: &str,
        rel: &str,
    ) -> Result<(), SandboxError> {
        const SCRIPT: &str = r#"
container=$1
rel=$2
case $container in
  /*) ;;
  *) echo "container workspace must be absolute" >&2; exit 2 ;;
esac
case $rel in
  ""|/*) echo "refusing path" >&2; exit 3 ;;
esac
if [ -L "$container" ]; then echo "workspace is a symlink" >&2; exit 4; fi
cur=$container
rest=$rel
while [ -n "$rest" ]; do
  part=${rest%%/*}
  if [ "$rest" = "$part" ]; then
    rest=
  else
    rest=${rest#*/}
  fi
  case $part in
    ""|.) continue ;;
    ..) echo "refusing parent segment" >&2; exit 3 ;;
  esac
  cur=$cur/$part
  if [ -L "$cur" ]; then echo "refusing symlink" >&2; exit 4; fi
  if [ ! -e "$cur" ]; then echo "path is missing in the container" >&2; exit 5; fi
done
exit 0
"#;
        let out = Command::new(&self.binary)
            .args([
                "exec",
                &self.name,
                "sh",
                "-c",
                SCRIPT,
                "sh",
                container_dir,
                rel,
            ])
            .output()
            .await
            .map_err(|e| SandboxError::ExecFailed(format!("{} exec: {e}", self.binary)))?;
        if !out.status.success() {
            return Err(SandboxError::ExecFailed(format!(
                "refusing to copy {rel}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }
}

/// Parse `git status --porcelain=v1 -z --no-renames` output.
pub fn parse_porcelain_z(out: &str) -> Vec<ChangedPath> {
    out.split('\0')
        .filter(|entry| entry.len() > 3)
        .filter_map(|entry| {
            let (status, path) = entry.split_at(3);
            let path = path.to_string();
            let mut chars = status.chars();
            let (x, y) = (chars.next()?, chars.next()?);
            if x == 'D' || y == 'D' {
                Some(ChangedPath::Deleted(path))
            } else {
                Some(ChangedPath::Modified(path))
            }
        })
        .collect()
}

/// `host_dir/rel`, or `None` if `rel` is absolute or leaves `host_dir`.
fn safe_join(host_dir: &Path, rel: &str) -> Option<PathBuf> {
    let rel_path = Path::new(rel);
    if rel.is_empty()
        || rel_path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    Some(host_dir.join(rel_path))
}

fn host_has_symlink_parent(host_dir: &Path, rel: &str) -> bool {
    let mut cur = host_dir.to_path_buf();
    let mut comps: Vec<_> = Path::new(rel).components().collect();
    comps.pop();
    for comp in comps {
        cur.push(comp);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
            Err(_) => return true,
        }
    }
    false
}

fn host_final_is_symlink(host_dir: &Path, rel: &str) -> bool {
    match std::fs::symlink_metadata(host_dir.join(rel)) {
        Ok(meta) => meta.file_type().is_symlink(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> ContainerHandle {
        ContainerHandle {
            binary: "docker".into(),
            name: "stepyard-session-x".into(),
        }
    }

    #[test]
    fn porcelain_parses_modified_untracked_and_deleted() {
        let out = " M src/a.rs\0?? new dir/b.txt\0 D gone.txt\0D  staged_gone.txt\0A  added.rs\0";
        assert_eq!(
            parse_porcelain_z(out),
            vec![
                ChangedPath::Modified("src/a.rs".into()),
                ChangedPath::Modified("new dir/b.txt".into()),
                ChangedPath::Deleted("gone.txt".into()),
                ChangedPath::Deleted("staged_gone.txt".into()),
                ChangedPath::Modified("added.rs".into()),
            ]
        );
    }

    #[test]
    fn porcelain_empty_output_has_no_changes() {
        assert!(parse_porcelain_z("").is_empty());
    }

    #[test]
    fn safe_join_rejects_paths_that_leave_the_workspace() {
        let root = Path::new("/project");
        assert_eq!(
            safe_join(root, "src/a.rs"),
            Some(PathBuf::from("/project/src/a.rs"))
        );
        assert_eq!(safe_join(root, "../etc/passwd"), None);
        assert_eq!(safe_join(root, "a/../../x"), None);
        assert_eq!(safe_join(root, "/etc/passwd"), None);
        assert_eq!(safe_join(root, ""), None);
    }

    #[tokio::test]
    async fn copy_out_skips_unsafe_paths_without_running_anything() {
        let dir = tempfile::tempdir().unwrap();
        let report = handle()
            .copy_out(
                "/workspace",
                &[
                    ChangedPath::Modified("../escape".into()),
                    ChangedPath::Deleted("/abs".into()),
                ],
                dir.path(),
            )
            .await
            .unwrap();
        assert_eq!(report.skipped, vec!["../escape", "/abs"]);
        assert!(report.copied.is_empty() && report.deleted.is_empty());
    }

    #[tokio::test]
    async fn copy_out_rejects_a_symlink_parent_on_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "keep").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        let err = handle()
            .copy_out(
                "/workspace",
                &[ChangedPath::Modified("escape/secret".into())],
                dir.path(),
            )
            .await
            .expect_err("symlink parent must be refused");
        assert!(err.to_string().contains("symlink"), "{err}");
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret")).unwrap(),
            "keep"
        );
    }

    #[tokio::test]
    async fn copy_out_rejects_a_symlink_file_on_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "keep").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("leak")).unwrap();
        let err = handle()
            .copy_out(
                "/workspace",
                &[ChangedPath::Modified("leak".into())],
                dir.path(),
            )
            .await
            .expect_err("symlink file must be refused");
        assert!(err.to_string().contains("symlink"), "{err}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        assert!(std::fs::symlink_metadata(dir.path().join("leak"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn exec_argv_without_user_keeps_home_from_the_step_env() {
        let argv = handle().exec_argv(None, "/workspace", &["HOME".into()], "sh", &[]);
        assert_eq!(
            argv,
            [
                "exec",
                "-i",
                "-w",
                "/workspace",
                "-e",
                "HOME",
                "stepyard-session-x",
                "sh"
            ]
        );
    }

    #[test]
    fn exec_argv_passes_env_keys_without_values() {
        let argv = handle().exec_argv(
            Some("stepyard"),
            "/workspace",
            &["ZED".into(), "HOME".into(), "ANTHROPIC_API_KEY".into()],
            "claude",
            &["-p".into()],
        );
        assert_eq!(
            argv,
            [
                "exec",
                "-i",
                "--user",
                "stepyard",
                "-w",
                "/workspace",
                "-e",
                "HOME=/home/stepyard",
                "-e",
                "ANTHROPIC_API_KEY",
                "-e",
                "ZED",
                "stepyard-session-x",
                "claude",
                "-p",
            ]
        );
    }
}

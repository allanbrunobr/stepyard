use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum SandboxRuntime {
    Docker,
    Local,
}

impl SandboxRuntime {
    pub fn profile_default() -> Self {
        if cfg!(feature = "sqlite") {
            Self::Local
        } else {
            Self::Docker
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("invalid STEPYARD_SANDBOX value `{0}` (expected `docker` or `local`)")]
    InvalidEnv(String),
}

pub fn resolve_runtime(
    cli_flag: Option<SandboxRuntime>,
    legacy_no_sandbox: bool,
) -> Result<SandboxRuntime, ResolveError> {
    resolve_runtime_inner(
        cli_flag,
        std::env::var("STEPYARD_SANDBOX").ok(),
        legacy_no_sandbox,
    )
}

pub(crate) fn resolve_runtime_inner(
    cli_flag: Option<SandboxRuntime>,
    env_value: Option<String>,
    legacy_no_sandbox: bool,
) -> Result<SandboxRuntime, ResolveError> {
    if let Some(runtime) = cli_flag {
        return Ok(runtime);
    }

    if let Some(raw) = env_value {
        return match raw.as_str() {
            "docker" => Ok(SandboxRuntime::Docker),
            "local" => Ok(SandboxRuntime::Local),
            other => Err(ResolveError::InvalidEnv(other.to_string())),
        };
    }

    if legacy_no_sandbox {
        return Ok(SandboxRuntime::Local);
    }

    Ok(SandboxRuntime::profile_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_flag_wins_over_env() {
        let got = resolve_runtime_inner(
            Some(SandboxRuntime::Local),
            Some("docker".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(got, SandboxRuntime::Local);
    }

    #[test]
    fn env_wins_over_legacy_no_sandbox() {
        let got = resolve_runtime_inner(None, Some("docker".to_string()), true).unwrap();
        assert_eq!(got, SandboxRuntime::Docker);
    }

    #[test]
    fn legacy_no_sandbox_maps_to_local() {
        let got = resolve_runtime_inner(None, None, true).unwrap();
        assert_eq!(got, SandboxRuntime::Local);
    }

    #[test]
    fn invalid_env_errors() {
        assert!(matches!(
            resolve_runtime_inner(None, Some("podman".to_string()), false),
            Err(ResolveError::InvalidEnv(_))
        ));
    }
}

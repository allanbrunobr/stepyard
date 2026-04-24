//! Docker CLI stderr → [`SandboxError`] classifier.
//!
//! Round 3 "`Other(String)` discipline" anchor — architecture.md §D9:
//! construction of [`SandboxError::Other`] is permitted ONLY inside this
//! module (the only Docker-domain classifier today; git and workspace
//! classifiers land in follow-up stories). Known-shape stderr classifies
//! to typed variants (`BackendUnavailable`, `CreateFailed`); genuinely
//! unrecognised stderr falls through to [`SandboxError::Other`] with the
//! raw bytes preserved UTF-8-lossy so the diagnostic survives in the
//! stored error — display-boundary sanitization happens elsewhere, at
//! the CLI error printer via [`crate::display::sanitize_human`] in the
//! binary crate (wired at `src/main.rs`).
//!
//! Input type is `&[u8]` because `tokio::process::Command::output()`
//! returns raw bytes: docker's stderr is not guaranteed UTF-8, and the
//! classifier is the single conversion point — [`String::from_utf8_lossy`]
//! lives here and nowhere else.
//!
//! Only exit-code-nonzero stderr routes through this module. IO-layer
//! failures (can't spawn the docker binary, pipe errors) stay on the
//! existing `SandboxError::BackendUnavailable(e.to_string())` /
//! `SandboxError::ExecFailed(e.to_string())` paths at the call sites —
//! those are typed `std::io::Error` values, not docker output.

use crate::sandbox::SandboxError;

/// 8 KiB ceiling on any raw-stderr payload the classifier stores in a
/// `SandboxError` variant. Matches the "Stored form" clause of the
/// `Other(String)` discipline spec. Truncation snaps to a UTF-8 char
/// boundary so the stored string stays valid for `Display`.
const MAX_STORED_BYTES: usize = 8 * 1024;

/// Classify stderr from a non-zero-exit `docker run` (or equivalent
/// "create a container" subprocess call).
pub(crate) fn classify_create_stderr(stderr: &[u8]) -> SandboxError {
    let raw = lossy_truncated(stderr);
    if is_daemon_unreachable(&raw) {
        return SandboxError::BackendUnavailable(raw);
    }
    if is_image_missing(&raw) {
        return SandboxError::CreateFailed(raw);
    }
    SandboxError::Other(raw)
}

/// Classify stderr from a non-zero-exit `docker rm` subprocess call.
///
/// Returns `None` for the "No such container" idempotent-swallow case —
/// `SandboxLifecycle::destroy*` is contractually idempotent and trying
/// to remove a container that is already gone is not an error.
pub(crate) fn classify_destroy_stderr(stderr: &[u8]) -> Option<SandboxError> {
    let raw = lossy_truncated(stderr);
    if raw.contains("No such container") {
        return None;
    }
    if is_daemon_unreachable(&raw) {
        return Some(SandboxError::BackendUnavailable(raw));
    }
    Some(SandboxError::Other(raw))
}

fn is_daemon_unreachable(s: &str) -> bool {
    s.contains("Cannot connect to the Docker daemon") || s.contains("Is the docker daemon running")
}

fn is_image_missing(s: &str) -> bool {
    // `docker run` against an unknown/private image prints one of these
    // two lead-ins depending on whether the pull attempt happened.
    s.contains("Unable to find image") || s.contains("pull access denied")
}

/// Convert raw stderr bytes to a trimmed, 8 KiB-bounded UTF-8 string.
///
/// The classifier is the only place in the crate that owns the
/// lossy-UTF-8 conversion. Truncation happens AFTER the conversion so
/// the `MAX_STORED_BYTES` limit applies to the UTF-8 byte length of the
/// final string (replacement chars included), and the char-boundary
/// snap keeps the string a valid `str`.
fn lossy_truncated(stderr: &[u8]) -> String {
    let decoded = String::from_utf8_lossy(stderr);
    let trimmed = decoded.trim();
    if trimmed.len() <= MAX_STORED_BYTES {
        return trimmed.to_string();
    }
    let mut end = MAX_STORED_BYTES;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_daemon_unreachable_maps_to_backend_unavailable() {
        let err = classify_create_stderr(
            b"Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
        );
        match err {
            SandboxError::BackendUnavailable(raw) => {
                assert!(raw.contains("Cannot connect"));
            }
            other => panic!("expected BackendUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn create_image_missing_maps_to_create_failed() {
        let err = classify_create_stderr(b"Unable to find image 'missing:latest' locally");
        match err {
            SandboxError::CreateFailed(raw) => {
                assert!(raw.contains("Unable to find image"));
            }
            other => panic!("expected CreateFailed, got {other:?}"),
        }
    }

    #[test]
    fn create_pull_denied_maps_to_create_failed() {
        let err = classify_create_stderr(
            b"docker: Error response from daemon: pull access denied for private/img, repository does not exist.",
        );
        match err {
            SandboxError::CreateFailed(raw) => {
                assert!(raw.contains("pull access denied"));
            }
            other => panic!("expected CreateFailed, got {other:?}"),
        }
    }

    #[test]
    fn create_unknown_stderr_maps_to_other_preserving_raw() {
        // Required test: unknown stderr falls into `Other(raw)` with the
        // bytes converted via from_utf8_lossy — the classifier is the
        // only construction site for `Other`, and the raw diagnostic
        // must survive verbatim for operator triage.
        let err = classify_create_stderr(b"something weird we've never seen before");
        match err {
            SandboxError::Other(raw) => {
                assert_eq!(raw, "something weird we've never seen before");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn create_non_utf8_stderr_survives_via_lossy_conversion() {
        // Required test: byte-sequence with invalid UTF-8 bytes must not
        // panic; from_utf8_lossy replaces bad subsequences with U+FFFD
        // and the surrounding text is preserved.
        let err = classify_create_stderr(&[
            b'd', b'o', b'c', b'k', b'e', b'r', b':', b' ', 0xFF, 0xFE, b' ', b'b', b'a', b'd',
        ]);
        match err {
            SandboxError::Other(raw) => {
                assert!(raw.starts_with("docker:"));
                assert!(raw.ends_with(" bad"));
                assert!(raw.contains('\u{FFFD}'));
            }
            other => panic!("expected Other for lossy input, got {other:?}"),
        }
    }

    #[test]
    fn destroy_no_such_container_returns_none_for_idempotency() {
        // Preserves the legacy inline check at docker.rs:138 — removing
        // an already-gone container is not an error because
        // `SandboxLifecycle::destroy*` is contractually idempotent.
        let out = classify_destroy_stderr(b"Error: No such container: minion-session-abc123");
        assert!(out.is_none(), "expected idempotent swallow, got {out:?}");
    }

    #[test]
    fn destroy_daemon_unreachable_maps_to_backend_unavailable() {
        let err = classify_destroy_stderr(
            b"Cannot connect to the Docker daemon at unix:///var/run/docker.sock.",
        );
        match err {
            Some(SandboxError::BackendUnavailable(raw)) => {
                assert!(raw.contains("Cannot connect"));
            }
            other => panic!("expected BackendUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn destroy_unknown_stderr_maps_to_other_preserving_raw() {
        let err = classify_destroy_stderr(b"unexpected teardown failure from daemon");
        match err {
            Some(SandboxError::Other(raw)) => {
                assert_eq!(raw, "unexpected teardown failure from daemon");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn stored_form_truncates_at_8kib_at_char_boundary() {
        // The Other-discipline spec caps stored payloads at 8 KiB so a
        // runaway stderr (e.g. a gigabyte of compile noise) cannot bloat
        // the error value. Truncation must land on a UTF-8 char
        // boundary to keep the stored String valid.
        let big = vec![b'x'; 10_000];
        let err = classify_create_stderr(&big);
        match err {
            SandboxError::Other(raw) => {
                assert!(
                    raw.len() <= MAX_STORED_BYTES,
                    "stored form exceeded 8 KiB: {}",
                    raw.len()
                );
                assert!(raw.starts_with("xxxx"));
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn stored_form_snaps_before_multibyte_at_limit() {
        // Place a 2-byte char exactly at the 8192-byte mark: 8191 'a's
        // then 'é' (0xC3 0xA9). Naive slicing at 8192 would split the
        // scalar; the classifier must snap back to byte 8191.
        let mut bytes: Vec<u8> = vec![b'a'; 8_191];
        bytes.extend_from_slice("é".as_bytes()); // pushes across the 8192 boundary
        bytes.extend(std::iter::repeat_n(b'b', 100));
        let err = classify_create_stderr(&bytes);
        match err {
            SandboxError::Other(raw) => {
                assert_eq!(raw.len(), 8_191, "expected snap at 8191, got {}", raw.len());
                assert!(raw.ends_with('a'));
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn whitespace_is_trimmed_from_stored_form() {
        // Docker tends to append a trailing newline; a leading-whitespace
        // prefix occasionally shows up too. Trim before storage so the
        // classifier output matches the patterns operators expect to see.
        let err = classify_create_stderr(b"  \n\nsomething weird\n");
        match err {
            SandboxError::Other(raw) => {
                assert_eq!(raw, "something weird");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }
}

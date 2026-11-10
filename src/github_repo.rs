//! `OWNER/REPO` as accepted by `git clone` argv.
//!
//! The character set matches the dashboard dispatch check (`[\w.-]` in
//! JavaScript, which is ASCII). Two extra refusals are intentional: a
//! component that is `.` or `..`, and a component that starts with `-`.
//! The dashboard regex allows both. Neither can break a quoted shell
//! string, and both are rejected here so a repo value cannot be a path
//! segment or a git option.

/// Maximum `owner/name` length. GitHub's own limits are 39 and 100.
const MAX_LEN: usize = 140;

pub fn parse_github_repo(input: &str) -> Result<&str, &'static str> {
    if input.len() > MAX_LEN {
        return Err("--repo must be OWNER/REPO using only ASCII letters, digits, '.', '_' and '-'");
    }
    let Some((owner, name)) = input.split_once('/') else {
        return Err("--repo must be OWNER/REPO using only ASCII letters, digits, '.', '_' and '-'");
    };
    if !is_component(owner) || !is_component(name) || name.contains('/') {
        return Err("--repo must be OWNER/REPO using only ASCII letters, digits, '.', '_' and '-'");
    }
    Ok(input)
}

fn is_component(part: &str) -> bool {
    if part.is_empty() || part == "." || part == ".." || part.starts_with('-') {
        return false;
    }
    part.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_normal_owner_and_name() {
        assert_eq!(parse_github_repo("acme/backend").unwrap(), "acme/backend");
        assert!(parse_github_repo("a.b_c-d/E1").is_ok());
    }

    #[test]
    fn rejects_shell_metacharacters_and_path_segments() {
        for bad in [
            "o/r;id",
            "owner/repo; echo STEPYARD_INJECT_MARKER; #",
            "owner/repo && id",
            "o/r id",
            "../etc",
            "owner/..",
            "-owner/name",
            "owner/-name",
            "owner/name/extra",
            "",
            "owner",
            "/owner/name",
        ] {
            assert!(parse_github_repo(bad).is_err(), "{bad}");
        }
    }
}

use std::fmt;
use std::str::FromStr;

use stepyard_harness::Workflow;
use stepyard_sandbox_orchestrator::BranchStrategy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliBranchStrategy(BranchStrategy);

impl CliBranchStrategy {
    pub fn into_branch_strategy(self) -> BranchStrategy {
        self.0
    }
}

impl FromStr for CliBranchStrategy {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "head" => Ok(Self(BranchStrategy::Head)),
            "merge-to-head" => Ok(Self(BranchStrategy::MergeToHead {
                target: "main".to_string(),
            })),
            value => {
                let Some(name) = value.strip_prefix("named-branch:") else {
                    return Err("expected head, merge-to-head, or named-branch:<name>".to_string());
                };
                if name.is_empty() {
                    return Err("named-branch requires a non-empty branch name".to_string());
                }
                Ok(Self(BranchStrategy::NamedBranch {
                    name: name.to_string(),
                }))
            }
        }
    }
}

impl fmt::Display for CliBranchStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            BranchStrategy::Head => f.write_str("head"),
            BranchStrategy::MergeToHead { .. } => f.write_str("merge-to-head"),
            BranchStrategy::NamedBranch { name } => write!(f, "named-branch:{name}"),
            _ => f.write_str("<unknown>"),
        }
    }
}

pub fn parse_cli_branch_strategy(input: &str) -> Result<CliBranchStrategy, String> {
    input.parse()
}

pub fn apply_cli_override(workflow: &mut Workflow, strategy: Option<CliBranchStrategy>) {
    if let Some(strategy) = strategy {
        workflow.set_branch_strategy(strategy.into_branch_strategy());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_head() {
        let parsed = parse_cli_branch_strategy("head")
            .unwrap()
            .into_branch_strategy();
        assert!(matches!(parsed, BranchStrategy::Head));
    }

    #[test]
    fn parses_merge_to_head_with_main_default() {
        let parsed = parse_cli_branch_strategy("merge-to-head")
            .unwrap()
            .into_branch_strategy();
        assert!(matches!(
            parsed,
            BranchStrategy::MergeToHead { ref target } if target == "main"
        ));
    }

    #[test]
    fn parses_named_branch() {
        let parsed = parse_cli_branch_strategy("named-branch:feat/foo")
            .unwrap()
            .into_branch_strategy();
        assert!(matches!(
            parsed,
            BranchStrategy::NamedBranch { ref name } if name == "feat/foo"
        ));
    }

    #[test]
    fn rejects_invalid_formats() {
        for input in [
            "",
            "foo",
            "merge_to_head",
            "named_branch:feat/foo",
            "named-branch:",
        ] {
            assert!(
                parse_cli_branch_strategy(input).is_err(),
                "{input:?} should reject"
            );
        }
    }

    #[test]
    fn cli_override_wins_over_invalid_yaml_branch_fields() {
        let mut workflow = Workflow::new("override", vec![]);
        workflow.branch_strategy = stepyard_harness::BranchStrategyYaml::NamedBranch;
        assert!(workflow.resolve_branch_strategy().is_err());

        apply_cli_override(
            &mut workflow,
            Some(parse_cli_branch_strategy("head").unwrap()),
        );
        assert!(matches!(
            workflow.resolve_branch_strategy().unwrap(),
            BranchStrategy::Head
        ));
    }

    mod proptest_tests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn arbitrary_inputs_never_panic(input in ".*") {
                let _ = parse_cli_branch_strategy(&input);
            }
        }
    }
}

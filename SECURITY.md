# Security

Stepyard is alpha software. A workflow file, a Slack message, and `--repo`
can start commands. The default engine copies a project into a container
and copies changes back.

## Reporting

Do not open a public issue for a vulnerability. Contact the maintainer
privately through GitHub first:

https://github.com/allanbrunobr/stepyard/security

## What this tree does not claim

`config.global.sandbox.network.allow` and `deny` do not filter egress. A
non-empty list only selects the bridge network. See `docs/DOCKER-SANDBOX.md`.

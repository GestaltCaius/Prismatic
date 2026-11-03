# 0008. Dependencies are pinned, delayed and scanned

Status: accepted

## Decision
Every dependency is pinned to an exact version, so nothing changes unless a pull request says so. Dependabot opens
those pull requests, grouped, and waits seven days after a release before proposing it (security updates are not
delayed). Every pull request, and a weekly run, checks `Cargo.lock` with cargo-deny (advisories, licenses, banned
crates, sources) and OSV-Scanner, and the workflow files with zizmor. An accepted advisory is listed in the
scanner's config with the reason, so anything new still fails. GitHub Actions are pinned by commit hash.
Dependabot alerts and private vulnerability reporting are on.

## Forbidden
- A version range, `*` or `latest` for a dependency or an action.
- Ignoring an advisory without a written reason.

## Revisit when
The pins cost more in pull requests than they save, or a dependency is dropped because its notices cannot be fixed.

## Cost if wrong
Fixes arrive a week late, and a vulnerability in an ignored advisory goes unseen until the entry is removed.

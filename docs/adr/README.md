# Decisions

Why Prismatic is built the way it is. Each record states the rule, what it forbids, the one condition that
would reopen it, and what it costs if it turns out wrong. They hold policy only: no file paths, and not the
story of how the decision was reached (the git history has that). Where code lives is in `AGENTS.md`.

A change that contradicts a record needs a new record that supersedes it, in the same pull request.

| # | Decision |
|---|---|
| [0001](0001-disable-by-renaming-the-folder.md) | A mod is disabled by a dot in front of its folder name |
| [0002](0002-dependencies-drive-enable-and-disable.md) | Dependencies decide what else is enabled or disabled |
| [0003](0003-missing-dependencies-are-offered.md) | Missing dependencies are offered, never installed unasked |
| [0004](0004-nexus-is-used-like-a-browser.md) | Nexus Mods is used like a browser, with the user's session cookie |
| [0005](0005-updates-keep-the-old-mod.md) | Updates keep the old mod and its config |
| [0006](0006-the-web-ui-is-local-and-guarded.md) | The web UI is local, guarded and runs one job at a time |
| [0007](0007-smapi-is-a-hint.md) | SMAPI's database is a hint, not a requirement |
| [0008](0008-dependencies-are-pinned-and-scanned.md) | Dependencies are pinned, delayed and scanned |
| [0009](0009-release-builds.md) | Release builds are native, old on Linux and scanned before publishing |
| [0010](0010-small-binary-strict-lints.md) | Small binary, strict lints |
| [0011](0011-releases-come-from-release-please.md) | Releases come from release-please |

## Format
Status, Decision, Forbidden, Revisit when (one line), Cost if wrong.

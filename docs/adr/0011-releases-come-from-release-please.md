# 0011. Releases come from release-please

Status: accepted

## Decision
Merging a pull request to `master` lets release-please keep one release pull request open: the version bump in
`Cargo.toml` and `Cargo.lock` and the new `CHANGELOG.md` section, written from the conventional commit titles
since the last release. Merging that pull request creates the tag and the GitHub Release with the changelog as
its notes. The tag starts the release workflow, which builds, scans and attaches the binaries (ADR 0009).

Pull request titles are the input: squash merging makes them the commit subjects. `feat`, `fix`, `perf` and
`docs` show in the changelog, the other types are hidden. While the version is below 1.0 a breaking change
bumps the minor version.

release-please acts through a token minted per run from a GitHub App, limited to this repository and to contents,
issues and pull requests (write). The default workflow token is not used: what it creates starts no workflows, so
the release pull request would get none of the required checks and the tag would never be built.

## Forbidden
- Tagging, or editing the version, by hand outside the release pull request.
- A long-lived personal access token in place of the app.
- Giving the app, or the token, more permissions than those three.
- A pull request title that is not a conventional commit. A wrong one is left out of the changelog and cannot be
  fixed once it is merged.

## Revisit when
Releases need another rhythm than "merge the release pull request", or the project starts publishing to crates.io.

## Cost if wrong
If the app's key leaks or expires, release pull requests and tags stop until the key is replaced, and a leaked key
can write to every repository the app is installed on. A bad tag cannot be moved or deleted without the admin
bypass on the tag ruleset.

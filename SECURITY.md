# Security policy

## Supported versions
Only the latest release (and `master`) gets security fixes.

## Reporting a vulnerability
Please report it privately, not in a public issue:
[Report a vulnerability](https://github.com/GestaltCaius/Prismatic/security/advisories/new)
(Security tab, "Report a vulnerability"). Say what you found, how to reproduce it and which version you used.
I will answer as soon as I can, and credit you in the fix unless you prefer not to be named.

## In scope
Prismatic runs a small web server on your own machine, keeps your Nexus session cookie in memory and unpacks
downloaded archives into your mods folder. Reports about those are the most useful, for example:
- a website or another local program getting the server to act (it only listens on `127.0.0.1` and wants a random token)
- the cookie being written to disk, logged or sent anywhere except nexusmods.com
- an archive writing outside the mods folder

## Dependencies
Known vulnerabilities in dependencies are checked on every pull request and every week with `cargo-deny` and
OSV-Scanner, and by Dependabot. The few accepted ones are listed with their reasons in `osv-scanner.toml` and `deny.toml`.
Release binaries are built with `cargo-auditable`, so they can be scanned too, see the Security section of the README.

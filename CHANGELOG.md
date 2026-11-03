# Changelog

## 0.1.0 (2026-11-03)

First release.

### Features

* **Command line and web interface.** One core, two front ends: `prismatic` opens a local web page, or use the commands in a terminal.
* **Install from Nexus Mods** with a page URL (`prismatic add`), using your signed-in session cookie. Missing dependencies are offered, never installed unasked.
* **Update mods** with `prismatic update`. SMAPI's database finds newer versions and sources, the previous version is kept in `.old` and config files are copied across.
* **Disable and enable mods** by putting a dot in front of the folder name, which SMAPI ignores. Mods that need a disabled mod go with it. With no name, a list lets you choose.
* **Back up and share your mod list** with `export` and `import`, for co-op.
* **Web interface** with a Mods and a Settings tab, a mods folder chooser with Steam and GOG defaults, a Nexus cookie panel, author links and a missing-dependencies panel.
* **Binaries** for Linux, macOS and Windows, on x86-64 and ARM64.

### Security

* The web server listens on `127.0.0.1` only and needs a launch token and a loopback `Host`.
* The Nexus cookie is never written to disk by Prismatic, logged or exported.
* Archive entries cannot write outside the mods folder.
* Dependencies are pinned and checked with cargo-deny and OSV-Scanner. Release binaries are built with cargo-auditable, scanned before publishing, and ship with a CycloneDX SBOM and `SHA256SUMS`.

### Documentation

* README with a screenshot and command examples, `SECURITY.md`, and decision records in `docs/adr`.

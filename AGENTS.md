# Notes for agents and new contributors

Prismatic is a Rust CLI and a local web UI for managing Stardew Valley mods. Read `docs/adr/README.md` before
changing behaviour: the records there say what was decided, what is forbidden and when to reopen it. A change
that contradicts one needs a new record that supersedes it, in the same pull request.

## Where things live
This is the only file that names paths, so a refactor changes one place.

The library (`src/lib.rs`) is the core. `Manager` in `src/manager.rs` is the API that both front ends call, so
behaviour belongs there and not in `src/cli.rs` or `src/ui/`.

| File | What it holds |
|---|---|
| `src/manifest.rs` | Reading mod manifests, the enable and disable cascade, missing dependencies |
| `src/install.rs` | Extracting archives, replacing a mod, renaming folders (blocking, run it with `spawn_blocking`) |
| `src/manager.rs` | Orchestration: updates, installs, SMAPI id cache and cooldown |
| `src/nexus.rs`, `src/github.rs`, `src/http.rs` | Downloading, the Nexus session cookie, the browser-like HTTP client |
| `src/smapi.rs` | Asking SMAPI's mod database for Nexus ids and newest versions |
| `src/modlist.rs` | The export and import file |
| `src/paths.rs` | Where the Mods folder is by default |
| `src/progress.rs` | Progress events both front ends show |
| `src/cli.rs`, `src/main.rs` | The command line |
| `src/ui/` | The web server: `mod.rs` setup, `guard.rs` request check, `api.rs` routes, `jobs.rs` job list, `ui.html` the whole page |
| `tests/cli.rs` | Runs the real binary against throwaway folders, no network |
| `DESIGN.md` | The web UI's look and its components |

## Commands
- `mise run check`: format check, clippy, build and tests. Run it before pushing.
- `cargo test`: the tests need no network, those that do are `#[ignore]` (run them with `-- --ignored`).
- CI also runs `cargo deny check`, OSV-Scanner and zizmor, see `.github/workflows/ci.yml`.

## Conventions
- Warnings are errors, and clippy runs with `pedantic` and `nursery`. `unwrap` is denied outside tests, `unsafe` is forbidden.
- Comments say why, not what, and match the density of the code around them.
- Dependencies are pinned with `=`. Do not loosen a pin, and give a reason when you ignore an advisory.
- Folder handling and cookie reading differ per OS, and CI runs the tests on Linux, macOS and Windows. Keep a change there testable on all three.

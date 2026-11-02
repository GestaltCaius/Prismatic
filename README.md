# Prismatic
A CLI tool for installing and updating Stardew Valley mods.

## Installation
Download the archive for your platform from the
[releases page](https://github.com/GestaltCaius/Prismatic/releases), unpack it and put `prismatic` somewhere on your `PATH`.

Or build it with Rust: `cargo install --git https://github.com/GestaltCaius/Prismatic`

On macOS a downloaded binary is quarantined, run `xattr -d com.apple.quarantine ./prismatic` once.

## Usage
To update all mods simply run `prismatic update`.
Without `-d`, the first of these folders that exists is used (Steam first, then GOG):

| System | Locations, relative to your home folder unless absolute |
|---|---|
| Linux | `.steam/steam/steamapps/common/Stardew Valley/Mods`, `.local/share/Steam/steamapps/common/Stardew Valley/Mods`, `.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common/Stardew Valley/Mods` (Flatpak), `GOG Games/Stardew Valley/game/Mods` |
| macOS | `Library/Application Support/Steam/steamapps/common/Stardew Valley/Contents/MacOS/Mods`, `/Applications/Stardew Valley.app/Contents/MacOS/Mods` |
| Windows | `C:\Program Files (x86)\Steam\steamapps\common\Stardew Valley\Mods`, the same under `C:\Program Files\Steam`, `C:\Program Files (x86)\GOG Galaxy\Games\Stardew Valley\Mods`, `C:\GOG Games\Stardew Valley\Mods` |

If Stardew Valley is installed somewhere else (a second Steam library, say) specify the folder with `-d <dir>`
(e.g. `prismatic -d <dir> update`)

| Command | What it does |
|---|---|
| `prismatic` or `prismatic ui` | Opens the [web interface](#web-interface) |
| `prismatic list` | Lists installed mods |
| `prismatic add <nexus url>` | Installs a mod from a Nexus Mods page, e.g. `https://www.nexusmods.com/stardewvalley/mods/3109` |
| `prismatic update` | Updates installed mods if needed (`--force` skips the check) |
| `prismatic export [file]` | Writes your mod list to `prismatic-mods.json` (or `file`) |
| `prismatic import <file>` | Installs the mods listed in a file made by `export` |

The exit code is 0 on success and 1 on any error, or when an update or import could not do all it was asked to.
Mods without a usable update source are reported by `update` but don't count as failures.
A mod whose `manifest.json` can't be read is skipped with a warning, the others still work.

Mods whose manifest has no usable update key (or a placeholder such as `Nexus:???`) are looked up on SMAPI's mod database
by their `UniqueID`, so `update` and the web interface can often still update them. A mod SMAPI doesn't know takes the
source its siblings agree on (same pack folder, same author in the `UniqueID`). Mods of one pack share a download,
and updating one of them replaces the whole pack folder.

### Web interface
Running `prismatic` with no command (or double-clicking it on Windows) starts a small server on your own machine and opens
it in your browser. It can list installed mods, check for and install updates, add a mod from a Nexus page, and export or
import mod lists, with the same checks the commands use. Authors link to their Nexus profile. Paste your Nexus cookie once in the page, it is kept in your browser.
If no mods folder is found the page asks for one, and the "Mods folder" panel can change it at any time (not while a job
runs). Type the full path, `~` stands for your home folder. The choice is remembered in the browser and used the next
time, unless the program is started with `-d`, which always wins. Note that the page can then install into any existing
folder you give it, like the commands can with `-d`.
Add `-d <dir>` before `ui` for a non-default mods directory, `--port <n>` to change the port (47821 by default,
a free one is used if it is taken) and `--no-open` to only print the address.

The server only listens on `127.0.0.1`, and every request needs the random token that is part of the printed address,
which stops other websites from using it to install mods on your machine. Stop it with Ctrl+C.

### Backing up and sharing mod lists
`export` and `import` ask which mods to include (space toggles, enter confirms, everything starts selected).
Pass `--all` to skip the question, which is also what happens when there is no terminal.
The file holds each mod's name, version, `UniqueID` and every update key its manifest declares (e.g. `Nexus:3109`,
`GitHub:owner/repo`), never any credentials.
Some manifests have no usable Nexus id (e.g. `Nexus:???`), for those export asks SMAPI's mod database (smapi.io) for it,
which needs an internet connection. Files from older versions (format 1) can still be imported.
To play co-op with the same mods, export on one machine and send the file to your friends, who run `prismatic import <file>`.

Import always installs the latest version of each mod, so friends can end up on a newer version than the host.
Only Nexus mods can be installed for now, entries from other sources are reported as failed and skipped.
Mods without an update key can't be exported.

### Nexus Mods
Nexus downloads use your `nexusmods_session` cookie, so you must be signed in to Nexus.
Treat the cookie like a password.

Either sign in to Nexus in your browser and Prismatic reads the cookie from it,
or set the `NEXUS_COOKIE` environment variable to the cookie's value (or the whole cookie string).
To find the value, open nexusmods.com while signed in, then in the browser's developer tools go to
Application (Chrome) or Storage (Firefox), Cookies, `nexusmods_session`.

```sh
# macOS and Linux
NEXUS_COOKIE=<value> prismatic update

# Windows PowerShell
$env:NEXUS_COOKIE="<value>"; prismatic update
```

### Config Files
Currently only files or directories named `config` or `config.json` are copied when updating.
When updating the mod's directory is moved into `.old` in the mods directory and is otherwise untouched,
if some config files aren't automatically copied they can manually be copied from there.

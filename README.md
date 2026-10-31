# Prismatic
A CLI tool for installing and updating Stardew Valley mods.

## Installation
Download the archive for your platform from the
[releases page](https://github.com/GestaltCaius/Prismatic/releases), unpack it and put `prismatic` somewhere on your `PATH`.

Or build it with Rust: `cargo install --git https://github.com/GestaltCaius/Prismatic`,
or without Nexus support (see [Nexus Mods](#nexus-mods))
`cargo install --git https://github.com/GestaltCaius/Prismatic --no-default-features -F github`

On macOS a downloaded binary is quarantined, run `xattr -d com.apple.quarantine ./prismatic` once.

## Usage
To update all mods simply run `prismatic update`.
The mod directory defaults to `~/.steam/steam/steamapps/common/Stardew Valley/Mods` on Unix based systems
and `C:\Program Files (x86)\Steam\steamapps\common\Stardew Valley\Mods` on Windows,
if Stardew Valley is installed to an atypical location you can specify it with `-d <dir>`
(e.g. `prismatic -d <dir> update`)

| Command | What it does |
|---|---|
| `prismatic list` | Lists installed mods |
| `prismatic add <nexus url>` | Installs a mod from a Nexus Mods page, e.g. `https://www.nexusmods.com/stardewvalley/mods/3109` |
| `prismatic update` | Updates installed mods if needed (`--force` skips the check) |
| `prismatic export [file]` | Writes your mod list to `prismatic-mods.json` (or `file`) |
| `prismatic import <file>` | Installs the mods listed in a file made by `export` |

### Backing up and sharing mod lists
`export` and `import` ask which mods to include (space toggles, enter confirms, everything starts selected).
Pass `--all` to skip the question, which is also what happens when there is no terminal.
The file only holds each mod's update key (e.g. `Nexus:3109`), name and version, never any credentials.
To play co-op with the same mods, export on one machine and send the file to your friends, who run `prismatic import <file>`.

Import always installs the latest version of each mod, so friends can end up on a newer version than the host.
Only Nexus mods can be installed for now, entries from other sources are reported as failed and skipped.
Mods without an update key can't be exported.

### Nexus Mods
Nexus downloads use your `nexusmods_session` cookie, so you must be signed in to Nexus.
If you are uncomfortable with this you can compile without the `nexus` feature.
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
When updating the mod's directory is moved into `.old` and is otherwise untouched,
if some config files aren't automatically copied they can manually be copied from there.

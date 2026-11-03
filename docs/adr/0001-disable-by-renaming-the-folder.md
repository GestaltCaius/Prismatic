# 0001. A mod is disabled by a dot in front of its folder name

Status: accepted

## Decision
Disabling a mod renames its folder from `Name` to `.Name`, and enabling takes the dot off. SMAPI ignores folders
whose name starts with a dot, on every platform, so the state lives on disk and needs no file of ours. Enabling
a mod also takes the dot off every folder above it, so a mod inside a disabled pack folder loads again.
The list, the updates and the dependency checks all read the state from the folder name.

## Forbidden
- Keeping the enabled or disabled state anywhere but the folder name (a database, a settings file, a manifest edit).
- Deleting a mod to disable it.
- Touching the mod's own files: the manifest and the contents stay exactly as they were.

## Revisit when
SMAPI stops ignoring dot-prefixed folders.

## Cost if wrong
Disabled mods load in the game again, silently. Renaming back fixes it, nothing is lost.

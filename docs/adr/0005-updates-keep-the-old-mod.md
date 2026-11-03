# 0005. Updates keep the old mod and its config

Status: accepted

## Decision
Updating a mod never deletes it. The old folder moves into a `.old` folder under the mods folder, named with its
version, and the `config.json` or `config` of each mod in it is copied into the new one. If extraction fails the
old folder is put back. A mod that belongs to a pack is replaced together with its whole pack folder, since
they come from one download. Installing a new mod touches nothing if its folder already exists. Disabled mods are
skipped by updates. Archive entries that point outside the mods folder, including symbolic links, are not followed.

## Forbidden
- Removing the old version of a mod, or anything under `.old`.
- Replacing a mod before the new archive has been downloaded.
- Copying config files other than `config` and `config.json`.

## Revisit when
Users report losing settings kept in other files, which would call for copying more than two names.

## Cost if wrong
`.old` grows without bound, and settings kept in other files have to be copied back by hand.

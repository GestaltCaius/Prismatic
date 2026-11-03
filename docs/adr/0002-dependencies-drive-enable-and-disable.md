# 0002. Dependencies decide what else is enabled or disabled

Status: accepted

## Decision
Disabling a mod also disables every enabled mod that needs it, through any chain of requirements. Enabling a mod
also enables the disabled mods it needs. A requirement counts when it is in `Dependencies` with `IsRequired`
true (the default when the field is missing) or in `ContentPackFor`. A requirement that another enabled mod
already provides is left alone. Both front ends list the affected mods, the page asks first and the command
line prints each one.

Dependency lists are read as raw JSON, so a malformed list can never hide the mod that holds it.

When a mod shares a pack folder with others, the siblings are given their own dot before the shared folder
is touched, so disabling one mod does not enable or disable its neighbours.

## Forbidden
- Changing the state of a mod that the user was not shown.
- Treating a missing `IsRequired` as optional.
- Dropping a mod from the list because its manifest or its dependency list is broken. A broken manifest is
  a warning, the other mods keep working.

## Revisit when
SMAPI changes what counts as a required dependency.

## Cost if wrong
A mod is switched off that the user wanted on, or left on without something it needs and the game logs an
error. Both are visible in the list and reversible.

# 0007. SMAPI's database is a hint, not a requirement

Status: accepted

## Decision
SMAPI's mod database (smapi.io) tells Prismatic which Nexus page a mod belongs to when its manifest has no
usable update key (missing, or a placeholder such as `Nexus:???`), and what the newest version is. Answers are
remembered for the session. After a failed lookup SMAPI is left alone for 30 seconds, and callers that arrive
during a lookup wait for its answer instead of sending their own, because the page polls every 700 ms. The page
gives up on a slow lookup after 3 seconds and shows what it has. A mod SMAPI does not know takes the source its
pack siblings agree on.

The update check itself needs SMAPI, it is how Prismatic knows a newer version exists. If SMAPI cannot be reached
the check fails with that error, and `--force` skips the question and updates every mod that has a source.
Listing, disabling and enabling never need SMAPI.

## Forbidden
- Holding up the page, or failing a command that does not need versions, because SMAPI is slow or down.
- Asking SMAPI again on every poll after a failure.
- Using a placeholder update key as if it were real.

## Revisit when
SMAPI's database changes its format or its availability.

## Cost if wrong
While SMAPI is down no update check works (`--force` still does), some mods are reported as having no update
source, and a mod can be matched to the wrong page if the database is wrong.

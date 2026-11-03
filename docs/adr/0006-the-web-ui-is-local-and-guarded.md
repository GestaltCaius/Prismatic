# 0006. The web UI is local, guarded and runs one job at a time

Status: accepted

## Decision
The web interface is a server on the user's own machine, listening on the loopback address only. Every request
needs a loopback `Host` (which stops DNS rebinding) and a random token made at launch, in the address for the
page and in a header for the API. The token is compared in constant time. Only the page's images skip the token.
The whole page is one self-contained file embedded in the binary, with no outside scripts, fonts or images.

Jobs (update, add, import, install dependencies) run one at a time, in the order they were started, and the page
polls for their progress. Changing the mods folder or the enabled state while a job runs is refused.

## Forbidden
- Listening on any other address.
- An API route that skips the guard.
- Loading anything from the network into the page.
- Running two jobs at once on one mods folder.

## Revisit when
A need for remote access appears, which would call for real authentication and not a launch token.

## Cost if wrong
Another website or program on the machine could make Prismatic install mods or change folders. The token
and the `Host` check are the only defence.

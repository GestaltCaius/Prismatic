# 0003. Missing dependencies are offered, never installed unasked

Status: accepted

## Decision
When enabled mods need a mod that is not installed, Prismatic offers to install it. The page lists them in a
panel, the command line shows a picker with everything selected, and nothing is installed until the user confirms.
After `add` or `import`, the offer covers only what the mods just installed need. A mod is installed from Nexus
when SMAPI's database knows its page, otherwise it is only named so the user can find it. Without a terminal
the commands list the missing mods and install nothing.

## Forbidden
- Installing a dependency without asking.
- Offering what some other, unrelated installed mod needs, as part of `add` or `import`.

## Revisit when
Users keep accepting every offer, which would make a default of "install them" worth considering.

## Cost if wrong
An unwanted mod is installed, or a dependency stays missing until the user installs it by hand.

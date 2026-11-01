# Prismatic design

The web UI (`src/ui.html`) is one self-contained page. Plain HTML, CSS and JavaScript, no framework, no web fonts.
The structure follows the Obsidian DESIGN.md (flat surfaces, hairline borders, small radii, one accent, system fonts).
The look is a Stardew Valley parchment theme: wood-brown borders, parchment panels, grass-green actions.
There is a single theme. A dark variant would be an invented palette, not the game's.

## Tokens

The `:root` block in `src/ui.html` is the source of truth. The accessibility tests in `src/ui.rs` read these values.

| Token | Value | Use |
|---|---|---|
| `--bg` | `#e6cc96` | Page background, input fill |
| `--surface` | `#f8e8c0` | Panels, buttons |
| `--text` | `#3b2410` | Body text, focus ring |
| `--muted` | `#6b4c2c` | Secondary text, placeholders |
| `--border` | `#a8743c` | Panel and control borders (wood) |
| `--accent` | `#447a28` | Primary buttons, progress bars (grass) |
| `--accent-contrast` | `#ffffff` | Text on `--accent` |
| `--ok` / `--warn` / `--danger` | `#2f6e1f` / `#9a5200` / `#a3261e` | Status badges and the error banner |
| `--radius` | `6px` | All corners except pill badges |
| `--gap` | `12px` | Spacing between blocks |

## Type

System font stack for everything, monospace for update keys. Body 15px, line height 1.45.
Game fonts are not used: they hurt scanning of a table of mod names and versions.

## Shape and components

- Panels: 2px wood border on parchment. Controls: 1px border.
- Buttons: one primary per group (green), the rest on `--surface`. The cookie control is a `summary` styled as a button.
- Status badges are outlined pills, colored by state (ok, warn, danger).
- Focus: a 3px `--text` outline on every focusable element. Never remove it.
- Placeholders use `--muted` at full opacity.

## Assets

`src/assets/` holds `logo.png` (Stardew Valley logo, 1x), `duck.png` (tab icon) and `dog.gif` (idle state in the activity panel).
They belong to ConcernedApe and are used from his logo press kit. Prismatic is not affiliated with Stardew Valley or ConcernedApe.
Pixel art must stay crisp: integer scales only, with `image-rendering: pixelated`. The pixel logo is never scaled down.
The server hands the images out without the API token, to loopback hosts only, because `<img>` cannot send the token header.

## Accessibility gates

These run in `cargo test` (module `ui::tests`) and fail the build.

- Text pairs reach WCAG AA 4.5:1: text, muted, ok, warn and danger on `--surface`, muted and danger on `--bg`, and `--accent-contrast` on `--accent`.
- Control borders, the accent and the focus ring reach 3:1 against `--surface`.
- `lang`, viewport meta, `:focus-visible`, `role="alert"` on the error banner, `aria-live` on the cookie status.
- Every static `<input>` has an `aria-label` and every `<img>` has `alt` (empty for decorative images).

When changing a color, edit `:root` and run `cargo test ui::`. Do not hardcode colors outside `:root`.

---
version: alpha
name: Prismatic
description: Stardew Valley parchment theme for the Prismatic web UI, a plain list of mods with a few actions and one cookie field.
colors:
  primary: "#447a28"
  on-primary: "#ffffff"
  secondary: "#a8743c"
  neutral: "#e6cc96"
  surface: "#f8e8c0"
  on-surface: "#3b2410"
  muted: "#6b4c2c"
  success: "#2f6e1f"
  warning: "#9a5200"
  error: "#a3261e"
typography:
  h1:
    fontFamily: system-ui, -apple-system, Segoe UI, sans-serif
    fontSize: 24px
    fontWeight: 700
    lineHeight: 1.45
  h2:
    fontFamily: system-ui, -apple-system, Segoe UI, sans-serif
    fontSize: 17px
    fontWeight: 700
    lineHeight: 1.45
  body-md:
    fontFamily: system-ui, -apple-system, Segoe UI, sans-serif
    fontSize: 15px
    fontWeight: 400
    lineHeight: 1.45
  label-sm:
    fontFamily: system-ui, -apple-system, Segoe UI, sans-serif
    fontSize: 12px
    fontWeight: 400
    lineHeight: 1.45
  code-sm:
    fontFamily: ui-monospace, SF Mono, Consolas, monospace
    fontSize: 13px
    fontWeight: 400
    lineHeight: 1.45
rounded:
  md: 6px
  full: 999px
spacing:
  xs: 4px
  sm: 8px
  md: 12px
  lg: 16px
components:
  panel:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.on-surface}"
    rounded: "{rounded.md}"
    padding: 16px
  button:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.on-surface}"
    typography: "{typography.body-md}"
    rounded: "{rounded.md}"
    padding: 7px
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    typography: "{typography.body-md}"
    rounded: "{rounded.md}"
    padding: 7px
  input:
    backgroundColor: "{colors.neutral}"
    textColor: "{colors.on-surface}"
    typography: "{typography.body-md}"
    rounded: "{rounded.md}"
    padding: 7px
  text-muted:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.muted}"
    typography: "{typography.label-sm}"
  badge-success:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.success}"
    typography: "{typography.label-sm}"
    rounded: "{rounded.full}"
    padding: 1px
  badge-warning:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.warning}"
    typography: "{typography.label-sm}"
    rounded: "{rounded.full}"
    padding: 1px
  badge-error:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.error}"
    typography: "{typography.label-sm}"
    rounded: "{rounded.full}"
    padding: 1px
---

# Prismatic design

The web UI is `src/ui.html`, one self-contained page with plain HTML, CSS and JavaScript. The `:root` block there holds the same values as the tokens above, as CSS variables (`--bg` is `neutral`, `--text` is `on-surface`, `--accent` is `primary`, `--border` is `secondary`, `--ok`, `--warn` and `--danger` are `success`, `warning` and `error`). Keep the two in sync.

## Overview

Calm and practical, with a farm-sim feel. It is a tool for managing a list of mods, so it reads like a tidy notice board: parchment panels with wood-brown borders, grass-green for the one action that matters. The structure follows the Obsidian DESIGN.md (flat surfaces, hairline borders, small radii, one accent, system fonts). There is one theme only, because the game's recognisable UI is parchment and wood, and a dark variant would be an invented palette.

## Colors

- **Primary (#447a28):** grass green, used for primary buttons and progress bars.
- **Secondary (#a8743c):** wood brown, used for panel and control borders.
- **Neutral (#e6cc96):** parchment, the page background and input fill.
- **Surface (#f8e8c0):** light parchment for panels and buttons.
- **On-surface (#3b2410):** dark brown for body text and the focus ring.
- **Muted (#6b4c2c):** softer brown for secondary text and placeholders.
- **Success, warning, error:** deep green, amber and red for status badges and the error banner.

## Typography

System fonts everywhere, monospace for update keys. Game fonts are not used: they hurt scanning of a table of mod names and versions. Body is 15px with a 1.45 line height. Placeholders use the muted color at full opacity.

## Layout

A single column, 1000px max width, 16px page padding and 16px panel padding. Blocks are separated by 12px, controls in a row by 8px. The mods table scrolls horizontally on narrow screens.

## Elevation & Depth

Flat. Hierarchy comes from borders and tonal layers (neutral page, surface panels), never from shadows. Panels have a 2px wood border, controls a 1px one.

## Shapes

6px corners on panels, buttons and inputs. Status badges are fully rounded pills. Pixel art keeps crisp edges (see Assets).

## Components

- **Panel:** parchment surface with a 2px wood border.
- **Buttons:** one primary per group (green), the rest on the surface color. The cookie control is a `summary` styled as a button. Primary and secondary buttons are 7px by 12px.
- **Inputs:** neutral fill, 1px wood border, always with an `aria-label`.
- **Badges:** outlined pills colored by state (success, warning, error).
- **Focus:** a 3px on-surface outline on every focusable element.

## Do's and Don'ts

- Do keep one primary button per group.
- Do edit `:root` for any color change and run `cargo test ui::`.
- Don't hardcode colors outside `:root`.
- Don't remove or hide the focus outline.
- Don't scale the pixel logo down or use non-integer scales for pixel art.
- Don't add a font file: the page is embedded in the binary and must work offline.

## Assets

`src/assets/` holds `logo.png` (Stardew Valley logo, 1x), `duck.png` (tab icon) and `dog.gif` (idle state in the activity panel). They belong to ConcernedApe and come from his logo press kit and the Stardew Valley wiki. Prismatic is not affiliated with Stardew Valley or ConcernedApe. Pixel art is shown at integer scales with `image-rendering: pixelated`. The server hands the images out without the API token, to loopback hosts only, because `<img>` cannot send the token header.

## Accessibility gates

These run in `cargo test` (module `ui::tests`) and fail the build.

- Text pairs reach WCAG AA 4.5:1: on-surface, muted, success, warning and error on surface, muted and error on neutral, and on-primary on primary.
- Control borders, the primary color and the focus ring reach 3:1 against surface.
- The page has `lang`, a viewport meta tag, a `:focus-visible` rule, `role="alert"` on the error banner and `aria-live` on the cookie status.
- Every static `<input>` has an `aria-label` and every `<img>` has `alt` (empty for decorative images).

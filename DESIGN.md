---
name: Minerva
description: "The Gate Board — a status-board interface for goal/milestone-first project management: cool neutral ground, ink rules, four signal colours, tabular mono figures."
colors:
  ground: "#f2f3f4"
  panel: "#ffffff"
  ink: "#101114"
  slate: "#5a5f69"
  ink-line: "#1011141f"
  accent: "#101114"
  terracotta-dot: "#c4502b"
  status-on-track: "#1f7a4d"
  status-at-risk: "#d99a1b"
  status-off-track: "#ce2d2d"
  status-complete: "#101114"
  alert-amber: "#ffb020"
  error-text: "#ce2d2d"
  rail-bg: "#101114"
  rail-fg: "#f2f3f4"
  rail-fg-muted: "#a7a9ae"
  rail-line: "#f2f3f426"
  ground-dark: "#131417"
  panel-dark: "#1b1d21"
  ink-line-dark: "#f2f3f424"
  rail-bg-dark: "#0c0d0f"
typography:
  display:
    fontFamily: "system-ui, -apple-system, 'Segoe UI', sans-serif"
    fontWeight: 600
    letterSpacing: "-0.01em"
  body:
    fontFamily: "system-ui, -apple-system, 'Segoe UI', sans-serif"
    fontSize: "16px"
    lineHeight: 1.65
    fontWeight: 400
  figures:
    fontFamily: "'Spline Sans Mono', ui-monospace, SFMono-Regular, Menlo, monospace"
    fontFeature: "tnum"
    fontWeight: 400
  label:
    fontFamily: "system-ui, -apple-system, 'Segoe UI', sans-serif"
    fontSize: "11px"
    fontWeight: 600
    letterSpacing: "0.1em"
rounded:
  panel: "6px"
  item: "8px"
  chip: "10px"
  circle: "50%"
spacing:
  "1": "4px"
  "2": "8px"
  "3": "12px"
  "4": "16px"
  "5": "20px"
  "6": "24px"
  "7": "32px"
  "8": "48px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.panel}"
    rounded: "{rounded.panel}"
    padding: "9px 20px"
    height: "44px"
  button-primary-hover:
    backgroundColor: "color-mix(in oklch, #101114 88%, black)"
  button-secondary:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.panel}"
    padding: "8px 16px"
    height: "44px"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.panel}"
    padding: "8px 10px"
    height: "44px"
  text-input:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.panel}"
    padding: "10px 14px"
    height: "44px"
  nav-link-rail:
    backgroundColor: "transparent"
    textColor: "{colors.rail-fg}"
    rounded: "{rounded.item}"
    padding: "9px 12px"
  status-square-on-track:
    backgroundColor: "{colors.status-on-track}"
    size: "10px"
  status-square-at-risk:
    backgroundColor: "{colors.status-at-risk}"
    size: "10px"
  status-square-off-track:
    backgroundColor: "{colors.status-off-track}"
    size: "10px"
  status-square-complete:
    backgroundColor: "{colors.status-complete}"
    size: "10px"
  status-square-alert:
    backgroundColor: "{colors.alert-amber}"
    size: "10px"
---

# Design System: Minerva

## Overview

**Creative North Star: "The Gate Board"**

Minerva's interface is a status board, not a dashboard of charts and tables. The term's health is a ranking you watch, not a list you search: cool neutral ground (#F2F3F4) carrying white panels under ink rules, four signal colours for the four statuses, and one reserved amber that holds on rows a person set by hand. Everything numeric — counts, dates, stamps — is set in tabular monospace so columns line up like a departure board; everything verbal is the workhorse system sans. The posture is warm, capable and unfussy: a well-run staff room, with no enterprise overhead in the pixels either.

The Gate Board world lends type, palette, density and its signature moves (the solid status square, the 2px spine, the alert-hold row) — it does not lend layout or controls. The incumbent navigation model is preserved exactly: an ink rail on desktop, a top bar plus bottom tab bar below 900px.

Confirmed visual rejections: no blue anywhere in the system; no gradients, glass or other surface effects; no serif display type; no coloured status text (status is always a solid square beside an ink label); no KPI stat-card rows over chart-and-table compositions.

**Key Characteristics:**
- Cool neutral ground under ink rules; white panels; one always-ink rail in both colour modes
- Status renders as a solid 10px colour square beside an ink small-caps label — never coloured text
- Tabular mono figures (Spline Sans Mono) on every count, date and stamp
- One reserved amber (#FFB020) for manually-set rows only — the alert hold
- Complete is set in ink itself (paper in dark mode); settled reads neutral-final
- Flat panels with 1px ink-tinted borders; the system's one thick line is the 2px spine under the board's column heads

## Colors

A cool neutral ground carrying ink rules and exactly five saturated colours — four status signals, one reserved amber — plus the wordmark's terracotta dot.

### Primary
- **Ink Block** (#101114): primary actions are a solid ink block with a paper label (label set at 18px — the accent never carries small text); also the focus outline colour. In dark mode it inverts to a paper block (#F2F3F4) with an ink label.
- **Terracotta Dot** (#C4502B): the wordmark's dot only — a brand commitment, unchanged by the board world. Never an interface accent, never recoloured.

### Secondary (the board's signal colours)
- **On Track** (#1F7A4D): status square and tint for healthy items.
- **At Risk** (#D99A1B): status square and tint for items behind schedule.
- **Off Track** (#CE2D2D): status square and tint for items past target or failing; also the hue of semantic error copy (see below).
- **Complete** (= ink #101114; paper in dark mode): set in ink itself — settled reads neutral-final. The world has no blue, so Complete never gets one.

### Tertiary
- **Alert Amber** (#FFB020): reserved exclusively for rows a person set by hand (manual status overrides) — the alert hold. Never for decoration, warnings or errors.

### Neutral
- **Ground** (#F2F3F4): page canvas; cool neutral, not warm. Dark mode: #131417.
- **Panel** (#FFFFFF): boards, cards and panels. Dark mode: #1B1D21.
- **Ink** (#101114): body text, headings, rules, the Complete square; dark surfaces. In dark mode foreground becomes paper #F2F3F4.
- **Slate** (#5A5F69): secondary text, small-caps column heads and metadata. Dark mode: #9CA1AB.
- **Ink line** (ink at 12% on light, `#1011141f`; paper at 14% on dark, `#f2f3f424`): all rules and dividers — 1px.
- **Ink rail** (#101114 with paper text #F2F3F4, muted #A7A9AE, hairlines paper at 15% `#f2f3f426`): the sidebar and mobile top bar; dark in both modes. Dark mode: #0C0D0F, so it stays darker than the page.

**Error copy.** Semantic error text is the Off Track hue — pure #CE2D2D in light mode, lifted to 55% red / 45% foreground in dark mode so small text clears 4.5:1 on the near-black ground. Status squares keep the pure hue in both modes.

### Named Rules
**The No Blue Rule.** The world has no blue. There is no blue accent, link, status or focus colour; Complete is ink itself. Do not introduce one.

**The Square-Beside-the-Label Rule.** A status always renders as a solid 10px colour square beside an ink small-caps label — never as coloured text. The square carries the hue; the words stay ink (or slate).

**The Alert-Hold Rule.** Amber #FFB020 means exactly one thing: a person set this by hand. It appears on manually-set rows (amber square plus a 13% amber row tint) and nowhere else — never as decoration, warning or error colour.

## Typography

**Display Font:** system sans (`system-ui, -apple-system, "Segoe UI", sans-serif`)
**Body Font:** the same system sans — one workhorse face for all words
**Label/Mono Font:** Spline Sans Mono (self-hosted via @fontsource; fallbacks `ui-monospace, SFMono-Regular, Menlo, monospace`), tabular figures, weights 400/500/600 only

**Character:** A single sans does all the talking in plain, direct sentences; the mono is not a costume for prose but the board's figure face — counts, dates and stamps that must line up. Fraunces appears exactly once: as official brand artwork (the wordmark), never as interface type.

### Hierarchy
- **Headline** (600, 27px desktop / 21px mobile, line-height 1.15, -0.01em): page titles via the shared header; all headings h1–h6 are weight 600 with -0.01em tracking and balanced text wrap.
- **Board figure** (Spline Sans Mono 600, 34px desktop / 26px mobile, line-height 1, tabular): the large rollup counts. Every count, date and stamp uses this face — the `.fig` utility applies `font-variant-numeric: tabular-nums`.
- **Body** (400, 16px, line-height 1.65): prose, descriptions, row titles (row titles at 14.5px/500).
- **Label** (600, 10.5–12px, uppercase, tracked 0.08–0.14em, slate): small-caps column heads, kind tags, group labels and the board foot stamp.

### Named Rules
**The Tabular Figures Rule.** Every count, date and stamp is Spline Sans Mono with tabular figures — never proportional numerals in a sans face, and never mono used to dress up prose.

**The Wordmark-Only Fraunces Rule.** Fraunces is official artwork, not an interface font: it renders the brand wordmark (the full mark is path-based SVG so it cannot drift; the collapsed "M." text symbol is set in Fraunces 600 to match it). Font-detector warnings about overused serifs are false positives here — do not "fix" the wordmark into a UI face, and never use Fraunces for headings or body.

## Layout

One breakpoint: 900px. Above it, the shell is a two-column grid — a fixed 264px ink rail (72px when collapsed) beside a content column that alone scrolls (the shell is 100dvh with overflow hidden). Below it, a 56px ink top bar and a 64px bottom tab bar (plus safe-area inset) wrap the same content. The content column is capped at 1200px, centred, padded 24px on desktop and 16px on mobile.

The board composes in fixed columns: the rollup grid is a 132px kind-label column plus four equal status columns; the manually-set list is 96px (date) / fluid (title) / 248px (status). Below 900px the rollup drops its label column (the kind label spans a row above four aligned columns, and the status heads stack square-over-label centred), and manual rows stack date-and-status over the title. Wide content scrolls sideways inside its own box; the page never grows sideways.

Spacing runs on a 4px base grid (scale steps 4/8/12/16/20/24/32/48); controls are 44px tall.

### Named Rules
**The Ink-Rail Rule.** The navigation is ink in both colour modes — the page's light/dark state is a property of the content, never of the navigation. Dark mode follows the OS (`prefers-color-scheme`) with no manual toggle and no storage; do not add one.

## Elevation & Depth

Flat by tonal layering: the ground, panels and rail are three distinct values in each mode (dark mode's rail #0C0D0F sits darker than its ground #131417, which sits darker than its panels #1B1D21), and 1px ink-tinted borders do most of the separation. Shadows appear only twice: a hairline lift under resting panels, and a soft shadow for floating layers (popovers, the mobile sheet). No hard offset shadows, no ambient glows, no effects on brand marks.

### Shadow Vocabulary
- **Panel lift** (`box-shadow: 0 1px 3px color-mix(in oklch, var(--minerva-fg) 6%, transparent)`): resting board panels — a hint of separation from the ground, nothing more.
- **Floating layer** (`box-shadow: 0 12px 32px color-mix(in oklch, var(--minerva-fg) 14%, transparent)`): popover menus and the mobile bottom sheet only.

### Named Rules
**The Flat-By-Default Rule.** Surfaces are flat at rest; the two shadows above are the entire shadow vocabulary. Depth that needs to be louder is drawn as a rule (see the Spine), not a shadow.

## Shapes

Corners are gently rounded and role-assigned: 6px (`--minerva-radius`) for panels, cards, buttons, inputs and floating layers; 8px for interactive items inside menus and the rail (nav links, menu items, close buttons); 10px for the sidebar user chip; full circles for avatars. Status squares are deliberately sharp — solid 10×10px with no radius, the one hard-edged geometry in the system. Borders are 1px ink-tinted (12% ink on light, 14% paper on dark) throughout.

The board's spine is the system's only thick line: a 2px solid ink rule under the rollup's column heads. Everything else — row dividers, panel edges, separators — is 1px.

### Named Rules
**The Spine Rule.** The one thick line in the system is the 2px solid ink rule under the board's small-caps column heads. Do not thicken other rules or add a second heavy line; hierarchy comes from the spine plus tonal steps.

## Components

### Buttons
- **Shape:** 6px radius, 44px min-height, built on Bits UI's headless Button (the house rule for all interactive primitives).
- **Primary:** solid ink block with a paper label set at 18px (padding 9px 20px); inverts to a paper block with an ink label in dark mode. Hover darkens the block to 88% accent mixed with black; active presses down 1px.
- **Secondary:** transparent with a 1px ink-tinted border, ink text at 14.5px/500; hover tightens the border to full ink. The default variant.
- **Ghost:** borderless, 8px 10px padding; hover tints the background 6% foreground.
- **Focus / Disabled:** 2px solid accent outline with 2px offset (paper in dark mode); disabled drops opacity to 0.55 and removes pointer events.

### Inputs / Fields
- **Style:** panel background, 1px ink-tinted border, 6px radius, 44px min-height, 15px text; placeholder in slate.
- **Focus:** 2px accent outline plus the border moving to full accent — a ring, not a glow.
- **Invalid:** `aria-invalid` moves the border to the accent (error *copy* uses the error-text colour; see Colors).

### Navigation
- **Rail (desktop):** ink in both modes; wordmark top-left with a collapse toggle (rail 264px → 72px, in-memory only); nav links are icon + label at 14.5px/500 on 8px-radius rows — hover tints 10% paper, active holds 16% paper; an "Admin" group label and entry appear for admins; the user chip (avatar, name, role) sits at the rail foot and opens a popover menu. Collapsed links keep their accessible name and gain tooltips on hover and keyboard focus.
- **Mobile (<900px):** 56px ink top bar with the small wordmark and an avatar button; 64px bottom tab bar (icon over label, plus an Admin tab for admins); the avatar opens a bottom sheet (Bits UI Dialog) with account details, admin entry and sign out.
- **Focus:** rail controls take a 2px paper outline.

### Status Square & Label (signature)
A solid 10×10px square in the status hue beside an ink small-caps label (11–12px/600, tracked, uppercase). On Track #1F7A4D, At Risk #D99A1B, Off Track #CE2D2D, Complete = ink (paper in dark). The square is `aria-hidden`; the words carry the meaning for assistive tech. Never recolour the label to match the square.

### Board Rollup Cell
One cell per status per kind: a large tabular figure (34px/600 mono, 26px mobile) with an optional sub-count beneath in 11.5px slate mono ("1 by hand"). Cells share fixed columns so figures align vertically across the Goals and Milestones rows.

### Alert-Hold Row (signature)
A manually-set row keeps the amber tint: 13% alert-amber background over the panel, an amber square before the kind tag (Goal/Milestone in small caps), the title at 14.5px/500, and the status as its normal square-and-label with the automatic status alongside in small mono ("computed On Track"). The row is how a reader sees that a person touched this — the hold does not fade or change with the underlying status.

### Page Header
Title (27px/600) over an optional one-line description in 13.5–14.5px slate, with actions top-right when the role allows them; wraps on narrow screens. Pages that have no permitted action render no action slot at all.

### Wordmark (signature)
Official artwork from `/static` — path-based SVG ("Minerva" with the terracotta dot over the i), never retyped in CSS. The full mark renders at 21px in the rail, 17px in the mobile top bar and 28px on auth screens; the reversed (paper) variant shows on the ink rail and in dark mode. The collapsed "M." symbol is set in Fraunces 600 with the dot in terracotta to match the artwork. Clear space and minimum sizes follow `docs/design/brand.json`; never stretch, rotate, outline or add effects.

## Do's and Don'ts

### Do:
- **Do** take every colour from the `--minerva-*` tokens (light + `.dark`); stylesheets hold tokens only — no raw colours in component CSS.
- **Do** render status as a solid 10px square beside an ink small-caps label, per The Square-Beside-the-Label Rule.
- **Do** set every count, date and stamp in Spline Sans Mono with tabular figures (the `.fig` utility).
- **Do** keep the primary action a solid ink block with a paper label at 18px+, inverting to paper/ink in dark mode.
- **Do** use amber #FFB020 only on rows a person set by hand — square plus 13% row tint, with the computed status shown alongside.
- **Do** keep the rail ink in both colour modes and follow `prefers-color-scheme` (no toggle, no storage).
- **Do** use Bits UI for any new interactive primitive and style it on the tokens; keep wrapper component props stable.
- **Do** honour `prefers-reduced-motion` — the base layer already kills all animation and transition under it.

### Don't:
- **Don't** introduce blue anywhere — no blue accent, link, status or focus colour (The No Blue Rule).
- **Don't** set status text in the status hue; the words stay ink or slate and the square carries the colour.
- **Don't** use amber for decoration, warnings, validation or anything other than manually-set rows (The Alert-Hold Rule).
- **Don't** use Fraunces outside the wordmark — no serif display, headings or body in it (The Wordmark-Only Fraunces Rule); detector "overused font" warnings on it are false positives.
- **Don't** add gradients, glass, glow effects or hard offset shadows; the two-entry shadow vocabulary is closed.
- **Don't** thicken rules beyond the 2px board spine, or add a second heavy line (The Spine Rule).
- **Don't** recolor, stretch, rotate, outline or retype the wordmark; its only colour is the terracotta dot.
- **Don't** build a manual dark-mode toggle or persist a theme choice — OS scheme only.

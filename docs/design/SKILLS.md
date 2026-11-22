---
name: "Minerva"
category: Brands
surface: web
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
  alert-amber: "#ffb020"
  rail-bg: "#101114"
  ground-dark: "#131417"
  panel-dark: "#1b1d21"
  rail-bg-dark: "#0c0d0f"
---

# Minerva

> Category: Brands

> Surface: web

*Open source, goal/milestone-first project management for schools and non-profits.*

Open source, goal/milestone-first project management for schools and non-profits: teams start from what they are trying to achieve, break it into milestones, and track progress against those. SvelteKit frontend, Rust (Actix-web) backend, PostgreSQL. The interface is "The Gate Board": a status board on cool neutral ground under ink rules — warm, capable and unfussy, with no enterprise overhead in the pixels either.

## Color Palette

| Role | Name | Hex | Usage |
| --- | --- | --- | --- |
| background | Ground | `#f2f3f4` | page canvas; PWA background colour; dark mode `#131417` |
| surface | Panel | `#ffffff` | boards, cards and panels; dark mode `#1b1d21` |
| foreground | Ink | `#101114` | body text, headings, rules, the Complete square; PWA theme colour; paper `#f2f3f4` in dark mode |
| muted | Slate | `#5a5f69` | secondary text, small-caps column heads and metadata (5.8:1 on ground); dark mode `#9ca1ab` |
| border | Ink line | `#1011141f` | rules and dividers — 1 px; ink at 12% on light, paper at 14% on dark (`#f2f3f424`) |
| accent | Ink block | `#101114` | primary actions and focus rings; a solid block carrying a paper label at 18 px or larger, never small text; inverts to a paper block with an ink label in dark mode |
| status | On Track / At Risk / Off Track | `#1f7a4d` / `#d99a1b` / `#ce2d2d` | the status signals — always a solid 10 px square beside an ink small-caps label, never coloured text; Complete is ink itself (paper in dark); the world has no blue |
| alert | Alert amber | `#ffb020` | rows a person set by hand only (amber square plus 13% row tint) — the alert hold; never decoration, warning or error |
| brand | Terracotta dot | `#c4502b` | the wordmark's dot only; never an interface accent, never recoloured |

## Typography
- **Headings & body:** system-ui — headings weight 600 with -0.01em tracking; body 400 at 16px/1.65 — fallbacks: -apple-system, Segoe UI, sans-serif
- **Figures:** Spline Sans Mono (self-hosted) — tabular figures on every count, date and stamp; weights 400/500/600 only
- **Wordmark artwork:** Fraunces — official brand artwork only (the wordmark and the collapsed "M." symbol); never interface type

## Voice & Tone

- **Adjectives:** warm, capable, unfussy
- **Tone:** A well-run staff room: warm, capable and unfussy. Plain, direct sentences about getting the term's work done — no enterprise overhead, no corporate jargon.

### Messaging pillars
- Open source project management for schools and non-profits — self-hosted, without the enterprise overhead.
- Goal/milestone-first: teams start from what they are trying to achieve, break it into milestones, and track progress against those — rather than managing a flat list of tasks.
- Keeps staff, volunteers and boards on the same page.

### Vocabulary
- **Use:** goal, milestone, term, progress, staff, volunteers, boards
- **Avoid:** enterprise overhead or enterprise jargon, corporate or bureaucratic phrasing, flat task-list framing — Minerva is goal-first, not a to-do app

## Imagery

- **Style:** Cool neutral and flat — ground `#f2f3f4`, white panels, 1 px ink-tinted borders; the four status signals plus one reserved amber are the only saturated colours, and the wordmark's terracotta dot is the only brand mark
- **Subjects:** school staff, volunteers and boards, term and milestone planning, small, real workspaces
- **Treatment:** Real photography on ground or panel with 1 px ink-tinted borders; status always a solid square beside an ink label; no shadows beyond the two in the system (panel lift, floating layer), no gradients or effects on brand marks
- **Avoid:** blue of any kind, coloured status text, amber outside manually-set rows, Fraunces outside the wordmark, shadows/gradients/effects on the logo, recoloured dots or letters outside the palette, corporate-blue SaaS stock imagery

## Layout

- **Radius:** 6px panels/cards/buttons/inputs/floating layers; 8px menu and rail items; 10px sidebar user chip; full circles for avatars; status squares are sharp 10×10 px
- **Border weight:** 1px ink-tinted throughout; the one thick line is the 2px board spine under the rollup's column heads
- **Spacing:** 4px base grid (4/8/12/16/20/24/32/48); 1200px content column; controls 44px tall

### Posture rules
- Component kit should cover: Button, Card, Form, Navigation.
- Cool neutral ground, white panels, 1px ink-tinted borders (12% ink on light; 14% paper on dark).
- The rail — sidebar and mobile top bar — is ink in both colour modes (`#101114` light, `#0c0d0f` dark) with a paper wordmark and terracotta dot; dark mode follows `prefers-color-scheme` only, no toggle.
- Primary buttons are a solid ink block with a paper label at 18px+ (a paper block with an ink label in dark); the accent never carries small text.
- Status renders as a solid 10px square beside an ink small-caps label — never coloured text; Complete is ink itself, and the world has no blue.
- Every count, date and stamp is Spline Sans Mono with tabular figures; headings are the system sans at weight 600 with -0.01em tracking.
- Dark mode: ground `#131417`, panels `#1b1d21`, paper text `#f2f3f4`; the rail stays darker than the page (`#0c0d0f`).

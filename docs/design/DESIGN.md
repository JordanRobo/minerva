---
name: "Minerva"
category: Brands
surface: web
colors:
  cream: "#fbf6ee"
  white: "#ffffff"
  ink: "#1e2233"
  slate: "#5b6078"
  ink-line: "#dfdbd7"
  terracotta: "#c4502b"
  ember: "#e9805f"
---

# Minerva

> Category: Brands

> Surface: web

*Open source, goal/milestone-first project management for schools and non-profits.*

Open source, goal/milestone-first project management for schools and non-profits: teams start from what they are trying to achieve, break it into milestones, and track progress against those. SvelteKit frontend, Rust (Actix-web) backend, PostgreSQL. The identity should feel like a well-run staff room: warm, capable and unfussy.

## Color Palette

| Role | Name | Hex | Usage |
| --- | --- | --- | --- |
| background | Cream | `#fbf6ee` | page canvas; PWA background colour; reversed (cream) text on ink |
| surface | White | `#ffffff` | cards and panels |
| foreground | Ink | `#1e2233` | body text, headings and wordmark; dark surfaces; PWA theme colour |
| muted | Slate | `#5b6078` | secondary text and metadata (5.8:1 on cream) |
| border | Ink line | `#dfdbd7` | rules and dividers — 1 px; in source CSS it is ink at 12.5% alpha on cream (#1E223320), on dark backgrounds cream at 15% (#F3ECE125) |
| accent | Terracotta | `#c4502b` | the dot; primary actions and large accents on light backgrounds only; carries white or cream labels at 18 px or larger, never body copy (4.3:1 on cream) |
| accent-secondary | Ember | `#e9805f` | the dot and accent on dark backgrounds only (5.8:1 on ink) |

## Typography
- **Display:** Fraunces — weights 500 — fallbacks: Georgia, serif
- **Body:** system-ui — weights 400, 500 — fallbacks: -apple-system, Segoe UI, sans-serif

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

- **Style:** Warm, editorial and human — a cream canvas, white cards and a single terracotta accent, the dot of the wordmark
- **Subjects:** school staff, volunteers and boards, term and milestone planning, small, real workspaces
- **Treatment:** Real photography on cream or white with 1 px ink-tinted borders; terracotta is the only saturated colour in the interface; no shadows, gradients or effects on brand marks
- **Avoid:** shadows, gradients or effects on the logo, recoloured dots or letters outside the palette, the colour logo on photos or busy backgrounds without a solid container, corporate-blue SaaS stock imagery

## Layout

- **Radius:** 12px
- **Border weight:** 1px
- **Spacing:** 4px base grid; 880px content column; 56-80px section rhythm; body 16px/1.65

### Posture rules
- Component kit should cover: Button, Card, Form, Navigation.
- Cream canvas, white cards, 1px ink-tinted borders (12.5% ink on light; 15% cream on dark).
- Terracotta is the only saturated colour: primary buttons, the dot, large accents — white or cream labels at 18px+ only.
- Headlines set in Fraunces 500 (SOFT 100, WONK 1); body and interface in the system sans at 16px/1.65, weight 500 for emphasis.
- Dark mode: ink surfaces (#171A28 canvas, #1E2233 cards, #F3ECE1 text) with ember accents — terracotta is reserved for light backgrounds.

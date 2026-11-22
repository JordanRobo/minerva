# Minerva design system

The single source of truth for Minerva's visual world — "The Gate Board" — is [`DESIGN.md`](../DESIGN.md) at the repository root (product context in [`PRODUCT.md`](../PRODUCT.md)). This file used to carry a second copy of the system; that copy is gone. Point agents at the root `DESIGN.md` and this directory's kit, and treat the root document as authoritative when they disagree.

What lives here:

- `brand.json` — the token source (palette roles, type, voice, imagery, posture rules including dark mode). `apps/web/src/lib/styles/tokens.css` is generated from it — never hand-edit the CSS.
- `system/` — the OpenDesign `--brand-*` kit derived from `brand.json`: tokens (`variables.css`, `tokens.*.json`, `seed.json`, `theme.json`), the component showcase (`kit.html`, `kit.dark.html`) and sample artifacts under `artifacts/`.
- `app-shell.html` — the app-shell design spec & interactive prototype: ink rail, page header, content states, mobile frames, bare pre-sign-in layout.

# UI → Bits UI migration audit (roadmap 6.6)

Prepared 2026-10-15 on `feature/73-bits-ui-migration` (cut from main at tag `pre-bits-ui`, commit `8939724`).
Scope: every Svelte component and route under `apps/web/src`.

**Status (updated 2026-10-15, step 2):** the three native `<dialog>`s are migrated to **Bits UI Dialog** (bits-ui 2.19.5) — ConfirmDialog, LinkDialog and UserSheet now keep their public props with the open state mirrored into a local `bitsOpen` via a function binding (`bind:open={getOpen, setOpen}`); ConfirmDialog's refuses-to-close-while-busy rule lives in that setter (every close attempt — Escape, outside click, Cancel — funnels through it). Shared overlay/card chrome moved to `src/lib/styles/dialogs.css` (imported once from the root layout), styled with `--minerva-*` tokens; portaled content is brought back into component scope with `child` snippets. The dev gallery gained deterministic busy (2 s simulated in-flight) and error (fails once, then succeeds) confirm demos. Remaining: the Sidebar user menu → Popover and the four tooltips → Tooltip (later steps).

## Bits UI version

**bits-ui 2.19.5** (latest on npm as of 2026-10-15). Peer dependency `svelte ^5.33.0`; the app is on `svelte ^5.56.1` with SvelteKit 2 — **Svelte 5 support confirmed**. The other peer, `@internationalized/date ^3.8.1`, is only required by the date components (Calendar / DatePicker / …), which Minerva does not use — not needed at install.

Rule applied (D18 + task constraint): recommend a Bits UI replacement **only where it removes custom behaviour code**. Layout and display components stay custom; public component props are preserved so pages do not change.

## Component audit

| Component / file | What it does today | Bits UI primitive that replaces it (or keep custom) | Props / events other files depend on | A11y behaviours that must be preserved |
|---|---|---|---|---|
| `ui/Button.svelte` | `<button>` or `<a>` (when `href` given); variants primary/secondary/ghost; disabled = real `disabled`, or `aria-disabled` + navigation blocked for links | **Keep custom** — Bits UI's Button adds nothing over a styled `<button>`; the only behaviour code is the ~10-line disabled-link branch | Everywhere: `variant`, `href`, `disabled`, `type`, `onclick`, `children`; rest-spread carries `data-sveltekit-reload` (login) and `title=` (people page) | Disabled link stays focusable but inert (`aria-disabled`, `preventDefault`) |
| `ui/ConfirmDialog.svelte` | Destructive-action confirm on a native `<dialog>`: `$effect` syncs `showModal()`/`close()`, backdrop click detected via `e.target === dialog`, **refuses to close while `busy`** (cancel `preventDefault` + backdrop guard); error line via FormAlert | **Bits UI Dialog** — migrated (step 2): the guarded function binding on `open` replaces the `$effect` sync, the backdrop-target detection and the manual close-refusal. (Destructive-only uses could later split to AlertDialog; today's set mixes destructive and role-change confirms, so Dialog is the honest base) | people, invitations, sso, dev gallery: `open`, `title`, `message`, `confirmLabel`, `busy`, `error`, `onConfirm`, `onClose` — all preserved | Focus trap; Escape closes (except while busy); backdrop click closes (except while busy); focus returns to the trigger on close; title announced via labelled-by; error is `role="alert"`; refuses-to-close-while-busy incl. disabled buttons |
| `ui/LinkDialog.svelte` | Show-once link dialog on a native `<dialog>`: copy button with clipboard-API fallback, readonly input that selects on focus/click, fine print, optional note + "emailed to" Banner | **Bits UI Dialog** — migrated (step 2): same removal as ConfirmDialog (sync `$effect`, backdrop detection). Copy/selection logic is content, it stayed | people, invitations, dev gallery: `open`, `title`, `link`, `finePrint`, `note`, `emailedTo`, `onClose`, `children` snippet — all preserved | Focus trap; Escape; backdrop click; focus return; labelled-by title; copy button's "Copied" state change announced; readonly input keeps its accessible name and select-on-focus |
| `ui/Input.svelte` | Styled `<input>`, bindable `value`, rest-spread for `type`/autocomplete/aria attrs | **Keep custom** — Bits UI has no textfield primitive; zero behaviour code to remove | Field, accept-invite (visually-hidden password-manager autofill anchor), rest-spread consumers | `aria-invalid` styling hook; placeholder colour |
| `ui/PasswordInput.svelte` | Input + eye toggle button (`aria-pressed`, label swap) | **Keep custom** — no Bits UI password field; ~5 lines of state | Field (via `input="password"`): `value` bindable + rest | Toggle's `aria-pressed` and Show/Hide label swap |
| `ui/Field.svelte` | Label + Input/PasswordInput + hint/error with computed `aria-describedby` (`{id}-hint`/`{id}-error`) and `aria-invalid` | **Keep custom** — Bits UI ships a Label but no form-field compound; the describedby computation is 3 lines | login, accept-invite, reset-password, invitations, sso: `id`, `label`, `hint`, `error`, `input`, bindable `value`, rest (autocomplete etc.) | `aria-describedby` wiring to hint/error; `aria-invalid` on error; label `for` pairing |
| `ui/FormAlert.svelte` | Form-level message, `role="alert"` + `tabindex="-1"` so a failed submit can move focus to it | **Keep custom** — display component, no Bits UI equivalent. **Load-bearing:** login / accept-invite / reset-password all do `document.querySelector('form [role="alert"]')?.focus()` after a failed submit — both attributes and the focusability must survive | All forms + ConfirmDialog's error slot: `children` only | `role="alert"` (assertive) + programmatic focus target |
| `ui/PageHeader.svelte` | h1 + description + actions/secondary snippet slots + document title | **Keep custom** — layout/display (D18) | Every page: `title`, `description`, `documentTitle`, `actions`, `secondary` | Single h1 per page; document title |
| `ui/Banner.svelte` | Neutral notice, `role="status"`, optional dismiss button (local state only) | **Keep custom** — display; the dismissal is 2 lines of local state, no primitive to replace it | pages + LinkDialog: `dismissible`, `children` | `role="status"` (polite); dismiss button has an accessible name |
| `ui/Skeleton.svelte`, `ui/PageSkeleton.svelte` | aria-hidden pulse placeholders; fixed 4-row card composition with `aria-busy` | **Keep custom** — display (D18) | PageSkeleton: no props; Skeleton: `variant`, `width` | `aria-hidden` on skeletons; card's `aria-busy` + "Loading content" label; reduced-motion stops the pulse |
| `states/StateCard.svelte` | Shared state frame: 48px icon, heading level, title, message, hint, actions snippet | **Keep custom** — display family (D18) | All states below: `icon`, `level`, `title`, `message`, `hint`, `actions` | Heading level (`h1`/`h2`/`h3`) stays correct per context |
| `states/EmptyState.svelte` | StateCard wrapper with default icon | **Keep custom** — display (D18) | pages + dev gallery: `icon`, `level`, `title`, `message`, `actions` | as StateCard |
| `states/ErrorState.svelte` | StateCard + "Try again" button when `onretry` given | **Keep custom** — display (D18); retry is a callback, not a primitive | pages, RouteError: `level`, `title`, `message`, `onretry`, `actions` | as StateCard; retry button reachable |
| `states/AccessDenied.svelte` | StateCard (lock icon) with standing hint + "Back home" | **Keep custom** — display (D18). Its `title=` in grep output is a component prop, not an HTML tooltip | admin layout guard, RouteError: `level`, `message`, `hint`, `actions` | as StateCard |
| `states/RouteError.svelte` | Dispatches on `page.status`: 403 → AccessDenied, 404 → ErrorState + home link, 502/others → retry (`location.reload()` at the root boundary, `invalidateAll()` in-shell) | **Keep custom** — page-level routing logic, not a primitive | Both error boundaries: `reload` flag | as the states it renders |
| `shell/Sidebar.svelte` (layout part) | Desktop sidebar: brand row, primary + admin nav, collapse state | **Keep custom** — layout (D18) | `(app)/+layout.svelte`: `user`, `initials`, `isAdmin`, `collapsed`, `pathname`, `signOutError`, `onToggleCollapse`, `onSignOut` — all preserved | nav landmarks with labels; `aria-current="page"` on active links |
| `shell/Sidebar.svelte` (user menu) | **Hand-rolled disclosure**: trigger with `aria-haspopup`/`aria-expanded`/`aria-controls`, `$effect` installing document-level `keydown` (Escape) + `mousedown` (outside click) listeners, manual focus return (`lastFocus`), absolutely-positioned panel with id block + one action | **Bits UI Popover** — removes the ~40 lines of manual keyboard / outside-click / focus-return bookkeeping. Popover rather than DropdownMenu: the panel holds a heading block plus one action and the code comment states it is deliberately a disclosure, not `role="menu"` | Same component's public props (above) — unchanged; internal `menuOpen`/refs go away | Trigger keeps `aria-haspopup`/`aria-expanded`/`aria-controls`; Escape closes and returns focus to trigger; outside click closes and returns focus; disclosure (not menu) semantics; sign-out error stays `role="alert"` inside the panel |
| `shell/UserSheet.svelte` | Mobile bottom sheet on a native `<dialog>`: `$effect` showModal/close sync, backdrop-click close, close button, id block, admin link, sign out + error | **Bits UI Dialog** — migrated (step 2): positioned as a bottom sheet in CSS (the inner content element is styled, never the floating wrapper). The sync `$effect` and backdrop detection are gone; trap/Escape/focus-return come from the primitive | `(app)/+layout.svelte`: `user`, `initials`, `open`, `signOutError`, `onClosed`, `onSignOut` — all preserved | Focus trap; Escape; backdrop click; focus returns to the avatar trigger on close; `aria-label="Account"`; close button accessible name; sign-out error `role="alert"` |
| `shell/TopBar.svelte` | Mobile top bar: wordmark + avatar button that opens the sheet | **Keep custom** — layout; one pass-through button, no behaviour code | `(app)/+layout.svelte`: `initials`, `onOpenSheet` | Avatar trigger has an accessible name ("Account menu") |
| `shell/TabBar.svelte` | Mobile bottom tab nav (5–6 items via `--tabs`), `aria-current` on active | **Keep custom** — navigation, not Bits UI Tabs (which is for in-page content panels) | `(app)/+layout.svelte`: `pathname`, `items` | `nav` labelled "Primary"; `aria-current="page"` |
| `shell/NavLink.svelte` | Sidebar nav link: icon + label, active state, `title=` tooltip when collapsed (label stays visually hidden, never `display:none`) | **Keep custom** for the link; its `title=` is a **Tooltip candidate** (see inventory) | Sidebar: `item`, `active`, `collapsed` | `aria-current="page"`; accessible name preserved when collapsed |
| `shell/Wordmark.svelte` | Brand artwork (light/dark variants, collapsed "M." text) | **Keep custom** — display (D18) | Sidebar, TopBar, BareLayout: `collapsed`, `small`, `big` | `aria-label="Minerva"` on the span |
| `shell/BareLayout.svelte` | Pre-sign-in frame: wordmark + one centred card; also hosts the root error page | **Keep custom** — layout (D18) | `(auth)/+layout.svelte`, root `+error.svelte`: `children` | `<main id="main">` landmark |
| `icons/Icon.svelte` | Inline monoline SVGs keyed by name, `aria-hidden` | **Keep custom** — Bits UI ships no icons; the single-component set is deliberate | Everywhere: `name`, `size` | Decorative (`aria-hidden`) — adjacent text carries meaning |
| `(app)/+layout.svelte` | Shell grid (one 900px breakpoint), skip link, sign-out flow, `beforeNavigate` closes the sheet | **Keep custom** — page-level logic and layout | — | Skip link; `main#main[tabindex="-1"]`; landmarks |
| `(app)/admin/+layout.svelte` | Admin sub-nav pill tabs above each page's PageHeader | **Keep custom** — navigation (Bits UI Tabs is for content panels) | — | `nav` labelled "Admin"; `aria-current="page"` |
| `(app)/admin/people/+page.svelte` | User table: per-row role draft `<select>` + Apply, deactivate/reactivate behind ConfirmDialog, show-once reset LinkDialog, SSO-managed badge | Page logic **keep custom**; the select and two tooltips are inventoried below; dialogs migrate via their components | — | Table semantics (`scope="col"`, `data-label` stacking); per-row select accessible name; disabled buttons with explanatory tooltips (see note) |
| `(app)/admin/invitations/+page.svelte` | Invite form (Field + role `<select>`), invite table, revoke ConfirmDialog, show-once LinkDialog | Same treatment as People | — | Form label pairing (`label for="invite-role"`); table semantics |
| `(app)/admin/sso/+page.svelte` | Add-rule form (Field + role `<select>`), inline row edit (raw input + role `<select>`, focus moved into the input on edit), first/last-rule ConfirmDialogs, `<details>` help section | Same treatment; plus: the inline-edit raw `<input class="group-input">` duplicates `Input.svelte`'s styles — **reuse the existing Input component** (codebase fix, not Bits UI) | — | Focus moves into the row when it switches to edit mode (keyboard users must not land on `<body>`); select accessible names; table semantics |
| `(auth)/login/+page.svelte` | Provider-driven sign-in: email/password form (Field ×2) or one button per redirect provider; client validation with focus moved to the first invalid field, then to the FormAlert after a failed submit | **Keep custom** — no dialogs/selects/tooltips; the focus management is page-level form behaviour | — | `role="alert"` focus target after failed submit; field errors announced via Field's describedby; `aria-busy` on the form while submitting |
| `(auth)/accept-invite/+page.svelte`, `(auth)/reset-password/+page.svelte` | `?token` pages: inspect → checking (Skeleton) / invalid / retryable / form phases; same validation + focus pattern as login | **Keep custom** — as login | — | Same as login; `aria-busy` "Checking your link" skeleton state |
| `(app)/dev/components/+page.svelte` | Dev-only gallery demoing every shared component, incl. both dialogs (static title ids, one mounted at a time) | **Keep custom**, but update in lockstep when ConfirmDialog/LinkDialog migrate — the "mount only one at a time / static id" constraint disappears with Bits UI's auto-generated ids | — | Demos must keep showing light/dark and the busy state |
| Placeholder pages ×5 (`dashboard`, `goals`, `milestones`, `my-tasks`, `tasks`) + `(app)/admin/+page.svelte` (307) + both error boundaries + root `+layout.svelte` | PageHeader + EmptyState only / redirects / CSS imports | **Keep custom** — nothing interactive | — | as their components |

## Native `<select>` inventory (4 places)

| # | Location | What it is | Verdict |
|---|---|---|---|
| 1 | `admin/people/+page.svelte:238` | Per-row role draft select — `aria-label="Role for {name}"`, disabled for self, manual `onchange` type-check writing into the `draftRoles` map, Apply button confirms | **Keep native** — no custom behaviour code to remove; a 3-option fixed list is exactly what a native select is for. The only removable line is the manual value re-validation (a typed binding would make it redundant) — marginal |
| 2 | `admin/invitations/+page.svelte:244` | Invite-form role — `<label for="invite-role">`, `bind:value`, disabled while busy | **Keep native** — plain `bind:value`; nothing to remove |
| 3 | `admin/sso/+page.svelte:272` | Add-rule form role — same shape as #2 | **Keep native** — same reason |
| 4 | `admin/sso/+page.svelte:321` | Inline row-edit role — `aria-label="Role"`, `bind:value={editing.role}` | **Keep native** — same reason |

If a later step wants popup-styled pickers for visual consistency with the migrated dialogs, Bits UI `Select` is the primitive; it is not recommended now because it removes no behaviour code. A future searchable people picker (the M4 assignee/owner UI over `GET /api/people`) would be `Combobox`.

## Native tooltip (`title=`) and disclosure inventory

Real HTML tooltips — 4 places:

| # | Location | What it is | Verdict |
|---|---|---|---|
| 1 | `shell/NavLink.svelte:15` | Collapsed sidebar nav label | **Bits UI Tooltip** candidate |
| 2 | `shell/Sidebar.svelte:80` | Collapse toggle (also has a swapping `aria-label`) | **Bits UI Tooltip** candidate |
| 3 | `admin/people/+page.svelte:273` | Disabled Deactivate button — "You can't deactivate yourself" | **Bits UI Tooltip** candidate — and a real fix: the native tooltip never shows today, because Button sets `pointer-events: none` on disabled buttons; a Bits UI Tooltip wrapping the button would actually display |
| 4 | `admin/people/+page.svelte:286` | Disabled Reset password button — "Deactivated accounts can't get a reset link" | Same as #3 |

Grep false positives (component props named `title`, not HTML attributes — no action): `PageHeader`, `EmptyState`/`ErrorState`/`StateCard`, `ConfirmDialog`, `LinkDialog`.

Native disclosures:

| Location | What it is | Verdict |
|---|---|---|
| `admin/sso/+page.svelte:360` | `<details class="help">` — "How group rules work" | **Keep native** — zero custom behaviour code; the platform disclosure already handles keyboard and state for free (Bits UI Collapsible would add markup and remove nothing) |

Native `<dialog>` elements — 3 places, all covered by the table above: `ui/ConfirmDialog.svelte:49`, `ui/LinkDialog.svelte:67`, `shell/UserSheet.svelte:43` → **Bits UI Dialog** — migrated (step 2); no `<dialog>` or `showModal` remains under `src/lib/components`.

## Cross-cutting notes for the implementation steps

1. **FormAlert is load-bearing**: `role="alert"` + `tabindex="-1"` — three auth pages focus `form [role="alert"]` after a failed submit. Keep both attributes and the focusability intact.
2. **Dev gallery in lockstep**: it demos both dialogs; its static title ids (`confirm-dialog-title`, `link-dialog-title`) and one-at-a-time mount constraint disappear with Bits UI's auto-generated ids — resolved in step 2, which also added the busy and error confirm demos.
3. **Tooltips need a provider**: one `Tooltip.Provider` ancestor (per app shell, or per use site).
4. **Portals + scoped styles**: Dialog/Popover/Tooltip content renders in a portal — Svelte scoped `<style>` won't reach it. Use the `child` snippet to render own elements (spreading `{...props}`) or target data attributes with `:global(...)`.
5. **Styling stays token-based**: all new primitive markup is styled with the existing `--minerva-*` tokens, matching D18 and the design system in `docs/design/`.

## Summary — what actually migrates

- 3 native `<dialog>`s → **Bits UI Dialog** (ConfirmDialog, LinkDialog, UserSheet) — **done (step 2)**
- 1 hand-rolled disclosure (~40 lines of manual keyboard/outside-click/focus code) → **Bits UI Popover** (Sidebar user menu)
- 4 `title=` tooltips → **Bits UI Tooltip** (2 in the shell; 2 on disabled buttons that currently never display at all)

Everything else stays custom: Button, Input/PasswordInput/Field/FormAlert, PageHeader/Banner/Skeletons, the states family, all shell layout and navigation, Icon, all four native `<select>`s, and the one native `<details>`.

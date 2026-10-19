<script lang="ts">
	import { onMount } from 'svelte';
	import { apiRequest } from '$lib/api';
	import type { ApiErr } from '$lib/api';
	import type { Invite, InviteCreated, Role, SsoGroupRule } from '$lib/server/api';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Banner from '$lib/components/ui/Banner.svelte';
	import Field from '$lib/components/ui/Field.svelte';
	import FormAlert from '$lib/components/ui/FormAlert.svelte';
	import PageSkeleton from '$lib/components/ui/PageSkeleton.svelte';
	import ConfirmDialog from '$lib/components/ui/ConfirmDialog.svelte';
	import LinkDialog from '$lib/components/ui/LinkDialog.svelte';
	import Select from '$lib/components/ui/Select.svelte';
	import EmptyState from '$lib/components/states/EmptyState.svelte';
	import ErrorState from '$lib/components/states/ErrorState.svelte';
	import { roleLabel } from '$lib/nav';

	const ROLES: Role[] = ['admin', 'staff', 'read_only'];
	const ROLE_OPTIONS = ROLES.map((r) => ({ value: r, label: roleLabel(r) }));

	// null = still loading. The list is fetched in the browser (like the
	// People screen): every action is a same-origin fetch too, so all of the
	// screen's data lives in one place.
	let invites = $state<Invite[] | null>(null);
	let loadError = $state(false);

	// Whether SSO group rules are active — decides which SSO note to show
	// under the form (specific when active, general when we couldn't tell).
	let ssoRules = $state<'active' | 'none' | 'unknown'>('unknown');

	// The invite form. One message per attempt, in the FormAlert.
	let email = $state('');
	let role = $state<Role>('read_only');
	let sending = $state(false);
	let formError = $state<string | null>(null);

	// The row with an in-flight request; while any action is in flight, every
	// control on the screen is disabled (one mutation at a time).
	let busyId = $state<string | null>(null);
	let pendingRevoke = $state<Invite | null>(null);
	let confirming = $state(false);
	let revokeError = $state<string | null>(null);
	const anyBusy = $derived(sending || confirming || busyId !== null);

	// The last outcome, announced to assistive tech: errors assertively
	// (FormAlert is role=alert), successes politely (Banner is role=status).
	let notice = $state<{ kind: 'error' | 'success'; text: string } | null>(null);

	// The one-time link, shown exactly once in the LinkDialog (no storage, no
	// logs). `reissued` switches the fine print to "the old link is dead".
	let linkFor = $state<{ invite: Invite; link: string; emailed: boolean; reissued: boolean } | null>(null);

	onMount(loadAll);

	async function loadAll() {
		invites = null;
		loadError = false;
		const [list, rules] = await Promise.all([
			apiRequest<Invite[]>('GET', '/api/invites'),
			apiRequest<SsoGroupRule[]>('GET', '/api/sso/group-rules')
		]);
		if (list.ok) invites = list.data;
		else loadError = true;
		ssoRules = rules.ok ? (rules.data.length > 0 ? 'active' : 'none') : 'unknown';
	}

	function fmtDate(iso: string): string {
		return new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}

	// Pending first (the ones that still need a link shared), then the rest
	// newest-first — the API already answers newest first, so this only groups.
	const STATUS_RANK: Record<Invite['status'], number> = { pending: 0, expired: 1, accepted: 2, revoked: 3 };
	const sorted = $derived(
		(invites ?? []).slice().sort(
			(a, b) => STATUS_RANK[a.status] - STATUS_RANK[b.status] || b.created_at.localeCompare(a.created_at)
		)
	);

	function statusLabel(s: Invite['status']): string {
		if (s === 'pending') return 'Pending';
		if (s === 'accepted') return 'Accepted';
		if (s === 'revoked') return 'Revoked';
		return 'Expired';
	}

	function statusLine(inv: Invite): string {
		if (inv.status === 'pending') return `Expires on ${fmtDate(inv.expires_at)}.`;
		if (inv.status === 'expired') return `Expired on ${fmtDate(inv.expires_at)}.`;
		if (inv.status === 'accepted') return inv.consumed_at ? `Joined on ${fmtDate(inv.consumed_at)}.` : '';
		return inv.revoked_at ? `Revoked on ${fmtDate(inv.revoked_at)}.` : '';
	}

	function replaceInvite(updated: Invite) {
		if (!invites) return;
		invites = invites.map((i) => (i.id === updated.id ? updated : i));
	}

	// The API's messages are plain language by design, but the UI never shows
	// a raw code or an untranslated one: known cases get their own wording,
	// everything else falls back to a generic line.
	function friendlyCreateError(err: ApiErr): string {
		if (err.status === 0) return "We couldn't reach the server. Check your connection and try again.";
		if (err.status === 400) return 'That doesn\'t look like a valid email address.';
		if (err.status === 429 && err.retryAfter !== undefined) {
			const minutes = Math.max(1, Math.ceil(err.retryAfter / 60));
			return `You've created a lot of links in a short time. Try again in ${minutes} minute${
				minutes === 1 ? '' : 's'
			}.`;
		}
		if (err.status === 409) {
			if (err.code === 'account_exists') return 'An account with that email already exists.';
			return 'That email already has an invitation waiting — re-issue or revoke it in the list below.';
		}
		return 'Something went wrong. Please try again.';
	}

	function friendlyRowError(err: ApiErr): string {
		if (err.status === 0) return "We couldn't reach the server. Check your connection and try again.";
		if (err.status === 404) return 'That invitation no longer exists.';
		if (err.status === 429 && err.retryAfter !== undefined) {
			const minutes = Math.max(1, Math.ceil(err.retryAfter / 60));
			return `You've created a lot of links in a short time. Try again in ${minutes} minute${
				minutes === 1 ? '' : 's'
			}.`;
		}
		if (err.status === 409) {
			// account_exists: the invited email registered in the meantime
			// (re-issue only); conflict: the invite was accepted.
			if (err.code === 'account_exists') return 'An account with that email now exists, so the link can\'t be re-issued.';
			return 'This person has already joined.';
		}
		return 'Something went wrong. Please try again.';
	}

	// --- actions -----------------------------------------------------------

	async function submitInvite(e: SubmitEvent) {
		e.preventDefault();
		if (anyBusy) return;
		formError = null;
		sending = true;
		const res = await apiRequest<InviteCreated>('POST', '/api/invites', { email: email.trim(), role });
		sending = false;
		if (res.ok) {
			email = '';
			role = 'read_only';
			linkFor = { invite: res.data.invite, link: res.data.link, emailed: res.data.emailed, reissued: false };
			// Prepend locally instead of reloading: the dialog is open and a
			// skeleton flash behind it would be noise.
			invites = invites ? [res.data.invite, ...invites] : [res.data.invite];
		} else {
			formError = friendlyCreateError(res);
		}
	}

	function openRevoke(inv: Invite) {
		if (anyBusy) return;
		revokeError = null;
		pendingRevoke = inv;
	}

	async function confirmRevoke() {
		if (!pendingRevoke || confirming) return;
		const inv = pendingRevoke;
		confirming = true;
		revokeError = null;
		busyId = inv.id;
		const res = await apiRequest<Invite>('POST', `/api/invites/${inv.id}/revoke`);
		busyId = null;
		confirming = false;
		if (res.ok) {
			replaceInvite(res.data);
			pendingRevoke = null;
			notice = { kind: 'success', text: `The invitation for ${inv.email} has been revoked.` };
		} else if (res.status === 409 || res.status === 404) {
			// The row went stale under us (accepted or gone): say so plainly
			// and refresh instead of arguing with a dead dialog.
			pendingRevoke = null;
			notice = {
				kind: 'error',
				text: res.status === 409 ? 'This person has already joined.' : 'That invitation no longer exists.'
			};
			void loadAll();
		} else {
			revokeError = friendlyRowError(res);
		}
	}

	async function reissue(inv: Invite) {
		if (anyBusy) return;
		busyId = inv.id;
		notice = null;
		const res = await apiRequest<InviteCreated>('POST', `/api/invites/${inv.id}/reissue`);
		busyId = null;
		if (res.ok) {
			// A re-issue is a fresh token row; the server revokes the old one
			// in the same step, so mark it revoked here exactly as the next
			// load would show it (the list keeps the history).
			const now = new Date().toISOString();
			invites = invites
				? [res.data.invite, ...invites.map((i) => (i.id === inv.id ? { ...i, status: 'revoked' as const, revoked_at: now } : i))]
				: [res.data.invite];
			linkFor = { invite: res.data.invite, link: res.data.link, emailed: res.data.emailed, reissued: true };
		} else {
			notice = { kind: 'error', text: friendlyRowError(res) };
			if (res.status === 409 || res.status === 404) void loadAll();
		}
	}

	function closeLink() {
		linkFor = null;
	}
</script>

<PageHeader title="Invitations" description="Invite people and share their sign-up links." />

{#if notice}
	<div class="notices">
		{#if notice.kind === 'error'}
			<FormAlert>{notice.text}</FormAlert>
		{:else}
			<Banner>{notice.text}</Banner>
		{/if}
	</div>
{/if}

<section class="card" aria-labelledby="invite-form-title">
	<h2 id="invite-form-title">Invite someone</h2>
	<form novalidate onsubmit={submitInvite}>
		{#if formError}
			<FormAlert>{formError}</FormAlert>
		{/if}
		<div class="form-grid">
			<Field
				id="invite-email"
				label="Email address"
				type="email"
				placeholder="name@school.edu"
				bind:value={email}
				disabled={anyBusy}
			/>
			<div class="field role-field">
				<label for="invite-role">Role</label>
				<Select id="invite-role" bind:value={role} options={ROLE_OPTIONS} disabled={anyBusy} />
			</div>
		</div>
		<div class="form-actions">
			<Button variant="primary" type="submit" disabled={anyBusy || email.trim() === ''}>
				{sending ? 'Sending…' : 'Invite'}
			</Button>
		</div>
	</form>
	<div class="help">
		<p>When they open the link, they set a password and are signed in straight away.</p>
		{#if ssoRules === 'active'}
			<p>
				SSO group rules are active on this server, so if this person signs in with single sign-on, their role
				comes from their groups instead of the invite's role.
			</p>
		{:else if ssoRules === 'unknown'}
			<p>
				If SSO group rules are active, a person invited here who signs in with single sign-on gets their role
				from their groups instead of the invite's role.
			</p>
		{/if}
	</div>
</section>

<h2 class="section-title">All invitations</h2>

{#if loadError}
	<ErrorState title="We couldn't load invitations" message="Check your connection and try again." onretry={loadAll} />
{:else if invites === null}
	<PageSkeleton />
{:else if sorted.length === 0}
	<EmptyState icon="invite" title="No invitations yet" message="People you invite will appear here." />
{:else}
	<div class="table-card">
		<table>
			<thead class="vh">
				<tr>
					<th scope="col">Email</th>
					<th scope="col">Role</th>
					<th scope="col">Status</th>
					<th scope="col">Sent</th>
					<th scope="col"><span class="vh">Actions</span></th>
				</tr>
			</thead>
			<tbody>
				{#each sorted as inv (inv.id)}
					<tr class:dim={inv.status === 'expired' || inv.status === 'revoked'}>
						<td data-label="Email" class="email">{inv.email}</td>
						<td data-label="Role">{roleLabel(inv.role)}</td>
						<td data-label="Status">
							<span
								class="badge"
								class:done={inv.status === 'accepted'}
								class:dead={inv.status === 'expired' || inv.status === 'revoked'}
							>
								{statusLabel(inv.status)}
							</span>
							<p class="helper">{statusLine(inv)}</p>
						</td>
						<td data-label="Sent" class="sent">{fmtDate(inv.created_at)}</td>
						<td data-label="" class="actions-cell">
							{#if inv.status === 'pending'}
								<Button variant="ghost" disabled={anyBusy} onclick={() => reissue(inv)}>
									Re-issue link
								</Button>
								<Button variant="secondary" disabled={anyBusy} onclick={() => openRevoke(inv)}>
									Revoke
								</Button>
							{/if}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}

<ConfirmDialog
	open={pendingRevoke !== null}
	title={pendingRevoke ? `Revoke ${pendingRevoke.email}'s invitation?` : ''}
	message="Their link stops working right away, and they can't join with it. You can't undo this."
	confirmLabel="Revoke"
	busy={confirming}
	error={revokeError}
	onConfirm={confirmRevoke}
	onClose={() => {
		pendingRevoke = null;
		revokeError = null;
	}}
/>

<!-- The invite link is shown exactly once: it lives in this dialog's state
     and nowhere else (no storage, no logs). -->
<LinkDialog
	open={linkFor !== null}
	title={linkFor?.reissued ? 'New invitation link' : 'Invitation link'}
	link={linkFor?.link ?? ''}
	finePrint={
		linkFor
			? linkFor.reissued
				? `The old link no longer works. This one stops working on ${fmtDate(linkFor.invite.expires_at)} and will not be shown again.`
				: `It stops working on ${fmtDate(linkFor.invite.expires_at)}. It will not be shown again — if it's lost, use "Re-issue link" in the list below.`
			: ''
	}
	note={
		linkFor && !linkFor.emailed
			? `Email isn't set up on this server, so copy this link and send it to ${linkFor.invite.email} yourself.`
			: null
	}
	emailedTo={linkFor?.emailed ? linkFor.invite.email : null}
	onClose={closeLink}
>
	{#if linkFor}
		Share this link with <strong>{linkFor.invite.email}</strong>. When they open it, they set a password and are
		signed in straight away.
	{/if}
</LinkDialog>

<style>
	.notices {
		margin-bottom: var(--minerva-space-4);
	}

	.card {
		background: var(--minerva-surface);
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		padding: 20px;
	}

	.card h2 {
		margin: 0;
		font-size: 17px;
	}

	form {
		margin-top: var(--minerva-space-4);
	}

	.form-grid {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 220px;
		gap: 16px;
		align-items: start;
	}

	.role-field {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.role-field > label {
		font-size: 13px;
		font-weight: 500;
		color: var(--minerva-fg-muted);
	}

	.form-actions {
		display: flex;
		margin-top: var(--minerva-space-4);
	}

	.help {
		margin-top: var(--minerva-space-4);
		padding-top: var(--minerva-space-3);
		border-top: var(--minerva-border-w) solid var(--minerva-border);
		font-size: 13px;
		color: var(--minerva-fg-muted);
	}

	.help p {
		margin: 0 0 6px;
		max-width: 90ch;
	}

	.help p:last-child {
		margin-bottom: 0;
	}

	.section-title {
		margin: var(--minerva-space-7) 0 var(--minerva-space-3);
		font-size: 17px;
	}

	.table-card {
		background: var(--minerva-surface);
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		overflow-x: auto;
	}

	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 14.5px;
	}

	th {
		text-align: left;
		font-size: 13px;
		font-weight: 500;
		color: var(--minerva-fg-muted);
		padding: 12px 16px;
		border-bottom: var(--minerva-border-w) solid var(--minerva-border);
		white-space: nowrap;
	}

	td {
		padding: 14px 16px;
		border-bottom: var(--minerva-border-w) solid var(--minerva-border);
		vertical-align: top;
	}

	tbody tr:last-child td {
		border-bottom: 0;
	}

	tr.dim {
		/* The badge's text carries the meaning; this is only a second cue. */
		opacity: 0.72;
	}

	.email {
		color: var(--minerva-fg-muted);
		word-break: break-all;
	}

	.sent {
		white-space: nowrap;
		color: var(--minerva-fg-muted);
	}

	.badge {
		display: inline-flex;
		align-items: center;
		padding: 3px 10px;
		border-radius: 999px;
		border: var(--minerva-border-w) solid var(--minerva-border);
		font-size: 12.5px;
		font-weight: 500;
		white-space: nowrap;
	}

	.badge.done {
		background: color-mix(in oklch, var(--minerva-accent) 10%, transparent);
		border-color: color-mix(in oklch, var(--minerva-accent) 35%, transparent);
	}

	.badge.dead {
		background: color-mix(in oklch, var(--minerva-fg) 8%, transparent);
	}

	.helper {
		margin: 6px 0 0;
		font-size: 13px;
		color: var(--minerva-fg-muted);
		max-width: 44ch;
	}

	.actions-cell {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}

	/* Below the shell's breakpoint the table becomes stacked cards: one
	   block per row, each cell a label/value line (data-label). */
	@media (max-width: 899.98px) {
		.form-grid {
			grid-template-columns: 1fr;
		}

		.table-card {
			overflow-x: visible;
			padding: 12px;
		}

		table,
		tbody,
		tr,
		td {
			display: block;
		}

		tr {
			border: var(--minerva-border-w) solid var(--minerva-border);
			border-radius: var(--minerva-radius);
			padding: 6px 4px;
			margin-bottom: 12px;
		}

		tbody tr:last-child {
			margin-bottom: 0;
		}

		td {
			display: flex;
			align-items: center;
			justify-content: space-between;
			flex-wrap: wrap;
			gap: 4px 12px;
			padding: 7px 12px;
			border-bottom: 0;
		}

		td::before {
			content: attr(data-label);
			font-size: 13px;
			color: var(--minerva-fg-muted);
			flex: 0 0 auto;
		}

		td[data-label='']::before {
			content: none;
		}

		.helper {
			flex-basis: 100%;
			margin-top: 2px;
		}
	}
</style>

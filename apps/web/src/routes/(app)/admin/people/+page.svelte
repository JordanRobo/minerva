<script lang="ts">
	import { onMount } from 'svelte';
	import { apiRequest } from '$lib/api';
	import type { ApiErr } from '$lib/api';
	import type { AdminUser, PasswordResetLink, Role } from '$lib/server/api';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Banner from '$lib/components/ui/Banner.svelte';
	import FormAlert from '$lib/components/ui/FormAlert.svelte';
	import PageSkeleton from '$lib/components/ui/PageSkeleton.svelte';
	import ConfirmDialog from '$lib/components/ui/ConfirmDialog.svelte';
	import LinkDialog from '$lib/components/ui/LinkDialog.svelte';
	import EmptyState from '$lib/components/states/EmptyState.svelte';
	import ErrorState from '$lib/components/states/ErrorState.svelte';
	import { roleLabel } from '$lib/nav';
	import type { PageProps } from './$types';

	let { data } = $props();
	const me = $derived(data.user);

	const ROLES: Role[] = ['admin', 'staff', 'read_only'];

	// null = still loading. The list is fetched in the browser (see
	// +page.server.ts): every action below is a same-origin fetch too, so all
	// of the screen's data lives in one place.
	let users = $state<AdminUser[] | null>(null);
	let loadError = $state(false);

	// The row with an in-flight request; while set, every row's action
	// controls are disabled (one mutation at a time — no double-submits).
	let busyId = $state<string | null>(null);

	// The last outcome, announced to assistive tech: errors assertively
	// (FormAlert is role=alert), successes politely (Banner is role=status).
	let notice = $state<{ kind: 'error' | 'success'; text: string } | null>(null);

	// Per-row role drafts: what the select shows before "Apply" confirms it.
	const draftRoles = $state(new Map<string, Role>());
	const draftRole = (u: AdminUser): Role => draftRoles.get(u.id) ?? u.role;

	type PendingAction =
		| { kind: 'role'; user: AdminUser; newRole: Role }
		| { kind: 'deactivate'; user: AdminUser };
	let pending = $state<PendingAction | null>(null);
	let pendingError = $state<string | null>(null);
	let confirming = $state(false);

	let resetFor = $state<AdminUser | null>(null);
	let resetLink = $state<PasswordResetLink | null>(null);
	let creatingReset = $state(false);

	onMount(loadUsers);

	async function loadUsers() {
		users = null;
		loadError = false;
		const res = await apiRequest<AdminUser[]>('GET', '/api/users');
		if (res.ok) users = res.data;
		else loadError = true;
	}

	function name(u: AdminUser): string {
		return u.display_name.trim() || u.email;
	}

	const isSelf = (u: AdminUser): boolean => u.id === me.id;

	function replaceUser(updated: AdminUser) {
		if (!users) return;
		users = users.map((u) => (u.id === updated.id ? updated : u));
		draftRoles.delete(updated.id);
	}

	// The API's messages are plain language by design, but the UI never shows
	// a raw code or an untranslated one: known cases get their own wording,
	// everything else falls back to a generic line. `surfaceMessage` is for
	// the reset-link 409s, whose message already says exactly what's wrong.
	function friendlyError(err: ApiErr, u: AdminUser, surfaceMessage = false): string {
		if (err.status === 0) return "We couldn't reach the server. Check your connection and try again.";
		if (err.status === 404) return `${name(u)} no longer exists.`;
		if (err.status === 429 && err.retryAfter !== undefined) {
			const minutes = Math.max(1, Math.ceil(err.retryAfter / 60));
			return `You've created a lot of links in a short time. Try again in ${minutes} minute${
				minutes === 1 ? '' : 's'
			}.`;
		}
		if (err.code === 'role_managed_by_sso') {
			return "This person's role is set by the groups in your school's sign-in system. Change it there, or remove all SSO group rules to edit it here.";
		}
		if (err.status === 409) {
			if (err.message.includes('at least one active administrator')) {
				return 'There must always be at least one active administrator.';
			}
			if (err.message.includes('your own')) {
				return "You can't change your own role or deactivate yourself.";
			}
			if (surfaceMessage) return cap(err.message);
		}
		return 'Something went wrong. Please try again.';
	}

	function cap(s: string): string {
		return s.charAt(0).toUpperCase() + s.slice(1);
	}

	// --- actions -----------------------------------------------------------

	function openRoleChange(u: AdminUser) {
		if (busyId) return;
		pendingError = null;
		pending = { kind: 'role', user: u, newRole: draftRole(u) };
	}

	function openDeactivate(u: AdminUser) {
		if (busyId) return;
		pendingError = null;
		pending = { kind: 'deactivate', user: u };
	}

	async function confirmPending() {
		if (!pending || confirming) return;
		const u = pending.user;
		const newRole = pending.kind === 'role' ? pending.newRole : null;
		confirming = true;
		pendingError = null;
		busyId = u.id;
		const res = newRole
			? await apiRequest<AdminUser>('PUT', `/api/users/${u.id}/role`, { role: newRole })
			: await apiRequest<AdminUser>('POST', `/api/users/${u.id}/deactivate`);
		busyId = null;
		confirming = false;
		if (res.ok) {
			replaceUser(res.data);
			pending = null;
			notice = newRole
				? { kind: 'success', text: `${name(u)}'s role is now ${roleLabel(res.data.role)}.` }
				: { kind: 'success', text: `${name(u)} has been deactivated.` };
		} else {
			pendingError = friendlyError(res, u);
			if (res.status === 404) void loadUsers(); // the row is gone; refresh the list
		}
	}

	async function reactivate(u: AdminUser) {
		if (busyId) return;
		busyId = u.id;
		notice = null;
		const res = await apiRequest<AdminUser>('POST', `/api/users/${u.id}/reactivate`);
		busyId = null;
		if (res.ok) {
			replaceUser(res.data);
			notice = { kind: 'success', text: `${name(u)} has been reactivated.` };
		} else {
			notice = { kind: 'error', text: friendlyError(res, u) };
			if (res.status === 404) void loadUsers();
		}
	}

	async function issueReset(u: AdminUser) {
		if (busyId || creatingReset) return;
		creatingReset = true;
		notice = null;
		const res = await apiRequest<PasswordResetLink>('POST', `/api/users/${u.id}/password-reset`);
		creatingReset = false;
		if (res.ok) {
			resetFor = u;
			resetLink = res.data;
		} else {
			notice = { kind: 'error', text: friendlyError(res, u, true) };
			if (res.status === 404) void loadUsers();
		}
	}

	function closeReset() {
		resetFor = null;
		resetLink = null;
	}

	function expiryText(iso: string): string {
		return new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}
</script>

<PageHeader title="People" description="Who can sign in, and what they can do." />

{#if notice}
	<div class="notices">
		{#if notice.kind === 'error'}
			<FormAlert>{notice.text}</FormAlert>
		{:else}
			<Banner>{notice.text}</Banner>
		{/if}
	</div>
{/if}

{#if loadError}
	<ErrorState
		title="We couldn't load people"
		message="Check your connection and try again."
		onretry={loadUsers}
	/>
{:else if users === null}
	<PageSkeleton />
{:else if users.length === 0}
	<EmptyState icon="users" title="No people yet" message="Accounts will appear here once they're created." />
{:else}
	<div class="table-card">
		<table>
			<thead class="vh">
				<tr>
					<th scope="col">Name</th>
					<th scope="col">Email</th>
					<th scope="col">Role</th>
					<th scope="col">Status</th>
					<th scope="col"><span class="vh">Actions</span></th>
				</tr>
			</thead>
			<tbody>
				{#each users as u (u.id)}
					{@const self = isSelf(u)}
					<tr class:deactivated={!u.active}>
						<td data-label="Name">
							<span class="pname">{name(u)}{#if self} (you){/if}</span>
						</td>
						<td data-label="Email" class="email">{u.email}</td>
						<td data-label="Role">
							{#if u.role_managed_by_sso}
								<span class="role-cell">
									{roleLabel(u.role)}
									<span class="badge sso">Managed by SSO</span>
								</span>
								<p class="helper">
									This person's role is set by the groups in your school's sign-in system. Change it there, or
									remove all SSO group rules to edit it here.
								</p>
							{:else}
								<span class="role-cell">
									<select
										class="role-select"
										aria-label="Role for {name(u)}"
										value={draftRole(u)}
										disabled={self}
										onchange={(e) => {
											const v = e.currentTarget.value;
											if (v === 'admin' || v === 'staff' || v === 'read_only') {
												draftRoles.set(u.id, v);
											}
										}}
									>
										{#each ROLES as r (r)}
											<option value={r}>{roleLabel(r)}</option>
										{/each}
									</select>
									{#if !self && draftRole(u) !== u.role}
										<Button variant="ghost" disabled={busyId !== null} onclick={() => openRoleChange(u)}>
											Apply
										</Button>
									{/if}
								</span>
								{#if self}
									<p class="helper">You can't change your own role.</p>
								{/if}
							{/if}
						</td>
						<td data-label="Status">
							<span class="badge" class:deactivated={!u.active}>{u.active ? 'Active' : 'Deactivated'}</span>
						</td>
						<td data-label="" class="actions-cell">
							{#if u.active}
								<Button
									variant="secondary"
									disabled={busyId !== null || self}
									title={self ? "You can't deactivate yourself" : undefined}
									onclick={() => openDeactivate(u)}
								>
									Deactivate
								</Button>
							{:else}
								<Button variant="secondary" disabled={busyId !== null} onclick={() => reactivate(u)}>
									Reactivate
								</Button>
							{/if}
							<Button
								variant="ghost"
								disabled={busyId !== null || creatingReset || !u.active}
								title={!u.active ? 'Deactivated accounts can\'t get a reset link' : undefined}
								onclick={() => issueReset(u)}
							>
								Reset password
							</Button>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}

<ConfirmDialog
	open={pending !== null}
	title={
		pending?.kind === 'role'
			? `Change ${name(pending.user)}'s role?`
			: pending
				? `Deactivate ${name(pending.user)}?`
				: ''
	}
	message={
		pending?.kind === 'role'
			? `${name(pending.user)} will go from ${roleLabel(pending.user.role)} to ${roleLabel(pending.newRole)}. The change applies right away.`
			: pending
				? "They'll be signed out on every device immediately and won't be able to sign in until you reactivate them."
				: ''
	}
	confirmLabel={pending?.kind === 'role' ? 'Change role' : 'Deactivate'}
	busy={confirming}
	error={pendingError}
	onConfirm={confirmPending}
	onClose={() => {
		pending = null;
		pendingError = null;
	}}
/>

<!-- The reset link is shown exactly once: it lives in this dialog's state
     and nowhere else (no storage, no logs). -->
<LinkDialog
	open={resetFor !== null && resetLink !== null}
	title="Password reset link"
	link={resetLink?.link ?? ''}
	finePrint={
		resetLink
			? `It stops working on ${expiryText(resetLink.expires_at)}. It will not be shown again — if you close this without copying it, create a new one.`
			: ''
	}
	emailedTo={resetLink?.emailed && resetFor ? resetFor.email : null}
	onClose={closeReset}
>
	{#if resetFor}
		Share this link with <strong>{name(resetFor)}</strong>. It opens the screen where they set a new password.
	{/if}
</LinkDialog>

<style>
	.notices {
		margin-bottom: var(--minerva-space-4);
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

	tr.deactivated {
		/* The badge's text carries the meaning; this is only a second cue. */
		opacity: 0.72;
	}

	.pname {
		font-weight: 500;
	}

	.email {
		color: var(--minerva-fg-muted);
		word-break: break-all;
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

	.badge.deactivated {
		background: color-mix(in oklch, var(--minerva-fg) 8%, transparent);
	}

	.badge.sso {
		margin-left: 8px;
		background: color-mix(in oklch, var(--minerva-accent) 10%, transparent);
		border-color: color-mix(in oklch, var(--minerva-accent) 35%, transparent);
	}

	.role-cell {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}

	.role-select {
		min-height: var(--minerva-control-h);
		padding: 6px 12px;
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		background: var(--minerva-surface);
		color: var(--minerva-fg);
		font-family: inherit;
		font-size: 14.5px;
	}

	.role-select:focus {
		outline: 2px solid var(--minerva-accent);
		border-color: var(--minerva-accent);
	}

	.role-select[disabled] {
		opacity: 0.55;
		cursor: not-allowed;
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

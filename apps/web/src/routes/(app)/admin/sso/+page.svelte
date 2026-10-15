<script lang="ts">
	import { onMount } from 'svelte';
	import { apiRequest, postJson } from '$lib/api';
	import type { ApiErr } from '$lib/api';
	import type { AuthProvider, Role, SsoGroupRule } from '$lib/server/api';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Banner from '$lib/components/ui/Banner.svelte';
	import Field from '$lib/components/ui/Field.svelte';
	import Input from '$lib/components/ui/Input.svelte';
	import FormAlert from '$lib/components/ui/FormAlert.svelte';
	import PageSkeleton from '$lib/components/ui/PageSkeleton.svelte';
	import ConfirmDialog from '$lib/components/ui/ConfirmDialog.svelte';
	import EmptyState from '$lib/components/states/EmptyState.svelte';
	import ErrorState from '$lib/components/states/ErrorState.svelte';
	import { roleLabel } from '$lib/nav';

	const ROLES: Role[] = ['admin', 'staff', 'read_only'];

	// null = still loading. Like the other admin screens, everything is a
	// same-origin browser fetch, so all of the screen's data lives here.
	let rules = $state<SsoGroupRule[] | null>(null);
	let loadError = $state(false);

	// Whether a redirect (SSO) provider is registered — the "not turned on"
	// note only shows when we positively know there isn't one (null = couldn't tell).
	let ssoOn = $state<boolean | null>(null);

	// The add form. One message per attempt, in the FormAlert.
	let groupName = $state('');
	let role = $state<Role>('read_only');
	let sending = $state(false);
	let formError = $state<string | null>(null);

	// One row edits at a time; `editing` holds that row's draft.
	let editing = $state<{ rule: SsoGroupRule; name: string; role: Role } | null>(null);
	let editInput = $state<HTMLInputElement | null>(null);

	// The row with an in-flight request; while any action is in flight, every
	// control on the screen is disabled (one mutation at a time).
	let busyId = $state<string | null>(null);
	let confirming = $state(false);
	type Pending = { kind: 'first-rule' } | { kind: 'delete'; rule: SsoGroupRule; isLast: boolean };
	let pending = $state<Pending | null>(null);
	let pendingError = $state<string | null>(null);

	const anyBusy = $derived(sending || confirming || busyId !== null);

	// The last outcome, announced to assistive tech: errors assertively
	// (FormAlert is role=alert), successes politely (Banner is role=status).
	let notice = $state<{ kind: 'error' | 'success'; text: string } | null>(null);

	onMount(loadAll);

	async function loadAll() {
		rules = null;
		loadError = false;
		editing = null;
		const [list, providers] = await Promise.all([
			apiRequest<SsoGroupRule[]>('GET', '/api/sso/group-rules'),
			apiRequest<{ providers: AuthProvider[] }>('GET', '/api/auth/providers')
		]);
		if (list.ok) rules = list.data;
		else loadError = true;
		ssoOn = providers.ok ? providers.data.providers.some((p) => p.kind === 'redirect') : null;
	}

	const byName = (a: SsoGroupRule, b: SsoGroupRule) => a.group_name.localeCompare(b.group_name);

	function validName(n: string): boolean {
		const t = n.trim();
		return t.length >= 1 && t.length <= 255;
	}

	const nameProblem = $derived(
		groupName.trim() !== '' && groupName.trim().length > 255
			? 'Group names can be at most 255 characters.'
			: undefined
	);

	// The API's messages are plain language by design, but the UI never shows
	// a raw code or an untranslated one: known cases get their own wording,
	// everything else falls back to a generic line.
	function friendlyError(err: ApiErr): string {
		if (err.status === 0) return "We couldn't reach the server. Check your connection and try again.";
		if (err.status === 400) return 'That group name isn\'t valid — it needs 1 to 255 characters.';
		if (err.status === 404) return 'That rule no longer exists.';
		if (err.status === 409 && err.code === 'group_rule_exists') return "There's already a rule for that group.";
		return 'Something went wrong. Please try again.';
	}

	function savedText(r: SsoGroupRule): string {
		return `The rule for "${r.group_name}" is now set to ${roleLabel(r.role)}.`;
	}

	// --- actions -----------------------------------------------------------

	async function createRule() {
		sending = true;
		const res = await postJson<SsoGroupRule>('/api/sso/group-rules', { group_name: groupName.trim(), role });
		sending = false;
		return res;
	}

	function onCreated(rule: SsoGroupRule) {
		groupName = '';
		role = 'read_only';
		rules = rules ? [...rules, rule].sort(byName) : [rule];
		notice = { kind: 'success', text: savedText(rule) };
	}

	async function submitRule(e: SubmitEvent) {
		e.preventDefault();
		if (anyBusy || !validName(groupName)) return;
		formError = null;
		if ((rules ?? []).length === 0) {
			// The first rule switches the whole mechanism on — confirm that first.
			pendingError = null;
			pending = { kind: 'first-rule' };
			return;
		}
		const res = await createRule();
		if (res.ok) onCreated(res.data);
		else formError = friendlyError(res);
	}

	function openEdit(r: SsoGroupRule) {
		if (anyBusy || editing) return;
		editing = { rule: r, name: r.group_name, role: r.role };
	}

	// Move focus into the row when it switches to edit mode, so keyboard users
	// don't land on <body>.
	$effect(() => {
		if (editing && editInput) editInput.focus();
	});

	const editValid = $derived(editing !== null && validName(editing.name));

	async function saveRule() {
		if (!editing || anyBusy || !editValid) return;
		const r = editing.rule;
		busyId = r.id;
		notice = null;
		const res = await apiRequest<SsoGroupRule>('PUT', `/api/sso/group-rules/${r.id}`, {
			group_name: editing.name.trim(),
			role: editing.role
		});
		busyId = null;
		if (res.ok) {
			editing = null;
			rules = rules ? rules.map((x) => (x.id === res.data.id ? res.data : x)).sort(byName) : [res.data];
			notice = { kind: 'success', text: savedText(res.data) };
		} else {
			notice = { kind: 'error', text: friendlyError(res) };
			if (res.status === 404) void loadAll();
		}
	}

	function openDelete(r: SsoGroupRule) {
		if (anyBusy || editing) return;
		pendingError = null;
		pending = { kind: 'delete', rule: r, isLast: (rules ?? []).length === 1 };
	}

	async function confirmPending() {
		if (!pending || confirming) return;
		const p = pending;
		confirming = true;
		pendingError = null;
		if (p.kind === 'first-rule') {
			const res = await createRule();
			confirming = false;
			if (res.ok) {
				onCreated(res.data);
				pending = null;
			} else {
				pendingError = friendlyError(res);
			}
			return;
		}
		busyId = p.rule.id;
		const res = await apiRequest<null>('DELETE', `/api/sso/group-rules/${p.rule.id}`);
		busyId = null;
		confirming = false;
		if (res.ok) {
			rules = rules ? rules.filter((x) => x.id !== p.rule.id) : [];
			pending = null;
			notice = { kind: 'success', text: `The rule for "${p.rule.group_name}" was removed.` };
		} else if (res.status === 404) {
			pending = null;
			notice = { kind: 'error', text: 'That rule no longer exists.' };
			void loadAll();
		} else {
			pendingError = friendlyError(res);
		}
	}

	const confirmTitle = $derived(
		pending?.kind === 'first-rule'
			? 'Turn on group rules?'
			: pending
				? pending.isLast
					? 'Remove the last rule?'
					: `Remove the rule for "${pending.rule.group_name}"?`
				: ''
	);

	const confirmMessage = $derived(
		pending?.kind === 'first-rule'
			? 'This is the first rule. From now on, each time someone signs in with single sign-on, their role will be re-set from their groups — and roles set that way can no longer be edited by hand on the People screen.'
			: pending
				? pending.isLast
					? 'Group rules will switch off: SSO sign-ins will stop changing roles, and every role becomes editable by hand on the People screen again.'
					: 'The next time people in that group sign in with single sign-on, their role will be re-set from their other groups — or to Read-only if none of those have a rule.'
				: ''
	);
</script>

<PageHeader title="Single sign-on" description="Match groups from your school's sign-in system to roles." />

{#if !loadError && rules !== null}
	<Banner>
		{rules.length === 0
			? 'No rules set — everyone keeps the role they already have; SSO sign-ins never change roles.'
			: 'Rules are active — each time someone signs in with single sign-on, their role is re-set from the groups they belong to.'}
	</Banner>
{/if}

{#if !loadError && ssoOn === false}
	<p class="provider-note">
		Single sign-on isn't turned on on this server yet (it's configured in minerva.toml). You can still add group
		rules now — they'll apply as soon as it is.
	</p>
{/if}

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
		title="We couldn't load group rules"
		message="Check your connection and try again."
		onretry={loadAll}
	/>
{:else if rules === null}
	<PageSkeleton />
{:else}
	<section class="card" aria-labelledby="add-rule-title">
		<h2 id="add-rule-title">Add a rule</h2>
		<form novalidate onsubmit={submitRule}>
			{#if formError}
				<FormAlert>{formError}</FormAlert>
			{/if}
			<div class="form-grid">
				<Field
					id="sso-rule-group"
					label="Group name"
					placeholder="e.g. teachers"
					bind:value={groupName}
					disabled={anyBusy}
					error={nameProblem}
				/>
				<div class="field role-field">
					<label for="sso-rule-role">Role</label>
					<select id="sso-rule-role" class="role-select" bind:value={role} disabled={anyBusy}>
						{#each ROLES as r (r)}
							<option value={r}>{roleLabel(r)}</option>
						{/each}
					</select>
				</div>
			</div>
			<div class="form-actions">
				<Button variant="primary" type="submit" disabled={anyBusy || !validName(groupName)}>
					{sending ? 'Adding…' : 'Add rule'}
				</Button>
			</div>
		</form>
	</section>

	<h2 class="section-title">All rules</h2>

	{#if rules.length === 0}
		<EmptyState
			icon="sso"
			title="No rules yet"
			message="Add a rule to decide which role people in a group get when they sign in with single sign-on."
		/>
	{:else}
		<div class="table-card">
			<table>
				<thead class="vh">
					<tr>
						<th scope="col">Group name</th>
						<th scope="col">Role</th>
						<th scope="col"><span class="vh">Actions</span></th>
					</tr>
				</thead>
				<tbody>
					{#each rules as r (r.id)}
						<tr>
							{#if editing && editing.rule.id === r.id}
								<td data-label="Group name">
									<Input bind:ref={editInput} bind:value={editing.name} aria-label="Group name" />
									{#if editing.name.trim().length > 255}
										<p class="helper">Group names can be at most 255 characters.</p>
									{/if}
								</td>
								<td data-label="Role">
									<select class="role-select" bind:value={editing.role} aria-label="Role">
										{#each ROLES as roleOption (roleOption)}
											<option value={roleOption}>{roleLabel(roleOption)}</option>
										{/each}
									</select>
								</td>
								<td data-label="" class="actions-cell">
									<Button variant="secondary" disabled={anyBusy || !editValid} onclick={saveRule}>
										Save
									</Button>
									<Button variant="ghost" disabled={anyBusy} onclick={() => (editing = null)}>
										Cancel
									</Button>
								</td>
							{:else}
								<td data-label="Group name">
									<span class="gname">{r.group_name}</span>
								</td>
								<td data-label="Role">{roleLabel(r.role)}</td>
								<td data-label="" class="actions-cell">
									<Button variant="ghost" disabled={anyBusy || editing !== null} onclick={() => openEdit(r)}>
										Edit
									</Button>
									<Button
										variant="secondary"
										disabled={anyBusy || editing !== null}
										onclick={() => openDelete(r)}
									>
										Delete
									</Button>
								</td>
							{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	<details class="help">
		<summary>How group rules work</summary>
		<ul>
			<li>
				While at least one rule exists, each time someone signs in with single sign-on, their role is re-set from
				the groups their school's sign-in system says they belong to.
			</li>
			<li>
				If several of their groups have rules, the least permissive role applies — Read-only, then Staff, then
				Administrator.
			</li>
			<li>Group names must match exactly: capitals matter, and spaces at the start or end are ignored.</li>
			<li>
				If none of their groups has a rule — or their sign-in doesn't include any groups — they get Read-only. Add a
				rule for one of their groups to give them more access.
			</li>
			<li>
				Roles set this way can't be edited by hand on the People screen; they're shown as "Managed by SSO". Remove
				every rule to edit roles there again.
			</li>
			<li>
				The first administrator account (the one created when Minerva was set up) is never changed by group rules,
				and the last active administrator is never demoted — if a rule would do that, their role is left as it is.
			</li>
			<li>
				For any of this to work, your school's sign-in system must be set up to send each person's groups in its ID
				token (see the setup documentation). Minerva can't check that from here.
			</li>
		</ul>
	</details>
{/if}

<ConfirmDialog
	open={pending !== null}
	title={confirmTitle}
	message={confirmMessage}
	confirmLabel={pending?.kind === 'first-rule' ? 'Add rule' : 'Remove rule'}
	busy={confirming}
	error={pendingError}
	onConfirm={confirmPending}
	onClose={() => {
		pending = null;
		pendingError = null;
	}}
/>

<style>
	.provider-note {
		margin: 0 0 var(--minerva-space-4);
		font-size: 14px;
		color: var(--minerva-fg-muted);
		max-width: 90ch;
	}

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

	.form-actions {
		display: flex;
		margin-top: var(--minerva-space-4);
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

	.gname {
		font-weight: 500;
		word-break: break-all;
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

	.help {
		margin-top: var(--minerva-space-5);
	}

	.help summary {
		cursor: pointer;
		font-size: 14.5px;
		font-weight: 500;
	}

	.help ul {
		margin: 10px 0 0;
		padding-left: 20px;
		display: grid;
		gap: 6px;
		font-size: 13.5px;
		color: var(--minerva-fg-muted);
		max-width: 90ch;
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

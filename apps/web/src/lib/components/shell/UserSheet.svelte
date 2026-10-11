<script lang="ts">
	import Icon from '$lib/components/icons/Icon.svelte';
	import { adminNav, roleLabel } from '$lib/nav';
	import type { CurrentUser } from '$lib/server/api';

	let {
		user,
		initials,
		open,
		signOutError,
		onClosed,
		onSignOut
	}: {
		user: CurrentUser;
		initials: string;
		open: boolean;
		signOutError: string | null;
		onClosed: () => void;
		onSignOut: () => void;
	} = $props();

	const name = $derived(user.display_name || user.email);

	// Native <dialog>: showModal() brings the focus trap, Escape, the
	// ::backdrop and inert siblings for free. Closing (Escape, backdrop,
	// Close button) fires the `close` event, which bubbles back up as
	// onClosed. When focus returns on close, the browser hands it back to
	// the trigger — no manual bookkeeping needed.
	let dialog = $state<HTMLDialogElement | null>(null);

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) dialog.showModal();
		else if (!open && dialog.open) dialog.close();
	});

	function onBackdropClick(e: MouseEvent) {
		// A click on the ::backdrop targets the dialog element itself.
		if (e.target === dialog) dialog?.close();
	}
</script>

<dialog
	class="sheet"
	bind:this={dialog}
	aria-label="Account"
	onclose={onClosed}
	onclick={onBackdropClick}
>
	<button class="sheet-close" type="button" aria-label="Close menu" onclick={() => dialog?.close()}>
		<Icon name="close" size={15} />
	</button>

	<div class="sheet-id">
		<span class="avatar" aria-hidden="true">{initials}</span>
		<div class="sheet-id-text">
			<p class="um-name">{name}</p>
			<p class="um-email">{user.email}</p>
			<p class="um-role">{roleLabel(user.role)}</p>
		</div>
	</div>

	<hr class="rule" />

	{#if user.role === 'admin'}
		<p class="sheet-group-label">Admin</p>
		{#each adminNav as item (item.href)}
			<a class="um-item" href={item.href}>
				<Icon name={item.icon} />{item.label}
			</a>
		{/each}
		<hr class="rule" />
	{/if}

	<button class="um-item" type="button" onclick={onSignOut}>
		<Icon name="signout" />Sign out
	</button>

	{#if signOutError}
		<p class="um-error" role="alert">{signOutError}</p>
	{/if}
</dialog>

<style>
	.sheet {
		position: fixed;
		inset: auto 0 0 0;
		width: 100%;
		margin: 0;
		border: 0;
		border-top: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius) var(--minerva-radius) 0 0;
		background: var(--minerva-surface);
		color: var(--minerva-fg);
		padding: var(--minerva-space-2) var(--minerva-space-4)
			calc(var(--minerva-space-4) + env(safe-area-inset-bottom));
	}

	.sheet::backdrop {
		background: color-mix(in oklch, var(--minerva-fg) 40%, transparent);
	}

	.sheet-close {
		position: absolute;
		top: var(--minerva-space-2);
		right: var(--minerva-space-2);
		width: 34px;
		height: 34px;
		display: grid;
		place-items: center;
		border-radius: 8px;
		color: var(--minerva-fg-muted);
	}

	.sheet-close:hover {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
		color: var(--minerva-fg);
	}

	.sheet-id {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 12px 40px 10px var(--minerva-space-1);
	}

	.avatar {
		width: 40px;
		height: 40px;
		flex: 0 0 auto;
		border-radius: 50%;
		display: grid;
		place-items: center;
		border: var(--minerva-border-w) solid var(--minerva-border);
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
		font-size: 14px;
		font-weight: 600;
		color: var(--minerva-fg);
	}

	.sheet-id-text {
		min-width: 0;
	}

	.um-name {
		font-size: 15px;
		font-weight: 600;
		color: var(--minerva-fg);
		margin: 0;
	}

	.um-email {
		font-size: 13px;
		color: var(--minerva-fg-muted);
		margin: 0;
	}

	.um-role {
		font-size: 12px;
		color: var(--minerva-fg-muted);
		margin: 2px 0 0;
	}

	.rule {
		border: 0;
		border-top: var(--minerva-border-w) solid var(--minerva-border);
		margin: 10px 0;
	}

	.sheet-group-label {
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--minerva-fg-muted);
		padding: var(--minerva-space-1) 12px 6px;
		margin: 0;
	}

	.um-item {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		padding: 9px 12px;
		border-radius: 8px;
		font-size: 14px;
		color: var(--minerva-fg);
		text-align: left;
	}

	.um-item:hover {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
	}

	.um-item :global(svg) {
		color: var(--minerva-fg-muted);
	}

	.um-error {
		font-size: 13px;
		color: var(--minerva-accent);
		padding: 8px 12px 4px;
		margin: 0;
	}
</style>

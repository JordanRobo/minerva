<script lang="ts">
	import { Dialog } from 'bits-ui';
	import Icon from '$lib/components/icons/Icon.svelte';
	import { adminEntry, roleLabel } from '$lib/nav';
	import type { CurrentUser } from '$lib/server/api';

	// Mobile bottom sheet on Bits UI Dialog (D18): the primitive brings the
	// focus trap, Escape, outside-click and focus return to the trigger — no
	// manual bookkeeping. Positioned as a bottom sheet in CSS (the inner
	// content element, never the floating wrapper).
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

	let bitsOpen = $state(false);

	function setOpen(next: boolean) {
		bitsOpen = next;
		if (!next) onClosed();
	}

	const getOpen = () => bitsOpen;

	// The parent drives `open` one way (it also closes the sheet on
	// navigation). Mirror it into Bits UI's state in both directions: a parent
	// close must not re-fire onClosed, which is reserved for user-initiated
	// closes.
	$effect(() => {
		if (open && !bitsOpen) bitsOpen = true;
		else if (!open && bitsOpen) bitsOpen = false;
	});
</script>

<Dialog.Root bind:open={getOpen, setOpen}>
	<Dialog.Portal>
		<Dialog.Overlay />
		<Dialog.Content aria-label="Account">
			{#snippet child({ props })}
				<div {...props} class="sheet">
					<button class="sheet-close" type="button" aria-label="Close menu" onclick={() => setOpen(false)}>
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
						<a class="um-item" href={adminEntry.href}>
							<Icon name={adminEntry.icon} />{adminEntry.label}
						</a>
						<hr class="rule" />
					{/if}

					<button class="um-item" type="button" onclick={onSignOut}>
						<Icon name="signout" />Sign out
					</button>

					{#if signOutError}
						<p class="um-error" role="alert">{signOutError}</p>
					{/if}
				</div>
			{/snippet}
		</Dialog.Content>
	</Dialog.Portal>
</Dialog.Root>

<style>
	.sheet {
		position: fixed;
		inset: auto 0 0 0;
		width: 100%;
		border-top: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius) var(--minerva-radius) 0 0;
		background: var(--minerva-surface);
		color: var(--minerva-fg);
		padding: var(--minerva-space-2) var(--minerva-space-4)
			calc(var(--minerva-space-4) + env(safe-area-inset-bottom));
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

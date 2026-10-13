<script lang="ts">
	import Button from './Button.svelte';
	import FormAlert from './FormAlert.svelte';

	// A destructive-action confirmation on a native <dialog>: showModal()
	// brings the focus trap, Escape and the ::backdrop for free (the UserSheet
	// pattern). Mount only one at a time — the title id is static. While
	// `busy` the dialog refuses to close, so an in-flight request can't be
	// abandoned mid-write.
	let {
		open,
		title,
		message,
		confirmLabel = 'Confirm',
		busy = false,
		error,
		onConfirm,
		onClose
	}: {
		open: boolean;
		title: string;
		message: string;
		confirmLabel?: string;
		busy?: boolean;
		error?: string | null;
		onConfirm: () => void;
		onClose: () => void;
	} = $props();

	let dialog = $state<HTMLDialogElement | null>(null);

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) dialog.showModal();
		else if (!open && dialog.open) dialog.close();
	});

	function refuseCloseWhileBusy(e: Event) {
		if (busy) e.preventDefault();
	}

	function onBackdropClick(e: MouseEvent) {
		// A click on the ::backdrop targets the dialog element itself.
		if (!dialog || busy) return;
		if (e.target === dialog) dialog.close();
	}
</script>

<dialog
	class="confirm"
	bind:this={dialog}
	aria-labelledby="confirm-dialog-title"
	oncancel={refuseCloseWhileBusy}
	onclose={onClose}
	onclick={onBackdropClick}
>
	<h2 id="confirm-dialog-title">{title}</h2>
	<p>{message}</p>
	{#if error}
		<FormAlert>{error}</FormAlert>
	{/if}
	<div class="actions">
		<Button variant="secondary" disabled={busy} onclick={onClose}>Cancel</Button>
		<Button variant="primary" disabled={busy} onclick={onConfirm}>{confirmLabel}</Button>
	</div>
</dialog>

<style>
	.confirm {
		width: min(480px, calc(100vw - 32px));
		margin: auto;
		padding: var(--minerva-space-6);
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		background: var(--minerva-surface);
		color: var(--minerva-fg);
	}

	.confirm::backdrop {
		background: color-mix(in oklch, var(--minerva-fg) 40%, transparent);
	}

	h2 {
		margin: 0;
		font-size: 19px;
	}

	.confirm > p {
		margin: 10px 0 0;
		font-size: 15px;
		color: var(--minerva-fg-muted);
	}

	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 10px;
		margin-top: var(--minerva-space-5);
	}
</style>

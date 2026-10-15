<script lang="ts">
	import { Dialog } from 'bits-ui';
	import Button from './Button.svelte';
	import FormAlert from './FormAlert.svelte';

	// A destructive-action confirmation on Bits UI Dialog (D18): the primitive
	// brings the focus trap, Escape, outside-click and focus return to the
	// trigger. While `busy` the dialog refuses to close — every close attempt
	// (Escape, backdrop, Cancel) funnels through setOpen, which rejects it — so
	// an in-flight request can't be abandoned mid-write.
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

	let bitsOpen = $state(false);

	function setOpen(next: boolean) {
		if (!next && busy) return;
		bitsOpen = next;
		if (!next) onClose();
	}

	const getOpen = () => bitsOpen;

	// The parent drives `open` one way. Mirror it into Bits UI's state in both
	// directions: a parent close (e.g. after a successful confirm) must not
	// re-fire onClose, which is reserved for user-initiated closes.
	$effect(() => {
		if (open && !bitsOpen) bitsOpen = true;
		else if (!open && bitsOpen) bitsOpen = false;
	});
</script>

<Dialog.Root bind:open={getOpen, setOpen}>
	<Dialog.Portal>
		<Dialog.Overlay />
		<Dialog.Content>
			{#snippet child({ props })}
				<div {...props} class="confirm minerva-dialog-card">
					<Dialog.Title>
						{#snippet child({ props })}
							<h2 {...props}>{title}</h2>
						{/snippet}
					</Dialog.Title>
					<p>{message}</p>
					{#if error}
						<FormAlert>{error}</FormAlert>
					{/if}
					<div class="actions">
						<Button variant="secondary" disabled={busy} onclick={() => setOpen(false)}>Cancel</Button>
						<Button variant="primary" disabled={busy} onclick={onConfirm}>{confirmLabel}</Button>
					</div>
				</div>
			{/snippet}
		</Dialog.Content>
	</Dialog.Portal>
</Dialog.Root>

<style>
	.confirm {
		width: min(480px, calc(100vw - 32px));
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

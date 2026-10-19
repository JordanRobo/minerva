<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Dialog } from 'bits-ui';
	import Button from './Button.svelte';
	import Banner from './Banner.svelte';

	// The one-time link dialog (password resets, invitations) on Bits UI
	// Dialog (D18): the raw token lives in the parent's state and is shown
	// exactly once, here — never stored or logged. `note` is an instruction
	// line above the link (e.g. "email isn't set up, send it by hand");
	// `emailedTo` shows the "also sent by email" banner; `children` is the
	// intro sentence.
	let {
		open,
		title,
		link,
		finePrint,
		note = null,
		emailedTo = null,
		onClose,
		children
	}: {
		open: boolean;
		title: string;
		link: string;
		finePrint: string;
		note?: string | null;
		emailedTo?: string | null;
		onClose: () => void;
		children?: Snippet;
	} = $props();

	let bitsOpen = $state(false);

	function setOpen(next: boolean) {
		bitsOpen = next;
		if (!next) onClose();
	}

	const getOpen = () => bitsOpen;

	// The parent drives `open` one way. Mirror it into Bits UI's state in both
	// directions: a parent close (e.g. after the link was copied and the row
	// dismissed) must not re-fire onClose, which is reserved for user-initiated
	// closes.
	$effect(() => {
		if (open && !bitsOpen) bitsOpen = true;
		else if (!open && bitsOpen) bitsOpen = false;
	});

	let copied = $state(false);
	let linkInput = $state<HTMLInputElement | null>(null);

	// A new link means a fresh "Copied" state.
	$effect(() => {
		void link;
		copied = false;
	});

	// The link may be site-relative when server.web_base_url is unset; resolve
	// it against this origin before display or copy. Client-only on purpose:
	// the dialog can only open in the browser.
	const shownLink = $derived(link.startsWith('/') ? `${window.location.origin}${link}` : link);

	async function copyLink() {
		if (!linkInput) return;
		try {
			await navigator.clipboard.writeText(linkInput.value);
		} catch {
			// The clipboard API needs a secure context (plain-HTTP
			// self-hosting doesn't have one): select-all + the legacy command.
			linkInput.select();
			document.execCommand('copy');
		}
		copied = true;
	}
</script>

<Dialog.Root bind:open={getOpen, setOpen}>
	<Dialog.Portal>
		<Dialog.Overlay />
		<Dialog.Content>
			{#snippet child({ props })}
				<div {...props} class="link minerva-dialog-card">
					<Dialog.Title>
						{#snippet child({ props: headingProps })}
							<h2 {...headingProps}>{title}</h2>
						{/snippet}
					</Dialog.Title>
					<p class="intro">{@render children?.()}</p>
					{#if note}
						<p class="note">{note}</p>
					{/if}
					<div class="link-row">
						<input
							class="link-input"
							type="text"
							readonly
							value={shownLink}
							bind:this={linkInput}
							aria-label={title}
							onfocus={(e) => e.currentTarget.select()}
							onclick={(e) => e.currentTarget.select()}
						/>
						<Button variant="secondary" onclick={copyLink}>{copied ? 'Copied' : 'Copy'}</Button>
					</div>
					<p class="fine">{finePrint}</p>
					{#if emailedTo}
						<Banner>We've also sent it by email to {emailedTo}.</Banner>
					{/if}
					<div class="actions">
						<Button variant="secondary" onclick={() => setOpen(false)}>Close</Button>
					</div>
				</div>
			{/snippet}
		</Dialog.Content>
	</Dialog.Portal>
</Dialog.Root>

<style>
	.link {
		width: min(560px, calc(100vw - 32px));
	}

	h2 {
		margin: 0;
		font-size: 19px;
	}

	.intro {
		margin: 10px 0 0;
		font-size: 15px;
		color: var(--minerva-fg-muted);
	}

	.note {
		margin: 10px 0 0;
		font-size: 14.5px;
	}

	.link-row {
		display: flex;
		gap: 8px;
		margin-top: var(--minerva-space-4);
	}

	.link-input {
		flex: 1;
		min-width: 0;
		min-height: var(--minerva-control-h);
		padding: 10px 12px;
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		background: var(--minerva-bg);
		color: var(--minerva-fg);
		font-family: inherit;
		font-size: 13.5px;
		word-break: break-all;
	}

	.fine {
		margin: 10px 0 0;
		font-size: 13.5px;
	}

	.actions {
		display: flex;
		justify-content: flex-end;
		margin-top: var(--minerva-space-5);
	}
</style>

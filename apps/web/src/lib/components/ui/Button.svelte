<script lang="ts">
	import { Button as BitsButton } from 'bits-ui';
	import type { Snippet } from 'svelte';

	export type ButtonVariant = 'primary' | 'secondary' | 'ghost';

	let {
		variant = 'secondary',
		href,
		disabled = false,
		type = 'button',
		onclick,
		children,
		...rest
	}: {
		variant?: ButtonVariant;
		href?: string;
		disabled?: boolean;
		type?: 'button' | 'submit' | 'reset';
		onclick?: (event: MouseEvent) => void;
		children?: Snippet;
	} & Record<string, unknown> = $props();
</script>

{#if href !== undefined}
	<!-- A link: `disabled` means aria-disabled + no navigation. The primitive
	     owns that now — it drops the href from the DOM and takes the link out
	     of the tab order (tabindex="-1"), so a disabled link is inert. -->
	<BitsButton.Root
		class="btn {variant}"
		{href}
		disabled={disabled}
		onclick={disabled ? undefined : onclick}
		{...rest}
	>
		{@render children?.()}
	</BitsButton.Root>
{:else}
	<BitsButton.Root class="btn {variant}" {type} disabled={disabled} onclick={onclick} {...rest}>
		{@render children?.()}
	</BitsButton.Root>
{/if}

<style>
	/* :global() because the element is rendered inside Bits UI's Button.Root —
	   scoped selectors can't reach it (no child snippet exists for this part).
	   The class names were already global in effect: auth.css targets .btn. */
	:global(.btn) {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--minerva-space-2);
		min-height: var(--minerva-control-h);
		padding: 8px 16px;
		border: var(--minerva-border-w) solid transparent;
		border-radius: var(--minerva-radius);
		font-family: inherit;
		font-size: 14.5px;
		font-weight: 500;
		color: var(--minerva-fg);
		white-space: nowrap;
		cursor: pointer;
		transition:
			background var(--minerva-motion-mid) var(--minerva-motion-ease-in-out),
			border-color var(--minerva-motion-mid) var(--minerva-motion-ease-in-out);
	}

	:global(.btn:active) {
		transform: translateY(1px);
	}

	/* DESIGN.md posture: primary labels are 18px+ — the accent only carries
	   white (light) or cream (dark) labels at that size or larger. */
	:global(.primary) {
		background: var(--minerva-accent);
		color: var(--minerva-surface);
		font-size: 18px;
		padding: 9px 20px;
	}

	:global(.primary:hover) {
		background: color-mix(in oklch, var(--minerva-accent) 88%, black);
	}

	:global(html.dark .primary) {
		color: var(--minerva-fg);
	}

	:global(.secondary) {
		border-color: var(--minerva-border);
	}

	:global(.secondary:hover) {
		border-color: var(--minerva-fg);
	}

	:global(.ghost) {
		padding: 8px 10px;
	}

	:global(.ghost:hover) {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
	}

	:global(.btn[disabled]),
	:global(.btn[aria-disabled='true']) {
		opacity: 0.55;
		pointer-events: none;
	}
</style>

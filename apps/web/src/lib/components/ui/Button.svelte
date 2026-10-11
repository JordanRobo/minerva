<script lang="ts">
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
	<!-- A link: `disabled` means aria-disabled + navigation blocked, because
	     a real <a> has no disabled state. -->
	<a
		class="btn {variant}"
		href={href}
		aria-disabled={disabled || undefined}
		onclick={(event) => {
			if (disabled) event.preventDefault();
			else onclick?.(event);
		}}
		{...rest}
	>
		{@render children?.()}
	</a>
{:else}
	<button class="btn {variant}" type={type} disabled={disabled} onclick={onclick} {...rest}>
		{@render children?.()}
	</button>
{/if}

<style>
	.btn {
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

	.btn:active {
		transform: translateY(1px);
	}

	/* DESIGN.md posture: primary labels are 18px+ — the accent only carries
	   white (light) or cream (dark) labels at that size or larger. */
	.primary {
		background: var(--minerva-accent);
		color: var(--minerva-surface);
		font-size: 18px;
		padding: 9px 20px;
	}

	.primary:hover {
		background: color-mix(in oklch, var(--minerva-accent) 88%, black);
	}

	:global(html.dark) .primary {
		color: var(--minerva-fg);
	}

	.secondary {
		border-color: var(--minerva-border);
	}

	.secondary:hover {
		border-color: var(--minerva-fg);
	}

	.ghost {
		padding: 8px 10px;
	}

	.ghost:hover {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
	}

	.btn[disabled],
	.btn[aria-disabled='true'] {
		opacity: 0.55;
		pointer-events: none;
	}
</style>

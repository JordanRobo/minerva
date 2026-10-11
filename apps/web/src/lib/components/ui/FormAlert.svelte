<script lang="ts">
	import type { Snippet } from 'svelte';
	import Icon from '../icons/Icon.svelte';

	let {
		children,
		...rest
	}: { children?: Snippet } & Record<string, unknown> = $props();
</script>

<!-- Form-level error/success message. tabindex=-1 lets a failed submit move
     focus here (the element is not otherwise keyboard-reachable). No error
     colour token exists, so the signal is the icon + the accent left border —
     never colour alone. -->
<div class="alert" role="alert" tabindex="-1" {...rest}>
	<Icon name="alert" size={16} />
	<p>{@render children?.()}</p>
</div>

<style>
	.alert {
		display: flex;
		align-items: flex-start;
		gap: var(--minerva-space-2);
		padding: 10px 12px;
		border-left: 3px solid var(--minerva-accent);
		font-size: 14px;
		color: var(--minerva-fg);
	}

	.alert p {
		margin: 0;
	}
</style>

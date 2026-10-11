<script lang="ts">
	import StateCard from './StateCard.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import type { Snippet } from 'svelte';

	let {
		level = 2,
		title = 'Something went wrong',
		message = "We couldn't load this page. Check your connection and try again.",
		onretry,
		actions
	}: {
		level?: 1 | 2 | 3;
		title?: string;
		message?: string;
		onretry?: () => void;
		actions?: Snippet;
	} = $props();
</script>

{#snippet cardActions()}
	{#if onretry}
		<Button variant="primary" onclick={onretry}>Try again</Button>
	{/if}
	{@render actions?.()}
{/snippet}

<StateCard icon="alert" {level} {title} {message} actions={onretry || actions ? cardActions : undefined} />

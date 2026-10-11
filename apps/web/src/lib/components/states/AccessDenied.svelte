<script lang="ts">
	import StateCard from './StateCard.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import type { Snippet } from 'svelte';

	// `message` is the page-specific reason (the server's 403 text); `hint`
	// is the standing pointer to the right person.
	let {
		level = 2,
		message,
		hint = "Ask your school's administrator if you need access.",
		actions
	}: {
		level?: 1 | 2 | 3;
		message: string;
		hint?: string;
		actions?: Snippet;
	} = $props();
</script>

{#snippet cardActions()}
	<Button variant="ghost" href="/">Back home</Button>
	{@render actions?.()}
{/snippet}

<StateCard
	icon="lock"
	level={level}
	title="You don't have access to this"
	{message}
	{hint}
	actions={cardActions}
/>

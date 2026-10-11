<script lang="ts">
	import StateCard from './StateCard.svelte';
	import type { IconName } from '$lib/components/icons/Icon.svelte';
	import type { Snippet } from 'svelte';

	// `actions` is the primary action (e.g. "New goal"). Pages pass it only
	// when can(role, 'edit') allows — Read-only users get instructional
	// copy in `message` instead (e.g. "Ask a staff member to add one").
	let {
		icon = 'inbox',
		level = 2,
		title,
		message,
		actions
	}: {
		icon?: IconName;
		level?: 1 | 2 | 3;
		title: string;
		message: string;
		actions?: Snippet;
	} = $props();
</script>

<StateCard {icon} {level} {title} {message} {actions} />

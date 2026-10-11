<script lang="ts">
	import Button from '$lib/components/ui/Button.svelte';
	import Banner from '$lib/components/ui/Banner.svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import PageSkeleton from '$lib/components/ui/PageSkeleton.svelte';
	import EmptyState from '$lib/components/states/EmptyState.svelte';
	import ErrorState from '$lib/components/states/ErrorState.svelte';
	import AccessDenied from '$lib/components/states/AccessDenied.svelte';
	import { can } from '$lib/permissions';
	import type { PageProps } from './$types';

	// The first PageHeader is the page's own h1 and document title; the
	// later demos reuse the title so the tab doesn't change mid-page.
	let { data } = $props();
	const canEdit = $derived(can(data.user.role, 'edit'));
</script>

<PageHeader
	title="Components"
	description="Every shared component on one page — check light and dark, and resize to 380, 768 and 1280."
>
	{#snippet secondary()}
		<Button variant="secondary">Secondary action</Button>
	{/snippet}
	{#snippet actions()}
		{#if canEdit}
			<Button variant="primary">Primary action</Button>
		{/if}
	{/snippet}
</PageHeader>

<section class="section">
	<h2>Page header — Read-only variant (no primary action is passed)</h2>
	<PageHeader
		title="Read-only page"
		description="Only the secondary action is passed."
		documentTitle="Components · Minerva"
	>
		{#snippet secondary()}
			<Button variant="secondary">Secondary action</Button>
		{/snippet}
	</PageHeader>
</section>

<section class="section">
	<h2>Page header — title only</h2>
	<PageHeader title="Title only" documentTitle="Components · Minerva" />
</section>

<section class="section">
	<h2>Button</h2>
	<div class="row">
		<Button variant="primary">Primary</Button>
		<Button variant="secondary">Secondary</Button>
		<Button variant="ghost">Ghost</Button>
		<Button variant="primary" disabled>Disabled</Button>
		<Button variant="secondary" href="/goals">As a link</Button>
		<Button variant="ghost" href="/goals" disabled>Disabled link</Button>
	</div>
</section>

<section class="section">
	<h2>Banner</h2>
	<Banner>Something worth knowing, in one plain sentence.</Banner>
	<Banner dismissible>This notice can be dismissed — with the button or the keyboard.</Banner>
</section>

<section class="section">
	<h2>EmptyState — with the primary action (shown to staff and admin only)</h2>
	{#if canEdit}
		<EmptyState icon="goal" title="Goals will appear here" message="This page is on its way.">
			{#snippet actions()}
				<Button variant="primary">New goal</Button>
			{/snippet}
		</EmptyState>
	{:else}
		<EmptyState icon="goal" title="Goals will appear here" message="Ask a staff member to add one." />
	{/if}
</section>

<section class="section">
	<h2>EmptyState — without actions</h2>
	<EmptyState icon="task" title="Tasks will appear here" message="This page is on its way." />
</section>

<section class="section">
	<h2>ErrorState — with retry</h2>
	<ErrorState onretry={() => {}} />
</section>

<section class="section">
	<h2>AccessDenied</h2>
	<AccessDenied message="This area is only for administrators." />
</section>

<section class="section">
	<h2>PageSkeleton</h2>
	<PageSkeleton />
</section>

<section class="section">
	<h2>h-scroll — wide content scrolls in its own box</h2>
	<div class="h-scroll">
		<div class="wide">
			<p>Boards and timelines are wider than the page. They scroll sideways here; the page itself never moves.</p>
		</div>
	</div>
</section>

<style>
	.section {
		margin-top: var(--minerva-space-7);
	}

	.section h2 {
		margin: 0 0 var(--minerva-space-3);
		font-size: 20px;
	}

	.row {
		display: flex;
		align-items: center;
		gap: 10px;
		flex-wrap: wrap;
	}

	.wide {
		width: 1600px;
		padding: 20px 24px;
	}
</style>

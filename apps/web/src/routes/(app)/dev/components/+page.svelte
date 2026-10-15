<script lang="ts">
	import { Popover, Tooltip } from 'bits-ui';
	import Button from '$lib/components/ui/Button.svelte';
	import Banner from '$lib/components/ui/Banner.svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import PageSkeleton from '$lib/components/ui/PageSkeleton.svelte';
	import ConfirmDialog from '$lib/components/ui/ConfirmDialog.svelte';
	import LinkDialog from '$lib/components/ui/LinkDialog.svelte';
	import EmptyState from '$lib/components/states/EmptyState.svelte';
	import ErrorState from '$lib/components/states/ErrorState.svelte';
	import AccessDenied from '$lib/components/states/AccessDenied.svelte';
	import { can } from '$lib/permissions';
	import type { PageProps } from './$types';

	// The first PageHeader is the page's own h1 and document title; the
	// later demos reuse the title so the tab doesn't change mid-page.
	let { data } = $props();
	const canEdit = $derived(can(data.user.role, 'edit'));

	let confirmOpen = $state(false);
	let linkOpen = $state(false);
	let busyConfirmOpen = $state(false);
	let busyInFlight = $state(false);
	let errorConfirmOpen = $state(false);
	let errorConfirmError = $state<string | null>(null);
	let errorConfirmTries = $state(0);
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
	<h2>ConfirmDialog</h2>
	<Button variant="secondary" onclick={() => (confirmOpen = true)}>Open a confirm dialog</Button>
	<ConfirmDialog
		open={confirmOpen}
		title="Delete this item?"
		message="This can't be undone. The person or thing is gone for good."
		confirmLabel="Delete"
		onConfirm={() => (confirmOpen = false)}
		onClose={() => (confirmOpen = false)}
	/>
</section>

<section class="section">
	<h2>ConfirmDialog — refuses to close while busy</h2>
	<Button variant="secondary" onclick={() => (busyConfirmOpen = true)}>
		Open a confirm that saves for two seconds
	</Button>
	<ConfirmDialog
		open={busyConfirmOpen}
		title="Save this change?"
		message="The save takes two seconds. While it is in flight, Escape, the backdrop and Cancel must not close the dialog."
		confirmLabel="Save"
		busy={busyInFlight}
		onConfirm={() => {
			busyInFlight = true;
			setTimeout(() => {
				busyInFlight = false;
				busyConfirmOpen = false;
			}, 2000);
		}}
		onClose={() => (busyConfirmOpen = false)}
	/>
</section>

<section class="section">
	<h2>ConfirmDialog — error state</h2>
	<Button
		variant="secondary"
		onclick={() => {
			errorConfirmError = null;
			errorConfirmTries = 0;
			errorConfirmOpen = true;
		}}
	>
		Open a confirm that fails once
	</Button>
	<ConfirmDialog
		open={errorConfirmOpen}
		title="Delete this item?"
		message="The first attempt fails and shows the error below; the second succeeds."
		confirmLabel="Delete"
		error={errorConfirmError}
		onConfirm={() => {
			if (errorConfirmTries === 0) {
				errorConfirmTries = 1;
				errorConfirmError = 'Something went wrong. Try again.';
			} else {
				errorConfirmOpen = false;
			}
		}}
		onClose={() => (errorConfirmOpen = false)}
	/>
</section>

<section class="section">
	<h2>LinkDialog</h2>
	<Button variant="secondary" onclick={() => (linkOpen = true)}>Open a link dialog</Button>
	<LinkDialog
		open={linkOpen}
		title="Invitation link"
		link="/accept-invite?token=00000000-0000-4000-8000-000000000000"
		finePrint="It stops working on 20 Oct 2026, 3:45 pm. It will not be shown again."
		note="Email isn't set up on this server, so copy this link and send it to someone@example.com yourself."
		emailedTo={null}
		onClose={() => (linkOpen = false)}
	>
		Share this link with <strong>someone@example.com</strong>. When they open it, they set a password and are
		signed in straight away.
	</LinkDialog>
</section>

<section class="section">
	<h2>Tooltip</h2>
	<p class="demo-note">Opens on hover (after a short delay) and straight away on keyboard focus; it closes when the pointer leaves or the trigger blurs.</p>
	<div class="row">
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<Button variant="secondary" {...props}>Hover or focus me</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content side="top" sideOffset={6}>
				<span class="minerva-tooltip" role="tooltip">A short explanation, on hover and on keyboard focus.</span>
			</Tooltip.Content>
		</Tooltip.Root>

		<!-- The People-screen pattern: a disabled button inside a focusable
		     wrapper — the button stays inert, the span takes hover/focus. -->
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<span class="disabled-tip" aria-labelledby="gallery-disabled-btn" {...props}>
						<Button id="gallery-disabled-btn" variant="secondary" disabled>Disabled action</Button>
					</span>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content side="top" sideOffset={6}>
				<span class="minerva-tooltip" role="tooltip">This one is disabled on purpose — the tooltip still works.</span>
			</Tooltip.Content>
		</Tooltip.Root>
	</div>
</section>

<section class="section">
	<h2>Popover</h2>
	<p class="demo-note">
		Disclosure semantics (aria-expanded on the trigger, not role="menu"): click or Enter/Space opens it; Escape and outside clicks close it and return focus to the trigger.
	</p>
	<Popover.Root>
		<Popover.Trigger>
			{#snippet child({ props })}
				<Button variant="secondary" {...props}>Open a popover</Button>
			{/snippet}
		</Popover.Trigger>
		<Popover.Content side="top" align="start" sideOffset={8} trapFocus={false}>
			{#snippet child({ props, wrapperProps })}
				<div {...wrapperProps}>
					<div class="gallery-popover" {...props}>
						<p class="gp-title">A small panel</p>
						<p class="gp-body">Placed by floating-ui and styled from tokens, like the sidebar user menu.</p>
					</div>
				</div>
			{/snippet}
		</Popover.Content>
	</Popover.Root>
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

	.demo-note {
		margin: 0 0 var(--minerva-space-3);
		font-size: 14px;
		color: var(--minerva-fg-muted);
		max-width: 60ch;
	}

	/* Placed by Bits UI floating-ui (side="top" align="start"); the wrapper
	   inherits this z-index from the content's computed style. */
	.gallery-popover {
		width: 240px;
		z-index: 40;
		background: var(--minerva-surface);
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		box-shadow: 0 12px 32px color-mix(in oklch, var(--minerva-fg) 14%, transparent);
		padding: var(--minerva-space-3);
	}

	.gp-title {
		margin: 0 0 4px;
		font-size: 14.5px;
		font-weight: 600;
		color: var(--minerva-fg);
	}

	.gp-body {
		margin: 0;
		font-size: 13.5px;
		color: var(--minerva-fg-muted);
	}

	.wide {
		width: 1600px;
		padding: 20px 24px;
	}
</style>

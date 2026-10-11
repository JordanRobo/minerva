<script lang="ts">
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/states/EmptyState.svelte';
</script>

<PageHeader title="Dashboard" description="How we're tracking against our goals." />
<EmptyState icon="dashboard" title="Nothing to show yet" message="This page is on its way." />

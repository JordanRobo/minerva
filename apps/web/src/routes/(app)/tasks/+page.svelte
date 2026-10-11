<script lang="ts">
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/states/EmptyState.svelte';
</script>

<PageHeader title="Tasks" description="The day-to-day work, on one board." />
<EmptyState icon="task" title="Tasks will appear here" message="This page is on its way." />

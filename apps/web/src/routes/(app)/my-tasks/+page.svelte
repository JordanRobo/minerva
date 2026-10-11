<script lang="ts">
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/states/EmptyState.svelte';
</script>

<PageHeader title="My tasks" description="What's assigned to you." />
<EmptyState icon="my" title="Your tasks will appear here" message="This page is on its way." />

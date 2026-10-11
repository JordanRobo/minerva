<script lang="ts">
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/states/EmptyState.svelte';
</script>

<PageHeader title="Users" description="Who can sign in, and what they can do." />
<EmptyState icon="users" title="Users will appear here" message="This page is on its way." />

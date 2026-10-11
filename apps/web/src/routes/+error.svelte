<script lang="ts">
	import BareLayout from '$lib/components/shell/BareLayout.svelte';
	import RouteError from '$lib/components/states/RouteError.svelte';
</script>

<!-- Root error page: an error thrown by the (app) layout itself (e.g. the
     API was unreachable and the auth guard 502'd) never reaches the
     in-shell boundary, so the card renders in the bare pre-sign-in frame
     and retries with a full reload. -->
<BareLayout>
	<RouteError reload />
</BareLayout>

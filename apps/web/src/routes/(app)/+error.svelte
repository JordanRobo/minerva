<script lang="ts">
	import RouteError from '$lib/components/states/RouteError.svelte';
</script>

<!-- Renders inside the shell: the (app) layout stays up around errors
     thrown below it (the admin 403 guard, the unmatched-route 404, page
     loads). The (app) layout's own failures fall to the root boundary. -->
<RouteError />

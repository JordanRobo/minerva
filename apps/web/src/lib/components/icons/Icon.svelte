<script lang="ts">
	// Monoline icons from docs/design/app-shell.html (viewBox 24, stroke
	// currentColor, width 1.7). No icon library — a single component keyed
	// by name covers the shell and the UI/states components. The markup
	// goes through {@html} because the TS types for <svg> don't accept
	// injected content.
	export type IconName =
		| 'dashboard'
		| 'goal'
		| 'milestone'
		| 'task'
		| 'my'
		| 'users'
		| 'invite'
		| 'sso'
		| 'close'
		| 'signout'
		| 'chevron'
		| 'info'
		| 'alert'
		| 'shield'
		| 'lock'
		| 'inbox'
		| 'eye'
		| 'eye-off';

	const bodies: Record<IconName, string> = {
		dashboard:
			'<rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="7" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/><rect x="13" y="13" width="7" height="7" rx="1.5"/>',
		goal: '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="3.5"/>',
		milestone: '<path d="M5.5 21V4"/><path d="M5.5 4.5h11l-2.6 4 2.6 4h-11"/>',
		task: '<rect x="4" y="4" width="16" height="16" rx="3"/><path d="M8.5 12.5l2.5 2.5 5-5.5"/>',
		my: '<circle cx="9" cy="8.5" r="3.5"/><path d="M3.5 19.5c.8-3 3-4.5 5.5-4.5s4.7 1.5 5.5 4.5"/><path d="M15 11.5l2 2 3.5-4"/>',
		users:
			'<circle cx="9" cy="9" r="3.5"/><path d="M3.5 19.5c.7-3 3-4.5 5.5-4.5s4.8 1.5 5.5 4.5"/><path d="M15.5 6a3.5 3.5 0 010 6.7M17.2 15.2c1.7.7 2.9 2 3.3 4.3"/>',
		invite: '<rect x="3.5" y="5.5" width="17" height="13" rx="2.5"/><path d="M4.5 7.5l7.5 5.5 7.5-5.5"/>',
		sso: '<circle cx="8" cy="12" r="3.5"/><path d="M11.5 12H20M16.5 12v3M19.5 12v2.5"/>',
		close: '<path d="M6 6l12 12M18 6L6 18"/>',
		signout: '<path d="M13.5 4H18a2 2 0 012 2v12a2 2 0 01-2 2h-4.5"/><path d="M10 8l-4 4 4 4M6 12h9"/>',
		chevron: '<path d="M14.5 6l-6 6 6 6"/>',
		info: '<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5M12 8h.01"/>',
		alert: '<path d="M12 4.5l9.5 15h-19z"/><path d="M12 10.5v4M12 17.5h.01"/>',
		shield: '<path d="M12 3.5l7 2.7v5.3c0 4.4-2.9 7.4-7 9-4.1-1.6-7-4.6-7-9V6.2z"/>',
		lock: '<rect x="5.5" y="10.5" width="13" height="9.5" rx="2.5"/><path d="M8.5 10.5V8a3.5 3.5 0 017 0v2.5"/>',
		inbox:
			'<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 002 2h16a2 2 0 002-2v-6l-3.45-6.89A2 2 0 0016.76 4H7.24a2 2 0 00-1.79 1.11z"/>',
		eye:
			'<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/>',
		'eye-off':
			'<path d="M10.2 5.7A9.8 9.8 0 0112 5.5c6 0 9.5 6.5 9.5 6.5a17.5 17.5 0 01-2.5 3.4"/><path d="M6.4 6.6A17.2 17.2 0 002.5 12S6 18.5 12 18.5a9.6 9.6 0 004-.85"/><path d="M9.9 9.9a3 3 0 004.2 4.2"/><path d="M4 4l16 16"/>'
	};

	let { name, size = 18 }: { name: IconName; size?: number } = $props();

	const markup = $derived(
		`<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" ` +
			`stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">${bodies[name]}</svg>`
	);
</script>

<span class="ic" aria-hidden="true">{@html markup}</span>

<style>
	.ic {
		display: block;
		flex: 0 0 auto;
	}

	.ic :global(svg) {
		display: block;
	}
</style>

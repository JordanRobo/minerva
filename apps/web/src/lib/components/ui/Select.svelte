<script lang="ts" generics="T extends string">
	import { Select } from 'bits-ui';
	import Icon from '../icons/Icon.svelte';

	// Shared picker on Bits UI Select (D18): one trigger button plus a
	// portaled listbox for every select in the app, so they look and behave
	// identically. `value` binds to the page's state; `options` feeds the
	// listbox and the primitive's typeahead. `id`/`aria-label` pass through
	// to the trigger button (a <label for> may point at it); `open` is
	// bindable so the dev gallery can show the listbox in its open state.
	let {
		value = $bindable(),
		options,
		disabled = false,
		open = $bindable(false),
		...rest
	}: {
		value?: T;
		options: { value: string; label: string }[];
		disabled?: boolean;
		open?: boolean;
	} & Record<string, unknown> = $props();
</script>

<!-- Bits UI Select (D18): the primitive owns open state, keyboard handling
     (arrows / typeahead / Enter+Space / Escape), ARIA and focus — the trigger
     keeps focus the whole time (descendant pattern). Child snippets keep every
     element in this component's scope so the styles below apply; the content
     is portaled to <body>, which also clears table-card's overflow clipping. -->
<Select.Root type="single" {disabled} bind:open items={options} bind:value={value as never}>
	<Select.Trigger {...rest}>
		{#snippet child({ props })}
			<button {...props} class="minerva-select-trigger">
				<!-- Default rendering (no child snippet): the part resolves the
				     selected label itself; the class flows through to its span. -->
				<Select.Value class="minerva-select-value" />
				<span class="chev"><Icon name="chevron" size={16} /></span>
			</button>
		{/snippet}
	</Select.Trigger>
	<Select.Portal>
		<Select.Content sideOffset={4}>
			{#snippet child({ wrapperProps, props })}
				<div {...wrapperProps}>
					<div {...props} class="minerva-select-content">
						{#each options as o (o.value)}
							<Select.Item value={o.value} label={o.label}>
								{#snippet child({ props, selected })}
									<div {...props} class="minerva-select-item">
										<span class="item-label">{o.label}</span>
										{#if selected}
											<span class="check"><Icon name="check" size={16} /></span>
										{/if}
									</div>
								{/snippet}
							</Select.Item>
						{/each}
					</div>
				</div>
			{/snippet}
		</Select.Content>
	</Select.Portal>
</Select.Root>

<style>
	/* Trigger: mirrors the old per-page .role-select (tokens only). base.css's
	   `button { all: unset }` reset means display and font are set here. */
	.minerva-select-trigger {
		display: inline-flex;
		align-items: center;
		gap: var(--minerva-space-2);
		min-height: var(--minerva-control-h);
		padding: 6px 10px 6px 12px;
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		background: var(--minerva-surface);
		color: var(--minerva-fg);
		font-family: inherit;
		font-size: 14.5px;
		text-align: left;
		cursor: pointer;
	}

	/* Same focus treatment as Input.svelte: tint on any focus, the base.css
	   :focus-visible outline adds the ring for keyboard users. */
	.minerva-select-trigger:focus {
		border-color: var(--minerva-accent);
	}

	.minerva-select-trigger[data-disabled] {
		opacity: 0.55;
		cursor: not-allowed;
	}

	/* :global() because the span is rendered inside Bits UI's Select.Value
	   (Button.svelte's pattern) — scoped selectors can't reach it. */
	:global(.minerva-select-value) {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.chev {
		display: flex;
		color: var(--minerva-fg-muted);
		transform: rotate(90deg); /* the shared chevron points left */
	}

	/* Listbox: portaled to <body> and fixed-positioned by floating-ui, so it
	   clears the table-card's overflow and never shifts page layout. Same
	   chrome as the popover work (dialogs.css / gallery-popover). */
	.minerva-select-content {
		z-index: 50;
		width: max-content;
		min-width: var(--bits-select-anchor-width);
		max-width: min(360px, calc(100vw - 32px));
		max-height: min(320px, calc(100vh - 32px));
		overflow-y: auto;
		padding: var(--minerva-space-1) 0;
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		background: var(--minerva-surface);
		color: var(--minerva-fg);
		box-shadow: 0 12px 32px color-mix(in oklch, var(--minerva-fg) 14%, transparent);
	}

	.minerva-select-item {
		display: flex;
		align-items: center;
		gap: var(--minerva-space-2);
		min-height: var(--minerva-control-h); /* ≥44px touch target */
		padding: 6px 14px;
		font-size: 14.5px;
		cursor: pointer;
	}

	.minerva-select-item[data-highlighted] {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
	}

	.item-label {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.check {
		display: flex;
		color: var(--minerva-accent);
	}
</style>

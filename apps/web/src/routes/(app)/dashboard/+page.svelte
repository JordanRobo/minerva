<script lang="ts">
	import { onMount } from 'svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/states/EmptyState.svelte';
	import ErrorState from '$lib/components/states/ErrorState.svelte';
	import PageSkeleton from '$lib/components/ui/PageSkeleton.svelte';
	import { apiRequest } from '$lib/api';

	// Wire shapes of GET /api/dashboard/summary (roadmap 3.15, D22).
	type StatusKey = 'on_track' | 'at_risk' | 'off_track' | 'complete';

	interface StatusCount {
		count: number;
		manual: number;
	}
	interface KindSummary {
		total: number;
		manual: number;
		by_status: Record<StatusKey, StatusCount>;
	}
	interface ManuallySetItem {
		kind: 'goal' | 'milestone';
		id: string;
		title: string;
		status: StatusKey;
		automatic_status: StatusKey;
		target_date: string | null;
	}
	interface DashboardSummary {
		as_of: string;
		goals: KindSummary;
		milestones: KindSummary;
		manually_set: ManuallySetItem[];
	}

	const STATUS_ORDER: StatusKey[] = ['on_track', 'at_risk', 'off_track', 'complete'];
	const STATUS_LABEL: Record<StatusKey, string> = {
		on_track: 'On Track',
		at_risk: 'At Risk',
		off_track: 'Off Track',
		complete: 'Complete'
	};

	let summary = $state<DashboardSummary | null>(null);
	let failed = $state(false);

	async function load() {
		failed = false;
		const res = await apiRequest<DashboardSummary>('GET', '/api/dashboard/summary');
		if (res.ok) summary = res.data;
		else failed = true;
	}

	onMount(load);

	const kindRows = $derived(
		summary
			? [
					{ label: 'Goals', data: summary.goals },
					{ label: 'Milestones', data: summary.milestones }
				]
			: []
	);

	const nothingToTrack = $derived(
		summary !== null && summary.goals.total === 0 && summary.milestones.total === 0
	);

	const manualTotal = $derived(
		summary ? summary.goals.manual + summary.milestones.manual : 0
	);

	function fmtDate(iso: string | null): string {
		if (iso === null) return 'No date';
		return new Date(`${iso}T00:00:00`).toLocaleDateString('en-GB', {
			day: '2-digit',
			month: 'short'
		});
	}

	function fmtAsOf(iso: string): string {
		const d = new Date(iso);
		const date = d.toLocaleDateString('en-GB', {
			day: '2-digit',
			month: 'short',
			year: 'numeric',
			timeZone: 'UTC'
		});
		const time = d.toLocaleTimeString('en-GB', {
			hour: '2-digit',
			minute: '2-digit',
			timeZone: 'UTC'
		});
		return `${date} · ${time} UTC`;
	}
</script>

<PageHeader
	title="Dashboard"
	description="The term's work at a glance — and which statuses someone set by hand."
/>

{#if failed}
	<ErrorState onretry={load} />
{:else if !summary}
	<PageSkeleton />
{:else if nothingToTrack}
	<EmptyState
		icon="dashboard"
		title="Nothing to track yet"
		message="Add a goal and break it into milestones, and the board fills in as work moves."
	/>
{:else}
	<section class="board" aria-label="Status board">
		<div class="rollup">
			<div class="rhead">
				<span class="rlabel rhead-corner" aria-hidden="true"></span>
				{#each STATUS_ORDER as key (key)}
					<span class="stat-k"><i class="sq sq-{key}" aria-hidden="true"></i>{STATUS_LABEL[key]}</span>
				{/each}
			</div>
			{#each kindRows as row (row.label)}
				<div class="rrow">
					<span class="rlabel">{row.label}</span>
					{#each STATUS_ORDER as key (key)}
						{@const c = row.data.by_status[key]}
						<span class="cell">
							<span class="fig big">{c.count}</span>
							{#if c.manual > 0}
								<span class="sub fig">{c.manual} by hand</span>
							{/if}
						</span>
					{/each}
				</div>
			{/each}
		</div>

		{#if summary.manually_set.length > 0}
			<div class="mcols">
				<span>Target date</span>
				<span>Set by hand</span>
				<span>Status</span>
			</div>
			<ul class="mrows">
				{#each summary.manually_set as item (item.id)}
					<li class="mrow">
						<span class="fig date">{fmtDate(item.target_date)}</span>
						<span class="title">
							<i class="sq sq-alert" aria-hidden="true"></i>
							<span class="kind">{item.kind === 'goal' ? 'Goal' : 'Milestone'}</span>
							{item.title}
						</span>
						<span class="st">
							<i class="sq sq-{item.status}" aria-hidden="true"></i>
							{STATUS_LABEL[item.status]}
							<span class="auto fig">computed {STATUS_LABEL[item.automatic_status]}</span>
						</span>
					</li>
				{/each}
			</ul>
		{:else}
			<p class="mempty">Nothing set by hand — every status is computed.</p>
		{/if}

		<footer class="foot">
			<span class="fig">{fmtAsOf(summary.as_of)}</span>
			<span>{manualTotal} manually set</span>
		</footer>
	</section>
{/if}

<style>
	/* The board: one surface panel, fixed columns, tabular figures. The thick
	   ink rule under the status heads is the board's spine. */
	.board {
		background: var(--minerva-surface);
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		box-shadow: 0 1px 3px color-mix(in oklch, var(--minerva-fg) 6%, transparent);
		overflow: hidden;
	}

	.rollup {
		padding-top: 8px;
	}

	.rhead,
	.rrow {
		display: grid;
		grid-template-columns: 132px repeat(4, minmax(0, 1fr));
		align-items: center;
	}

	.rhead {
		padding: 16px 0 12px;
		border-bottom: 2px solid var(--minerva-fg);
	}

	.stat-k {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 0 16px;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--minerva-fg-muted);
	}

	.sq {
		width: 10px;
		height: 10px;
		flex: none;
		display: inline-block;
	}

	.sq-on_track {
		background: var(--minerva-status-on-track);
	}

	.sq-at_risk {
		background: var(--minerva-status-at-risk);
	}

	.sq-off_track {
		background: var(--minerva-status-off-track);
	}

	.sq-complete {
		background: var(--minerva-status-complete);
	}

	.sq-alert {
		background: var(--minerva-alert);
	}

	.rrow {
		padding: 14px 0;
		border-bottom: 1px solid var(--minerva-border);
	}

	.rlabel {
		padding-left: 24px;
		font-size: 12px;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--minerva-fg-muted);
	}

	.cell {
		display: flex;
		flex-direction: column;
		gap: 3px;
		padding: 0 16px;
		min-width: 0;
	}

	.big {
		font-size: 34px;
		font-weight: 600;
		line-height: 1;
	}

	.sub {
		font-size: 11.5px;
		color: var(--minerva-fg-muted);
	}

	.mcols {
		display: grid;
		grid-template-columns: 96px minmax(0, 1fr) 248px;
		padding: 10px 24px;
		border-bottom: 1px solid var(--minerva-border);
		font-size: 10.5px;
		font-weight: 600;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: var(--minerva-fg-muted);
	}

	.mrow {
		display: grid;
		grid-template-columns: 96px minmax(0, 1fr) 248px;
		align-items: center;
		padding: 12px 24px;
		border-bottom: 1px solid var(--minerva-border);
		/* The alert hold: a row a person set by hand keeps the amber tint. */
		background: color-mix(in oklch, var(--minerva-alert) 13%, transparent);
	}

	.mrow:last-child {
		border-bottom: none;
	}

	.date {
		font-size: 14px;
		font-weight: 500;
	}

	.title {
		display: flex;
		align-items: center;
		gap: 10px;
		min-width: 0;
		font-size: 14.5px;
		font-weight: 500;
	}

	.title .kind {
		flex: none;
		font-size: 10.5px;
		font-weight: 600;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--minerva-fg-muted);
	}

	.st {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 12px;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	.st .auto {
		margin-left: auto;
		font-family: var(--minerva-font-figures);
		font-size: 11px;
		font-weight: 400;
		letter-spacing: 0;
		text-transform: none;
		color: var(--minerva-fg-muted);
		white-space: nowrap;
	}

	.mempty {
		padding: 18px 24px;
		margin: 0;
		font-size: 13.5px;
		color: var(--minerva-fg-muted);
	}

	.foot {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px 16px;
		flex-wrap: wrap;
		padding: 12px 24px;
		border-top: 1px solid var(--minerva-border);
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--minerva-fg-muted);
	}

	/* Narrow screens: the kind label spans its row above four aligned
	   columns; manually-set rows stack date/status over the title. */
	@media (max-width: 899.98px) {
		.rhead,
		.rrow {
			grid-template-columns: repeat(4, minmax(0, 1fr));
		}

		.rhead-corner {
			display: none;
		}

		/* Stacked square-over-label gives the heads the full column width —
		   "COMPLETE" is one long word and clips otherwise at phone widths. */
		.stat-k {
			flex-direction: column;
			gap: 5px;
			padding: 0 8px;
			text-align: center;
			font-size: 10.5px;
			letter-spacing: 0.08em;
			overflow-wrap: anywhere;
		}

		.rrow .rlabel {
			grid-column: 1 / -1;
			padding-left: 16px;
			margin-bottom: 6px;
		}

		.big {
			font-size: 26px;
		}

		.mcols {
			display: none;
		}

		.mrow {
			grid-template-columns: minmax(0, 1fr) auto;
			grid-template-areas:
				'date st'
				'title title';
			row-gap: 6px;
			padding: 12px 16px;
		}

		.date {
			grid-area: date;
		}

		.title {
			grid-area: title;
		}

		.st {
			grid-area: st;
			flex-wrap: wrap;
			justify-content: flex-end;
		}

		.st .auto {
			margin-left: 0;
			width: 100%;
			text-align: right;
		}
	}
</style>

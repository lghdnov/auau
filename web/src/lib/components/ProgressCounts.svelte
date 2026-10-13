<script lang="ts">
	import type { FileCounts } from '$lib/types';
	import { progressPercent } from '$lib/progress';

	let { counts }: { counts: FileCounts } = $props();

	const total = $derived(counts.new + counts.in_progress + counts.done + counts.error);
	const percent = $derived(progressPercent(counts));
</script>

<div class="progress">
	<div class="stats">
		<span>Новые: {counts.new}</span>
		<span>В работе: {counts.in_progress}</span>
		<span>Готово: {counts.done}</span>
		<span>Ошибки: {counts.error}</span>
		<span class="total">Готово {counts.done} из {total} ({percent}%)</span>
	</div>
	<div
		class="bar"
		role="progressbar"
		aria-label="Прогресс разметки"
		aria-valuenow={percent}
		aria-valuemin={0}
		aria-valuemax={100}
	>
		<div class="fill" style:width="{percent}%"></div>
	</div>
</div>

<style>
	.stats {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem 1.25rem;
		margin-bottom: 0.5rem;
	}

	.total {
		margin-left: auto;
		color: #555;
	}

	.bar {
		height: 0.5rem;
		border-radius: 999px;
		background: #e5e5e5;
		overflow: hidden;
	}

	.fill {
		height: 100%;
		background: #4363d8;
		border-radius: inherit;
		transition: width 0.2s ease;
	}
</style>

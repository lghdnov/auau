<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import { api, ApiError } from '$lib/api';
	import type { LabelPatch } from '$lib/api';
	import type { FileStatus, Label } from '$lib/types';
	import FileTable from '$lib/components/FileTable.svelte';
	import LabelsPanel from '$lib/components/LabelsPanel.svelte';
	import ProgressCounts from '$lib/components/ProgressCounts.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let claiming = $state(false);
	let claimMessage = $state('');
	let actionError = $state('');

	const projectId = $derived(data.detail.project.id);
	const activeFilter = $derived(data.status ?? '');

	const FILTERS: { value: FileStatus | ''; label: string }[] = [
		{ value: '', label: 'Все' },
		{ value: 'new', label: 'Новые' },
		{ value: 'in_progress', label: 'В работе' },
		{ value: 'done', label: 'Готово' },
		{ value: 'error', label: 'Ошибки' }
	];

	function errorMessage(err: unknown): string {
		return err instanceof ApiError ? err.message : 'Не удалось связаться с сервером';
	}

	/** Взять файл из очереди: переход на разметку или «всё разобрано». */
	async function takeFile() {
		if (claiming) return;
		claiming = true;
		claimMessage = '';
		actionError = '';
		try {
			const file = await api.claimNext(projectId);
			if (file) {
				await goto(`/annotate/${file.id}`);
			} else {
				claimMessage = 'Все файлы разобраны';
			}
		} catch (err) {
			actionError = errorMessage(err);
		} finally {
			claiming = false;
		}
	}

	async function createLabel(name: string, hotkey: string): Promise<boolean> {
		actionError = '';
		try {
			await api.createLabel(projectId, hotkey ? { name, hotkey } : { name });
			await invalidateAll();
			return true;
		} catch (err) {
			actionError = errorMessage(err);
			return false;
		}
	}

	async function updateLabel(id: string, patch: LabelPatch): Promise<boolean> {
		actionError = '';
		try {
			await api.patchLabel(id, patch);
			await invalidateAll();
			return true;
		} catch (err) {
			actionError = errorMessage(err);
			return false;
		}
	}

	async function archiveLabel(label: Label): Promise<boolean> {
		actionError = '';
		try {
			await api.archiveLabel(label.id);
			await invalidateAll();
			return true;
		} catch (err) {
			actionError = errorMessage(err);
			return false;
		}
	}
</script>

<section>
	<nav class="crumbs"><a href="/">Проекты</a> / {data.detail.project.name}</nav>
	<h1>{data.detail.project.name}</h1>
	{#if data.detail.project.description}
		<p class="desc">{data.detail.project.description}</p>
	{/if}

	<ProgressCounts counts={data.detail.counts} />

	<div class="actions">
		<button onclick={takeFile} disabled={claiming}>Взять файл</button>
		<a class="button" href="/projects/{projectId}/upload">Загрузить файлы</a>
		<a class="button" href={api.exportUrl(projectId, 'json')} target="_blank" rel="noopener">
			Экспорт JSON
		</a>
		<a class="button" href={api.exportUrl(projectId, 'csv')} target="_blank" rel="noopener">
			Экспорт CSV
		</a>
	</div>
	{#if claimMessage}
		<p class="ok">{claimMessage}</p>
	{/if}
	{#if actionError}
		<p class="error">{actionError}</p>
	{/if}

	<h2>Файлы</h2>
	<nav class="filters">
		{#each FILTERS as f (f.value)}
			<a
				href={f.value ? `?status=${f.value}` : '?'}
				class:active={activeFilter === f.value}
			>
				{f.label}
			</a>
		{/each}
	</nav>
	<FileTable files={data.files} />

	<h2>Метки</h2>
	<LabelsPanel
		labels={data.detail.labels}
		onCreate={createLabel}
		onUpdate={updateLabel}
		onArchive={archiveLabel}
	/>
</section>

<style>
	.crumbs {
		font-size: 0.85rem;
		color: #555;
		margin-bottom: 0.25rem;
	}

	h2 {
		margin: 1.75rem 0 0.5rem;
		font-size: 1.15rem;
	}

	.desc {
		color: #555;
		margin-top: 0;
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		margin: 1rem 0 0.5rem;
	}

	button,
	.button {
		display: inline-block;
		padding: 0.45rem 0.9rem;
		border: 1px solid #4363d8;
		border-radius: 6px;
		background: #fff;
		color: #4363d8;
		font-size: 0.95rem;
		text-decoration: none;
		cursor: pointer;
	}

	button:hover:not(:disabled),
	.button:hover {
		background: #f5f7ff;
	}

	button:disabled {
		opacity: 0.6;
		cursor: default;
	}

	.filters {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
		margin-bottom: 0.75rem;
	}

	.filters a {
		padding: 0.25rem 0.7rem;
		border: 1px solid #ddd;
		border-radius: 999px;
		font-size: 0.85rem;
		color: #555;
		text-decoration: none;
	}

	.filters a.active {
		border-color: #4363d8;
		color: #4363d8;
		background: #f5f7ff;
	}

	.ok {
		color: #2e7d32;
	}

	.error {
		color: #c62828;
	}
</style>

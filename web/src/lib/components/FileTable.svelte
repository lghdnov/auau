<script lang="ts">
	import { goto } from '$app/navigation';
	import type { AudioFile, FileStatus } from '$lib/types';

	let { files }: { files: AudioFile[] } = $props();

	/** Русские подписи статусов (processing — файлы на подготовке медиа). */
	const STATUS_LABELS: Record<FileStatus, string> = {
		processing: 'Обработка',
		new: 'Новый',
		in_progress: 'В работе',
		done: 'Готово',
		error: 'Ошибка'
	};

	/** duration_ms → mm:ss; без длительности — прочерк. */
	function formatDuration(ms: number | null): string {
		if (ms == null) return '—';
		const totalSeconds = Math.floor(ms / 1000);
		const minutes = Math.floor(totalSeconds / 60);
		const seconds = totalSeconds % 60;
		return `${minutes}:${String(seconds).padStart(2, '0')}`;
	}

	/** Кликабельны только файлы, уже взятые в работу или законченные. */
	function isClickable(file: AudioFile): boolean {
		return file.status === 'in_progress' || file.status === 'done';
	}

	function open(file: AudioFile) {
		goto(`/annotate/${file.id}`);
	}
</script>

{#if files.length === 0}
	<p class="empty">Файлов нет.</p>
{:else}
	<table>
		<thead>
			<tr>
				<th>Имя</th>
				<th>Длительность</th>
				<th>Статус</th>
				<th>Ошибка</th>
			</tr>
		</thead>
		<tbody>
			{#each files as file (file.id)}
				{@const clickable = isClickable(file)}
				<tr
					class:clickable
					role={clickable ? 'button' : undefined}
					tabindex={clickable ? 0 : undefined}
					onclick={clickable ? () => open(file) : undefined}
					onkeydown={clickable
						? (e: KeyboardEvent) => {
								if (e.key === 'Enter' || e.key === ' ') open(file);
							}
						: undefined}
				>
					<td class="name">{file.original_name}</td>
					<td>{formatDuration(file.duration_ms)}</td>
					<td><span class="status {file.status}">{STATUS_LABELS[file.status]}</span></td>
					<td class="error-msg">{#if file.status === 'error'}{file.error_message}{/if}</td>
				</tr>
			{/each}
		</tbody>
	</table>
	{/if}

<style>
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.95rem;
	}

	th,
	td {
		text-align: left;
		padding: 0.4rem 0.6rem;
		border-bottom: 1px solid #e5e5e5;
		vertical-align: top;
	}

	th {
		color: #555;
		font-weight: 600;
	}

	.clickable {
		cursor: pointer;
	}

	.clickable:hover {
		background: #f5f7ff;
	}

	.name {
		max-width: 24rem;
		overflow-wrap: anywhere;
	}

	.status {
		white-space: nowrap;
	}

	.status.done {
		color: #2e7d32;
	}

	.status.error {
		color: #c62828;
	}

	.error-msg {
		max-width: 20rem;
		overflow-wrap: anywhere;
		color: #c62828;
	}

	.empty {
		color: #555;
	}
</style>

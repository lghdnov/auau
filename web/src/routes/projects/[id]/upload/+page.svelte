<script lang="ts">
	import { api } from '$lib/api';
	import { runUploadQueue } from '$lib/uploadQueue';
	import type { UploadOutcome } from '$lib/uploadQueue';
	import type { UploadResult } from '$lib/types';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	/** Строка списка: файл и вердикт (null — ещё не загружался). */
	interface UploadItem {
		file: File;
		result: UploadOutcome | null;
		errorMessage?: string;
	}

	/** Русские подписи и цвет вердикта по спеке: принято/дубликат/ошибка. */
	interface Verdict {
		cls: 'pending' | 'accepted' | 'duplicate' | 'error';
		label: string;
		message: string | null;
	}

	let items = $state<UploadItem[]>([]);
	let uploading = $state(false);
	let dragOver = $state(false);
	let fileInput = $state<HTMLInputElement | null>(null);

	/** Текущий батч — для прогресса «N из M» и метки «Загрузка…». */
	let batch = $state<UploadItem[]>([]);
	let doneCount = $state(0);

	const projectId = $derived(data.project.id);
	const pendingCount = $derived(items.filter((i) => i.result === null).length);
	const batchFinished = $derived(!uploading && items.length > 0 && pendingCount === 0);
	const current = $derived(uploading ? batch[doneCount] : undefined);

	function addFiles(list: FileList | null | undefined) {
		for (const file of list ?? []) {
			items.push({ file, result: null });
		}
	}

	function onDrop(event: DragEvent) {
		event.preventDefault();
		dragOver = false;
		addFiles(event.dataTransfer?.files);
	}

	function onPick(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		addFiles(input.files);
		input.value = '';
	}

	/** Прогнать все ещё не загруженные файлы; ошибки файлов очередь не роняют. */
	async function upload() {
		if (uploading || pendingCount === 0) return;
		const queue = [...items.filter((i) => i.result === null)];
		batch = queue;
		doneCount = 0;
		uploading = true;
		await runUploadQueue(projectId, queue.map((i) => i.file), api, (file, result, errorMessage) => {
			const item = queue.find((i) => i.file === file && i.result === null);
			if (item) {
				item.result = result;
				item.errorMessage = errorMessage;
			}
			doneCount++;
		});
		uploading = false;
	}

	function verdict(item: UploadItem): Verdict {
		const res = item.result;
		if (res === null) {
			return {
				cls: 'pending',
				label: item === current ? 'Загрузка…' : 'Ожидает',
				message: null
			};
		}
		if (res === 'error') {
			return { cls: 'error', label: 'Ошибка', message: item.errorMessage ?? null };
		}
		if (res.status === 'accepted') {
			return { cls: 'accepted', label: 'Принято', message: null };
		}
		if (res.status === 'duplicate') {
			return { cls: 'duplicate', label: 'Дубликат', message: res.message };
		}
		return { cls: 'error', label: 'Отклонено', message: res.message };
	}
</script>

<section>
	<nav class="crumbs">
		<a href="/">Проекты</a> / <a href="/projects/{projectId}">{data.project.name}</a> / Загрузка
	</nav>
	<h1>Загрузка файлов</h1>
	<p class="desc">Файлы загружаются по одному; ошибка одного файла не останавливает остальные.</p>

	<div
		class="dropzone"
		class:over={dragOver}
		role="button"
		tabindex={0}
		onclick={() => fileInput?.click()}
		onkeydown={(e) => {
			if (e.key === 'Enter' || e.key === ' ') {
				e.preventDefault();
				fileInput?.click();
			}
		}}
		ondragover={(e) => {
			e.preventDefault();
			dragOver = true;
		}}
		ondragleave={() => (dragOver = false)}
		ondrop={onDrop}
	>
		Перетащите аудиофайлы сюда или нажмите, чтобы выбрать
		<span class="hint">WAV, FLAC, MP3, OGG, M4A</span>
	</div>
	<input
		type="file"
		hidden
		multiple
		accept=".wav,.flac,.mp3,.ogg,.m4a"
		bind:this={fileInput}
		onchange={onPick}
	/>

	{#if items.length > 0}
		<h2>Файлы</h2>
		<ul class="files">
			{#each items as item, index (index)}
				{@const v = verdict(item)}
				<li>
					<span class="name">{item.file.name}</span>
					<span class="verdict {v.cls}">
						{v.label}{#if v.message}: {v.message}{/if}
					</span>
				</li>
			{/each}
		</ul>
	{/if}

	<div class="actions">
		<button onclick={upload} disabled={uploading || pendingCount === 0}>Загрузить</button>
		{#if uploading}
			<span class="progress">Загружено {doneCount} из {batch.length}</span>
		{:else if batchFinished}
			<a class="button" href="/projects/{projectId}">К дашборду</a>
		{/if}
	</div>
</section>

<style>
	.crumbs {
		font-size: 0.85rem;
		color: #555;
		margin-bottom: 0.25rem;
	}

	h2 {
		margin: 1.5rem 0 0.5rem;
		font-size: 1.15rem;
	}

	.desc {
		color: #555;
		margin-top: 0;
	}

	.dropzone {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.35rem;
		padding: 2rem 1rem;
		margin-top: 1rem;
		border: 2px dashed #bbb;
		border-radius: 10px;
		color: #555;
		cursor: pointer;
		text-align: center;
	}

	.dropzone:hover,
	.dropzone.over {
		border-color: #4363d8;
		color: #4363d8;
		background: #f5f7ff;
	}

	.hint {
		font-size: 0.85rem;
	}

	.files {
		list-style: none;
		padding: 0;
		margin: 0;
		font-size: 0.95rem;
	}

	.files li {
		display: flex;
		justify-content: space-between;
		gap: 1rem;
		padding: 0.4rem 0.2rem;
		border-bottom: 1px solid #e5e5e5;
	}

	.name {
		overflow-wrap: anywhere;
	}

	.verdict {
		white-space: nowrap;
		flex-shrink: 0;
		max-width: 60%;
		overflow-wrap: anywhere;
		text-align: right;
	}

	.verdict.pending {
		color: #777;
	}

	.verdict.accepted {
		color: #2e7d32;
	}

	.verdict.duplicate {
		color: #9a6700;
	}

	.verdict.error {
		color: #c62828;
	}

	.actions {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		margin-top: 1rem;
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

	.progress {
		color: #555;
	}
</style>

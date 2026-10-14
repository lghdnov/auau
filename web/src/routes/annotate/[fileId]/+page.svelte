<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { api, ApiError } from '$lib/api';
	import type { FileStatus } from '$lib/types';
	import { AnnotationState } from '$lib/annotator/store';
	import type { Segment } from '$lib/annotator/store';
	import { createAutosaver } from '$lib/annotator/autosave';
	import type { Autosaver } from '$lib/annotator/autosave';
	import { createWave } from '$lib/annotator/wave';
	import type { Wave } from '$lib/annotator/wave';
	import { hotkeyForEvent } from '$lib/annotator/hotkeys';
	import type { HotkeyAction } from '$lib/annotator/hotkeys';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const STATUS_LABELS: Record<FileStatus, string> = {
		processing: 'Обработка',
		new: 'Новый',
		in_progress: 'В работе',
		done: 'Готово',
		error: 'Ошибка'
	};

	/** Кнопки скорости из спеки: 0.5×–2×. */
	const RATES = [0.5, 0.75, 1, 1.5, 2];

	/**
	 * Сессия одного файла: модель + автосейв, замкнутые на его file.id.
	 * Поздний таймер автосейва предыдущего файла не может записать его
	 * сегменты в новый — id и состояние захвачены в замыкании.
	 */
	interface FileSession {
		fileId: string;
		st: AnnotationState;
		fileDirty: boolean;
		autosaver: Autosaver;
	}

	let model: AnnotationState = $state(new AnnotationState());
	let dirty = $state(false);
	let currentMs = $state(0);
	let playing = $state(false);
	let rate = $state(1);
	let selectedUid: string | null = $state(null);
	let waveError = $state('');
	let actionError = $state('');
	let completing = $state(false);
	let confirmEmpty = $state(false);
	let allDone = $state(false);
	let leaseLost = $state(false);

	let wave: Wave | null = null;
	let waveContainer: HTMLElement | null = null;
	let session: FileSession | null = null;

	const projectId = $derived(data.file.project_id);
	const durationMs = $derived(data.file.duration_ms ?? 0);
	/** Палитра без архивированных меток. */
	const labels = $derived(data.labels.filter((l) => !l.archived));
	const labelById = $derived(new Map(data.labels.map((l) => [l.id, l])));

	function colorOf(labelId: string): string {
		return labelById.get(labelId)?.color ?? '#999999';
	}

	function nameOf(labelId: string): string {
		return labelById.get(labelId)?.name ?? '?';
	}

	/** Отметить правку: сессия файла грязная, дебаунс-таймер перезапущен. */
	function markDirty(s: FileSession): void {
		s.fileDirty = true;
		if (session === s) dirty = true;
		s.autosaver.notifyChanged();
	}

	function isLeaseLost(err: unknown): boolean {
		return err instanceof ApiError && err.code === 'lease_required';
	}

	function genericError(err: unknown): string {
		return err instanceof ApiError ? err.message : 'Не удалось связаться с сервером';
	}

	/** Мс → м:сс.ммм. */
	function fmt(ms: number): string {
		const total = Math.floor(ms / 1000);
		return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, '0')}.${String(
			Math.floor(ms % 1000)
		).padStart(3, '0')}`;
	}

	function fmtRate(r: number): string {
		return `${r}×`;
	}

	function togglePlay(): void {
		if (!wave) return;
		if (wave.ws.isPlaying()) wave.pause();
		else wave.play();
	}

	function seekTo(ms: number): void {
		const target = Math.max(0, Math.min(ms, durationMs));
		wave?.seek(target);
		currentMs = target;
	}

	function seekBy(deltaMs: number): void {
		seekTo(currentMs + deltaMs);
	}

	function setRate(r: number): void {
		rate = r;
		wave?.setRate(r);
	}

	function selectLabel(id: string): void {
		model.selectLabel(id);
		wave?.setDrawColor(colorOf(id));
	}

	function rowClick(seg: Segment): void {
		selectedUid = seg.uid;
		seekTo(seg.start_ms);
	}

	function removeSegment(uid: string): void {
		model.removeSegment(uid);
		wave?.removeRegion(uid);
		if (selectedUid === uid) selectedUid = null;
		if (session) markDirty(session);
	}

	/** Правка начала: клампим в [0, end-1]; пустой ввод откатывается. */
	function editStart(seg: Segment, ev: Event): void {
		const input = ev.target as HTMLInputElement;
		if (input.value.trim() === '') {
			input.value = String(seg.start_ms);
			return;
		}
		const start = Math.max(0, Math.min(Math.round(Number(input.value)), seg.end_ms - 1));
		input.value = String(start);
		model.updateSegment(seg.uid, { start_ms: start });
		wave?.syncRegion(seg.uid, { startMs: start });
		if (session) markDirty(session);
	}

	function editEnd(seg: Segment, ev: Event): void {
		const input = ev.target as HTMLInputElement;
		if (input.value.trim() === '') {
			input.value = String(seg.end_ms);
			return;
		}
		const end = Math.max(seg.start_ms + 1, Math.min(Math.round(Number(input.value)), durationMs));
		input.value = String(end);
		model.updateSegment(seg.uid, { end_ms: end });
		wave?.syncRegion(seg.uid, { endMs: end });
		if (session) markDirty(session);
	}

	function onCompleteClick(): void {
		if (completing) return;
		if (model.segments.length === 0) {
			confirmEmpty = true;
			return;
		}
		void doComplete();
	}

	/** flush (с ошибками!) → complete → автозабор следующего файла. */
	async function doComplete(): Promise<void> {
		if (completing) return;
		completing = true;
		actionError = '';
		try {
			await session?.autosaver.flush();
			await api.completeFile(data.file.id);
			const next = await api.claimNext(projectId);
			if (next) {
				await goto(`/annotate/${next.id}`);
			} else {
				allDone = true;
			}
		} catch (err) {
			if (isLeaseLost(err)) leaseLost = true;
			else actionError = genericError(err);
		} finally {
			completing = false;
		}
	}

	$effect(() => {
		const file = data.file;
		const annotations = data.annotations;

		const st = new AnnotationState();
		for (const a of annotations) st.addSegment(a.label_id, a.start_ms, a.end_ms);

		const s: FileSession = {
			fileId: file.id,
			st,
			fileDirty: false,
			autosaver: createAutosaver(
				() => s.fileDirty,
				async () => {
					try {
						await api.putAnnotations(file.id, st.toPayload());
						s.fileDirty = false;
						if (session === s) dirty = false;
					} catch (err) {
						if (isLeaseLost(err)) leaseLost = true;
						throw err;
					}
				}
			)
		};

		session = s;
		model = st;
		dirty = false;
		selectedUid = null;
		currentMs = 0;
		playing = false;
		rate = 1;
		waveError = '';
		actionError = '';
		confirmEmpty = false;
		allDone = false;
		leaseLost = false;

		let cancelled = false;
		let w: Wave | null = null;

		if (waveContainer && file.duration_ms) {
			createWave(waveContainer, `/media/${file.id}`, `/media/${file.id}/peaks`, file.duration_ms)
				.then((created) => {
					if (cancelled) {
						created.onDestroy();
						return;
					}
					w = created;
					wave = created;

					created.onRegionCreated((startMs, endMs) => {
						if (!model.selectedLabelId) return null;
						const seg = model.addSegment(model.selectedLabelId, startMs, endMs);
						selectedUid = seg.uid;
						markDirty(s);
						return seg.uid;
					});
					created.onRegionDragged((uid, startMs, endMs) => {
						model.updateSegment(uid, { start_ms: startMs, end_ms: endMs });
						markDirty(s);
					});
					created.onRegionClicked((uid) => {
						selectedUid = uid;
					});
					created.onPlayhead((ms) => {
						currentMs = ms;
					});
					created.ws.on('play', () => (playing = true));
					created.ws.on('pause', () => (playing = false));

					for (const seg of model.segments) {
						created.addRegion(seg.uid, seg.start_ms, seg.end_ms, colorOf(seg.label_id));
					}
					if (st.selectedLabelId) created.setDrawColor(colorOf(st.selectedLabelId));
				})
				.catch((err: unknown) => {
					if (!cancelled) {
						waveError =
							err instanceof Error ? err.message : 'Не удалось нарисовать волну';
					}
				});
		}

		return () => {
			cancelled = true;
			w?.onDestroy();
			if (wave === w) wave = null;
			if (session === s) session = null;
		};
	});

	onMount(() => {
		function onKeydown(e: KeyboardEvent): void {
			if (e.key === 'Escape' && confirmEmpty) {
				confirmEmpty = false;
				return;
			}
			if (leaseLost) return;
			const action: HotkeyAction | null = hotkeyForEvent(e, data.labels);
			if (!action) return;
			e.preventDefault();
			switch (action.type) {
				case 'selectLabel':
					selectLabel(action.labelId);
					break;
				case 'togglePlay':
					togglePlay();
					break;
				case 'seek':
					seekBy(action.deltaMs);
					break;
				case 'deleteSelected':
					if (selectedUid) removeSegment(selectedUid);
					break;
			}
		}

		function onBeforeUnload(): void {
			const s = session;
			if (s?.fileDirty) {
				navigator.sendBeacon(
					`/api/v1/files/${s.fileId}/annotations`,
					new Blob([JSON.stringify(s.st.toPayload())], { type: 'application/json' })
				);
			}
		}

		window.addEventListener('keydown', onKeydown);
		window.addEventListener('beforeunload', onBeforeUnload);
		return () => {
			window.removeEventListener('keydown', onKeydown);
			window.removeEventListener('beforeunload', onBeforeUnload);
		};
	});
</script>

<section>
	<header class="topbar">
		<a class="back" href="/projects/{projectId}">← Проект</a>
		<span class="name">{data.file.original_name}</span>
		<span class="status {data.file.status}">{STATUS_LABELS[data.file.status]}</span>
		<span class="savestate" class:dirty>{dirty ? 'правки не сохранены' : 'сохранено'}</span>
		<button class="primary" onclick={onCompleteClick} disabled={completing}>
			{completing ? 'Сохраняем…' : 'Готово ✓'}
		</button>
	</header>

	{#if waveError}
		<p class="error">{waveError}</p>
	{:else if data.file.duration_ms == null}
		<p class="error">Файл ещё обрабатывается — волна недоступна.</p>
	{/if}
	{#if actionError}
		<p class="error">{actionError}</p>
	{/if}
	{#if allDone}
		<div class="banner">
			<p>Все файлы разобраны</p>
			<a class="button" href="/projects/{projectId}">К дашборду</a>
		</div>
	{/if}

	<div class="wave" bind:this={waveContainer}></div>

	<div class="transport">
		<button onclick={togglePlay} disabled={!durationMs} title="Space">
			{playing ? 'Пауза' : 'Играть'}
		</button>
		<button onclick={() => seekBy(-5000)} disabled={!durationMs} title="ArrowLeft">−5 с</button>
		<button onclick={() => seekBy(5000)} disabled={!durationMs} title="ArrowRight">+5 с</button>
		<span class="timecode">{fmt(currentMs)} / {fmt(durationMs)}</span>
		<span class="rates">
			{#each RATES as r (r)}
				<button class:active={rate === r} onclick={() => setRate(r)}>{fmtRate(r)}</button>
			{/each}
		</span>
	</div>

	<div class="palette">
		{#each labels as label (label.id)}
			<button
				class="label"
				class:active={model.selectedLabelId === label.id}
				onclick={() => selectLabel(label.id)}
				title={label.hotkey ? `Клавиша ${label.hotkey}` : undefined}
			>
				<span class="dot" style:background={label.color}></span>
				{label.name}
				{#if label.hotkey}<kbd>{label.hotkey}</kbd>{/if}
			</button>
		{/each}
		{#if labels.length === 0}
			<p class="hint">В проекте нет активных меток — создайте их на дашборде проекта.</p>
		{:else if !model.selectedLabelId}
			<p class="hint">Выберите метку и протяните по волне, чтобы создать сегмент.</p>
		{/if}
	</div>

	{#if model.segments.length === 0}
		<p class="hint">Сегментов пока нет.</p>
	{:else}
		<table class="segments">
			<thead>
				<tr>
					<th></th>
					<th>Метка</th>
					<th>Начало, мс</th>
					<th></th>
					<th>Конец, мс</th>
					<th>Длительность</th>
					<th></th>
				</tr>
			</thead>
			<tbody>
				{#each [...model.segments].sort((a, b) => a.start_ms - b.start_ms) as seg (seg.uid)}
					<tr
						class:selected={selectedUid === seg.uid}
						onclick={() => rowClick(seg)}
						title="Клик — перейти к началу сегмента"
					>
						<td><span class="dot" style:background={colorOf(seg.label_id)}></span></td>
						<td>{nameOf(seg.label_id)}</td>
						<td>
							<input
								type="number"
								min={0}
								max={seg.end_ms - 1}
								step={1}
								value={seg.start_ms}
								onclick={(e) => e.stopPropagation()}
								onchange={(e) => editStart(seg, e)}
							/>
						</td>
						<td>→</td>
						<td>
							<input
								type="number"
								min={seg.start_ms + 1}
								max={durationMs}
								step={1}
								value={seg.end_ms}
								onclick={(e) => e.stopPropagation()}
								onchange={(e) => editEnd(seg, e)}
							/>
						</td>
						<td>{fmt(seg.end_ms - seg.start_ms)}</td>
						<td>
							<button
								class="danger"
								title="Удалить сегмент (Del)"
								onclick={(e) => {
									e.stopPropagation();
									removeSegment(seg.uid);
								}}
							>
								×
							</button>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</section>

{#if confirmEmpty}
	<div class="overlay" role="dialog" aria-modal="true">
		<div class="modal">
			<p>Разметка пуста — пометить файл готовым?</p>
			<div class="row">
				<button class="primary" onclick={() => { confirmEmpty = false; void doComplete(); }}>
					Пометить готовым
				</button>
				<button onclick={() => (confirmEmpty = false)}>Отмена</button>
			</div>
		</div>
	</div>
{/if}

{#if leaseLost}
	<div class="overlay" role="dialog" aria-modal="true">
		<div class="modal">
			<p>Файл вернулся в очередь, ваши правки не сохранены</p>
			<div class="row">
				<a class="button" href="/projects/{projectId}">К дашборду</a>
			</div>
		</div>
	</div>
{/if}

<style>
	.topbar {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		margin-bottom: 0.75rem;
	}

	.topbar .name {
		font-weight: 700;
		overflow-wrap: anywhere;
	}

	.back {
		color: #4363d8;
		text-decoration: none;
		white-space: nowrap;
	}

	.status {
		color: #555;
		font-size: 0.85rem;
		white-space: nowrap;
	}

	.status.done {
		color: #2e7d32;
	}

	.status.error {
		color: #c62828;
	}

	.savestate {
		color: #2e7d32;
		font-size: 0.8rem;
		margin-left: auto;
		white-space: nowrap;
	}

	.savestate.dirty {
		color: #b26a00;
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

	button.primary {
		background: #4363d8;
		color: #fff;
		white-space: nowrap;
	}

	button.primary:hover:not(:disabled) {
		background: #3450b8;
	}

	.wave {
		border: 1px solid #ddd;
		border-radius: 6px;
		margin-bottom: 0.5rem;
	}

	.transport {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 0.4rem;
		margin-bottom: 0.75rem;
	}

	.timecode {
		font-variant-numeric: tabular-nums;
		color: #555;
		margin: 0 0.5rem;
	}

	.rates {
		display: inline-flex;
		gap: 0.25rem;
	}

	.rates button {
		padding: 0.25rem 0.55rem;
		font-size: 0.85rem;
	}

	.rates button.active {
		background: #4363d8;
		color: #fff;
	}

	.palette {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.4rem;
		margin-bottom: 1rem;
	}

	.label {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
	}

	.label.active {
		background: #4363d8;
		color: #fff;
	}

	.dot {
		width: 0.75rem;
		height: 0.75rem;
		border-radius: 999px;
		display: inline-block;
		flex: none;
	}

	kbd {
		border: 1px solid #ddd;
		border-radius: 4px;
		padding: 0 0.3rem;
		font-size: 0.75rem;
		background: #fafafa;
	}

	.label.active kbd {
		border-color: rgba(255, 255, 255, 0.6);
		background: transparent;
	}

	.hint {
		color: #555;
		font-size: 0.9rem;
	}

	.segments {
		border-collapse: collapse;
		width: 100%;
	}

	.segments th,
	.segments td {
		border-bottom: 1px solid #eee;
		padding: 0.3rem 0.5rem;
		text-align: left;
		vertical-align: middle;
	}

	.segments th {
		font-size: 0.8rem;
		color: #555;
	}

	.segments tbody tr {
		cursor: pointer;
	}

	.segments tbody tr:hover {
		background: #f7f8fc;
	}

	.segments tbody tr.selected {
		background: #eef1fd;
	}

	.segments input {
		width: 8.5rem;
		padding: 0.25rem 0.4rem;
		border: 1px solid #ddd;
		border-radius: 4px;
		font-variant-numeric: tabular-nums;
	}

	button.danger {
		border-color: #c62828;
		color: #c62828;
		padding: 0.15rem 0.5rem;
	}

	.banner {
		display: flex;
		align-items: center;
		gap: 1rem;
		border: 1px solid #2e7d32;
		border-radius: 6px;
		padding: 0.5rem 0.75rem;
		margin-bottom: 0.75rem;
	}

	.banner p {
		margin: 0;
		color: #2e7d32;
	}

	.error {
		color: #c62828;
	}

	.overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.4);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 50;
	}

	.modal {
		background: #fff;
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
		max-width: 26rem;
		box-shadow: 0 8px 30px rgba(0, 0, 0, 0.25);
	}

	.modal p {
		margin: 0 0 1rem;
	}

	.modal .row {
		display: flex;
		gap: 0.5rem;
		justify-content: flex-end;
	}
</style>

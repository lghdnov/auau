import WaveSurfer from 'wavesurfer.js';
import RegionsPlugin from 'wavesurfer.js/plugins/regions';
import type { Region } from 'wavesurfer.js/plugins/regions';

/**
 * Изоляция wavesurfer.js: экран разметки работает в миллисекундах и uid-ах
 * сегментов, не зная про секунды и id регионов.
 */

/** Формат пиков версии 1 (app/src/media/peaks.rs): LE, пары i16 [min,max]. */
const PEAKS_MAGIC = 0x61756175;
const PEAKS_VERSION = 1;
const PEAKS_HEADER_LEN = 14;
const PEAKS_PAIR_LEN = 4;

/** Overview-уровень пиков, нормализованный в −1..1 (для опции peaks). */
export interface PeaksArrays {
	min: number[];
	max: number[];
}

/**
 * Разобрать бинарник `GET /media/:id/peaks`: заголовок
 * `magic u32 | version u16 | overview_count u32 | detail_count u32`,
 * затем пары `[min i16, max i16]` overview-уровня и detail-уровень.
 * Возвращает overview, делённый на 32768; битый буфер бросает Error
 * (по мотивам PeaksError::BadMagic/BadVersion/Truncated на бэкенде).
 */
export function decodePeaks(buf: ArrayBuffer): PeaksArrays {
	if (buf.byteLength < PEAKS_HEADER_LEN) throw new Error('peaks data is truncated (header)');
	const view = new DataView(buf);
	if (view.getUint32(0, true) !== PEAKS_MAGIC) throw new Error('peaks data has a bad magic value');
	if (view.getUint16(4, true) !== PEAKS_VERSION) {
		throw new Error('peaks data has an unsupported version');
	}
	const overviewCount = view.getUint32(6, true);
	const detailCount = view.getUint32(10, true);
	const declared = 14 + (overviewCount + detailCount) * PEAKS_PAIR_LEN;
	if (buf.byteLength < declared) throw new Error('peaks data is truncated (payload)');

	const min: number[] = [];
	const max: number[] = [];
	for (let i = 0; i < overviewCount; i++) {
		const off = PEAKS_HEADER_LEN + i * PEAKS_PAIR_LEN;
		min.push(view.getInt16(off, true) / 32768);
		max.push(view.getInt16(off + 2, true) / 32768);
	}
	return { min, max };
}

/** Цвет региона: полупрозрачная заливка, чтобы волна под ним читалась. */
function regionColor(color: string): string {
	const hex = /^#([0-9a-f]{6})$/i.exec(color.trim());
	if (!hex) return color;
	const n = parseInt(hex[1], 16);
	return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, 0.3)`;
}

/** Правка границ региона после правки сегмента в списке. */
export interface WaveRegionPatch {
	startMs?: number;
	endMs?: number;
}

/** Тонкая обёртка над инстансом wavesurfer (времена в мс, адресация по uid). */
export interface Wave {
	/** Инстанс wavesurfer для мелочей вне обёртки (события play/pause и т.п.). */
	readonly ws: WaveSurfer;
	/** Нарисовать регион сегмента; id региона = uid сегмента. */
	addRegion(uid: string, startMs: number, endMs: number, color: string): void;
	/** Подтянуть границы региона за сегментом, правленным в списке. */
	syncRegion(uid: string, patch: WaveRegionPatch): void;
	/** Убрать регион (сегмент удалён). */
	removeRegion(uid: string): void;
	/** Цвет регионов drag-создания (цвет выбранной метки); null — запрет создания. */
	setDrawColor(color: string | null): void;
	/** Drag по волне создал регион: вернуть uid нового сегмента либо null (отменить). */
	onRegionCreated(cb: (startMs: number, endMs: number) => string | null): void;
	/** Пользователь перетащил/растянул регион до новых границ. */
	onRegionDragged(cb: (uid: string, startMs: number, endMs: number) => void): void;
	/** Клик по региону (выбор сегмента). */
	onRegionClicked(cb: (uid: string) => void): void;
	/** Позиция воспроизведения (событие timeupdate). */
	onPlayhead(cb: (ms: number) => void): void;
	seek(ms: number): void;
	play(): void;
	pause(): void;
	/** Скорость воспроизведения 0.5–2×. */
	setRate(rate: number): void;
	/** Полный демонтаж (unsubscribe + destroy wavesurfer). */
	onDestroy(): void;
}

/**
 * Создать волну: пики качаются с `peaksUrl` и рисуются без загрузки аудио,
 * `mediaUrl` подключается как источник media-элемента только для
 * воспроизведения (стриминг с Range).
 */
export async function createWave(
	container: HTMLElement,
	mediaUrl: string,
	peaksUrl: string,
	durationMs: number
): Promise<Wave> {
	const res = await fetch(peaksUrl);
	if (!res.ok) throw new Error(`Не удалось загрузить пики (HTTP ${res.status})`);
	const { min, max } = decodePeaks(await res.arrayBuffer());
	if (min.length === 0) throw new Error('Пики пусты — нечего рисовать');

	const regions = RegionsPlugin.create();
	const ws = WaveSurfer.create({
		container,
		height: 128,
		waveColor: '#9aa7c7',
		progressColor: '#4363d8',
		cursorColor: '#333333',
		plugins: [regions]
	});

	await ws.load(mediaUrl, [max, min], durationMs / 1000);

	const byUid = new Map<string, Region>();
	const programmatic = new Set<string>();

	let onCreated: ((startMs: number, endMs: number) => string | null) | null = null;
	let onDragged: ((uid: string, startMs: number, endMs: number) => void) | null = null;
	let onClicked: ((uid: string) => void) | null = null;
	let onPlayhead: ((ms: number) => void) | null = null;

	regions.on('region-created', (region) => {
		if (programmatic.has(region.id) || byUid.has(region.id)) return;
		const startMs = Math.round(region.start * 1000);
		const endMs = Math.round(region.end * 1000);
		const uid = onCreated?.(startMs, endMs);
		if (!uid) {
			region.remove();
			return;
		}
		region.setOptions({ id: uid });
		byUid.set(uid, region);
	});
	regions.on('region-updated', (region) => {
		if (!byUid.has(region.id)) return;
		onDragged?.(region.id, Math.round(region.start * 1000), Math.round(region.end * 1000));
	});
	regions.on('region-clicked', (region) => {
		if (byUid.has(region.id)) onClicked?.(region.id);
	});
	ws.on('timeupdate', (seconds) => onPlayhead?.(seconds * 1000));

	let disableDragSelection: (() => void) | null = null;

	return {
		ws,
		addRegion(uid, startMs, endMs, color) {
			if (byUid.has(uid)) return;
			programmatic.add(uid);
			try {
				const region = regions.addRegion({
					id: uid,
					start: startMs / 1000,
					end: endMs / 1000,
					color: regionColor(color),
					drag: true,
					resize: true
				});
				byUid.set(uid, region);
			} finally {
				programmatic.delete(uid);
			}
		},
		syncRegion(uid, patch) {
			const region = byUid.get(uid);
			if (!region) return;
			region.setOptions({
				...(patch.startMs !== undefined ? { start: patch.startMs / 1000 } : {}),
				...(patch.endMs !== undefined ? { end: patch.endMs / 1000 } : {})
			});
		},
		removeRegion(uid) {
			byUid.get(uid)?.remove();
			byUid.delete(uid);
		},
		setDrawColor(color) {
			disableDragSelection?.();
			disableDragSelection = null;
			if (color === null) return;
			disableDragSelection = regions.enableDragSelection(
				{ color: regionColor(color), drag: true, resize: true },
				3
			);
		},
		onRegionCreated(cb) {
			onCreated = cb;
		},
		onRegionDragged(cb) {
			onDragged = cb;
		},
		onRegionClicked(cb) {
			onClicked = cb;
		},
		onPlayhead(cb) {
			onPlayhead = cb;
		},
		seek(ms) {
			ws.setTime(ms / 1000);
		},
		play() {
			void ws.play().catch(() => undefined);
		},
		pause() {
			ws.pause();
		},
		setRate(rate) {
			ws.setPlaybackRate(rate);
		},
		onDestroy() {
			ws.destroy();
			byUid.clear();
		}
	};
}

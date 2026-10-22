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

/** Один уровень пиков (overview или detail), нормализованный в −1..1. */
export interface PeaksArrays {
	min: number[];
	max: number[];
}

/** Оба уровня пиков: overview — весь файл, detail — 100 бак/с для зума. */
export interface DecodedPeaks {
	overview: PeaksArrays;
	detail: PeaksArrays;
}

/** Прочитать `count` пар `[min i16, max i16]` с `offset`, деля на 32768. */
function readPairs(view: DataView, offset: number, count: number): PeaksArrays {
	const min: number[] = [];
	const max: number[] = [];
	for (let i = 0; i < count; i++) {
		const off = offset + i * PEAKS_PAIR_LEN;
		min.push(view.getInt16(off, true) / 32768);
		max.push(view.getInt16(off + 2, true) / 32768);
	}
	return { min, max };
}

/**
 * Разобрать бинарник `GET /media/:id/peaks`: заголовок
 * `magic u32 | version u16 | overview_count u32 | detail_count u32`,
 * затем пары `[min i16, max i16]` overview-уровня и detail-уровня (100 бак/с).
 * Возвращает оба уровня, делённые на 32768; битый буфер бросает Error
 * (по мотивам PeaksError::BadMagic/BadVersion/Truncated на бэкенде).
 */
export function decodePeaks(buf: ArrayBuffer): DecodedPeaks {
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

	const overview = readPairs(view, PEAKS_HEADER_LEN, overviewCount);
	const detail = readPairs(view, PEAKS_HEADER_LEN + overviewCount * PEAKS_PAIR_LEN, detailCount);
	return { overview, detail };
}

/** Границы масштаба, px/с. */
export const ZOOM_MIN_PX_PER_SEC = 5;
export const ZOOM_MAX_PX_PER_SEC = 200;
/** Шаг зума: одно колесо/нажатие кнопки меняет масштаб в 1.25 раза. */
export const ZOOM_STEP = 1.25;

/** Прижать px/с к допустимым границам. */
export function clampZoom(pxPerSec: number): number {
	return Math.min(ZOOM_MAX_PX_PER_SEC, Math.max(ZOOM_MIN_PX_PER_SEC, pxPerSec));
}

/** Следующее значение масштаба: ×1.25 при dir=1, ÷1.25 при dir=-1, с clamp. */
export function zoomStep(pxPerSec: number, dir: 1 | -1): number {
	return clampZoom(pxPerSec * (dir === 1 ? ZOOM_STEP : 1 / ZOOM_STEP));
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
	addRegion(
		uid: string,
		startMs: number,
		endMs: number,
		color: string,
		options?: { editable?: boolean }
	): void;
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
	/** Приблизить на один шаг ×1.25 (колесо вверх / кнопка +). */
	zoomIn(): void;
	/** Отдалить на один шаг ÷1.25 (колесо вниз / кнопка −). */
	zoomOut(): void;
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
	const { overview, detail } = decodePeaks(await res.arrayBuffer());
	const level = detail.min.length > 0 ? detail : overview;
	if (level.min.length === 0) throw new Error('Пики пусты — нечего рисовать');

	const regions = RegionsPlugin.create();
	const ws = WaveSurfer.create({
		container,
		height: 128,
		waveColor: '#9aa7c7',
		progressColor: '#4363d8',
		cursorColor: '#333333',
		plugins: [regions]
	});

	await ws.load(mediaUrl, [level.max, level.min], durationMs / 1000);

	let zoomPxPerSec: number | null = null;

	function effectivePxPerSec(): number {
		if (zoomPxPerSec !== null) return zoomPxPerSec;
		const durationSec = ws.getDuration();
		if (!durationSec || container.clientWidth === 0) return ZOOM_MIN_PX_PER_SEC;
		return clampZoom(container.clientWidth / durationSec);
	}

	function applyZoom(dir: 1 | -1): void {
		zoomPxPerSec = zoomStep(effectivePxPerSec(), dir);
		ws.zoom(zoomPxPerSec);
	}

	function onWheel(event: WheelEvent): void {
		if (event.deltaY === 0) return;
		event.preventDefault();
		applyZoom(event.deltaY < 0 ? 1 : -1);
	}
	container.addEventListener('wheel', onWheel, { passive: false });

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
		addRegion(uid, startMs, endMs, color, options) {
			if (byUid.has(uid)) return;
			const editable = options?.editable ?? true;
			programmatic.add(uid);
			try {
				const region = regions.addRegion({
					id: uid,
					start: startMs / 1000,
					end: endMs / 1000,
					color: regionColor(color),
					drag: editable,
					resize: editable
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
		zoomIn() {
			applyZoom(1);
		},
		zoomOut() {
			applyZoom(-1);
		},
		onDestroy() {
			container.removeEventListener('wheel', onWheel);
			ws.destroy();
			byUid.clear();
		}
	};
}

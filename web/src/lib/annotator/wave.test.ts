import { describe, expect, it } from 'vitest';
import {
	clampZoom,
	decodePeaks,
	ZOOM_MAX_PX_PER_SEC,
	ZOOM_MIN_PX_PER_SEC,
	zoomStep
} from './wave';

/** Минимальный аналог app/src/media/peaks.rs::encode — только для тестов. */
function encode(
	overview: [number, number][],
	detail: [number, number][] = [],
	opts: { magic?: number; version?: number; declaredOverview?: number } = {}
): ArrayBuffer {
	const magic = opts.magic ?? 0x61756175;
	const version = opts.version ?? 1;
	const buf = new ArrayBuffer(14 + (overview.length + detail.length) * 4);
	const v = new DataView(buf);
	v.setUint32(0, magic, true);
	v.setUint16(4, version, true);
	v.setUint32(6, opts.declaredOverview ?? overview.length, true);
	v.setUint32(10, detail.length, true);
	let off = 14;
	for (const [min, max] of [...overview, ...detail]) {
		v.setInt16(off, min, true);
		v.setInt16(off + 2, max, true);
		off += 4;
	}
	return buf;
}

describe('decodePeaks', () => {
	it('читает overview-уровень парами [min,max] и нормализует делением на 32768', () => {
		const buf = encode(
			[
				[-1000, 2000],
				[-32768, 32767]
			],
			[[-5, 5]]
		);
		const { overview } = decodePeaks(buf);
		expect(overview.min).toEqual([-1000 / 32768, -1]);
		expect(overview.max).toEqual([2000 / 32768, 32767 / 32768]);
	});

	it('возвращает detail-уровень (100 бак/с): длина = duration_s × 100', () => {
		const detail: [number, number][] = Array.from({ length: 300 }, (_, i) => [-i, i]);
		const buf = encode([[-100, 100]], detail);
		const { detail: d } = decodePeaks(buf);
		expect(d.min).toHaveLength(300);
		expect(d.max).toHaveLength(300);
		expect(d.min[0]).toBe(0 / 32768);
		expect(d.max[299]).toBe(299 / 32768);
	});

	it('detail читается со сдвигом за overview', () => {
		const buf = encode(
			[
				[-1, 1],
				[-2, 2]
			],
			[
				[-30, 30],
				[-40, 40]
			]
		);
		const { overview, detail } = decodePeaks(buf);
		expect(overview.min).toEqual([-1 / 32768, -2 / 32768]);
		expect(detail.min).toEqual([-30 / 32768, -40 / 32768]);
		expect(detail.max).toEqual([30 / 32768, 40 / 32768]);
	});

	it('пустой overview — пустые массивы', () => {
		const { overview } = decodePeaks(encode([], [[1, 2]]));
		expect(overview.min).toEqual([]);
		expect(overview.max).toEqual([]);
	});

	it('неверный magic — ошибка', () => {
		expect(() => decodePeaks(encode([[0, 0]], [], { magic: 0xdeadbeef }))).toThrow(/magic/i);
	});

	it('неподдерживаемая версия — ошибка', () => {
		expect(() => decodePeaks(encode([[0, 0]], [], { version: 2 }))).toThrow(/version/i);
	});

	it('буфер короче заголовка — ошибка', () => {
		expect(() => decodePeaks(new ArrayBuffer(10))).toThrow(/trunc/i);
	});

	it('объявленный overview длиннее реальных данных — ошибка', () => {
		expect(() => decodePeaks(encode([[0, 0]], [], { declaredOverview: 5 }))).toThrow(/trunc/i);
	});

	it('detail-часть тоже должна присутствовать целиком — ошибка при обрезанном хвосте', () => {
		const full = encode([[0, 0]], [
			[1, 1],
			[2, 2]
		]);
		const truncated = full.slice(0, full.byteLength - 4);
		expect(() => decodePeaks(truncated)).toThrow(/trunc/i);
	});
});

describe('zoomStep / clampZoom', () => {
	it('шаг ×1.25 вверх и ÷1.25 вниз', () => {
		expect(zoomStep(100, 1)).toBe(125);
		expect(zoomStep(100, -1)).toBe(80);
		expect(zoomStep(80, 1)).toBe(100);
	});

	it('clamp на границах 5 и 200 px/с', () => {
		expect(zoomStep(200, 1)).toBe(ZOOM_MAX_PX_PER_SEC);
		expect(zoomStep(5, -1)).toBe(ZOOM_MIN_PX_PER_SEC);
		expect(clampZoom(0.5)).toBe(5);
		expect(clampZoom(100000)).toBe(200);
		expect(clampZoom(42)).toBe(42);
	});
});

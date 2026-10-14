import { describe, expect, it } from 'vitest';
import { decodePeaks } from './wave';

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
		const { min, max } = decodePeaks(buf);
		expect(min).toEqual([-1000 / 32768, -1]);
		expect(max).toEqual([2000 / 32768, 32767 / 32768]);
	});

	it('пустой overview — пустые массивы', () => {
		const { min, max } = decodePeaks(encode([], [[1, 2]]));
		expect(min).toEqual([]);
		expect(max).toEqual([]);
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

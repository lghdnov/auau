import { describe, expect, it } from 'vitest';
import { hotkeyForEvent } from './hotkeys';
import type { Label } from '../types';

function label(id: string, hotkey: string | null, archived = false): Label {
	return {
		id,
		project_id: 'p1',
		name: `метка-${id}`,
		color: '#4363d8',
		hotkey,
		sort_order: 0,
		archived
	};
}

/** Node не даёт глобального KeyboardEvent — стоит структурная заглушка. */
function key(k: string, opts: { mod?: 'ctrl' | 'meta' | 'alt'; target?: unknown } = {}): KeyboardEvent {
	return {
		key: k,
		ctrlKey: opts.mod === 'ctrl',
		metaKey: opts.mod === 'meta',
		altKey: opts.mod === 'alt',
		target: opts.target ?? null
	} as KeyboardEvent;
}

const TARGETS: [string, unknown][] = [
	['input', { tagName: 'INPUT' }],
	['textarea', { tagName: 'TEXTAREA' }],
	['select', { tagName: 'SELECT' }],
	['contentEditable', { tagName: 'DIV', isContentEditable: true }]
];

describe('hotkeyForEvent: маппинг клавиш', () => {
	const labels = [label('a', '1'), label('b', '2')];

	it('цифра выбирает метку с соответствующим hotkey', () => {
		expect(hotkeyForEvent(key('1'), labels)).toEqual({ type: 'selectLabel', labelId: 'a' });
		expect(hotkeyForEvent(key('2'), labels)).toEqual({ type: 'selectLabel', labelId: 'b' });
	});

	it('цифра без метки с таким hotkey — null', () => {
		expect(hotkeyForEvent(key('3'), labels)).toBeNull();
		expect(hotkeyForEvent(key('9'), [label('a', null)])).toBeNull();
	});

	it('hotkey архивированной метки не срабатывает', () => {
		const archived = [label('a', '1', true)];
		expect(hotkeyForEvent(key('1'), archived)).toBeNull();
	});

	it('Delete и Backspace удаляют выбранный сегмент', () => {
		expect(hotkeyForEvent(key('Delete'), labels)).toEqual({ type: 'deleteSelected' });
		expect(hotkeyForEvent(key('Backspace'), labels)).toEqual({ type: 'deleteSelected' });
	});

	it('Space — play/pause', () => {
		expect(hotkeyForEvent(key(' '), labels)).toEqual({ type: 'togglePlay' });
	});

	it('стрелки перематывают на 5 секунд', () => {
		expect(hotkeyForEvent(key('ArrowLeft'), labels)).toEqual({ type: 'seek', deltaMs: -5000 });
		expect(hotkeyForEvent(key('ArrowRight'), labels)).toEqual({ type: 'seek', deltaMs: 5000 });
	});

	it('прочие клавиши — null', () => {
		expect(hotkeyForEvent(key('a'), labels)).toBeNull();
		expect(hotkeyForEvent(key('Enter'), labels)).toBeNull();
		expect(hotkeyForEvent(key('0'), labels)).toBeNull();
	});

	it('комбинации с Ctrl/Meta/Alt не перехватываются', () => {
		expect(hotkeyForEvent(key('1', { mod: 'meta' }), labels)).toBeNull();
		expect(hotkeyForEvent(key('1', { mod: 'ctrl' }), labels)).toBeNull();
		expect(hotkeyForEvent(key(' ', { mod: 'alt' }), labels)).toBeNull();
	});
});

describe('hotkeyForEvent: фокус в поле ввода', () => {
	it.each(TARGETS)('фокус в %s — все хоткеи игнорируются', (_name, target) => {
		const labels = [label('a', '1')];
		expect(hotkeyForEvent(key('1', { target }), labels)).toBeNull();
		expect(hotkeyForEvent(key('Delete', { target }), labels)).toBeNull();
		expect(hotkeyForEvent(key(' ', { target }), labels)).toBeNull();
		expect(hotkeyForEvent(key('ArrowRight', { target }), labels)).toBeNull();
	});

	it('обычный элемент (div без contentEditable) не блокирует хоткеи', () => {
		const labels = [label('a', '1')];
		expect(hotkeyForEvent(key('1', { target: { tagName: 'DIV' } }), labels)).toEqual({
			type: 'selectLabel',
			labelId: 'a'
		});
	});
});

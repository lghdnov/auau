import type { Label } from '../types';

/** Действие экрана разметки, на которое раскладывается нажатие клавиши. */
export type HotkeyAction =
	| { type: 'selectLabel'; labelId: string }
	| { type: 'deleteSelected' }
	| { type: 'togglePlay' }
	| { type: 'seek'; deltaMs: number };

/** Шаг перемотки стрелками — 5 секунд. */
const SEEK_STEP_MS = 5000;

/**
 * Фокус в поле ввода — хоткеи перехватывать нельзя (пользователь печатает).
 * Проверка структурная (tagName/isContentEditable), без instanceof DOM-классов:
 * функция остаётся чистой и тестируемой в node.
 */
function isTypingTarget(target: EventTarget | null): boolean {
	const el = target as { tagName?: string; isContentEditable?: boolean } | null;
	if (!el) return false;
	const tag = (el.tagName ?? '').toLowerCase();
	return tag === 'input' || tag === 'textarea' || tag === 'select' || el.isContentEditable === true;
}

/**
 * Разложить событие клавиатуры на действие экрана разметки:
 * `1`–`9` — выбор метки по `label.hotkey` (архивированные не считаются),
 * `Delete`/`Backspace` — удалить выбранный сегмент, `Space` — play/pause,
 * `←`/`→` — перемотка ∓/±5 с.
 *
 * `null` — событие не является хоткеем: посторонняя клавиша, фокус в поле
 * ввода/textarea/select/contentEditable или комбинация с Ctrl/Meta/Alt
 * (не отбираем у браузера его шорткаты).
 */
export function hotkeyForEvent(e: KeyboardEvent, labels: Label[]): HotkeyAction | null {
	if (e.ctrlKey || e.metaKey || e.altKey) return null;
	if (isTypingTarget(e.target)) return null;

	if (e.key === ' ') return { type: 'togglePlay' };
	if (e.key === 'ArrowLeft') return { type: 'seek', deltaMs: -SEEK_STEP_MS };
	if (e.key === 'ArrowRight') return { type: 'seek', deltaMs: SEEK_STEP_MS };
	if (e.key === 'Delete' || e.key === 'Backspace') return { type: 'deleteSelected' };

	if (/^[1-9]$/.test(e.key)) {
		const label = labels.find((l) => !l.archived && l.hotkey === e.key);
		return label ? { type: 'selectLabel', labelId: label.id } : null;
	}
	return null;
}

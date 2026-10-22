import type { FileCounts } from './types';

/**
 * Процент готовности проекта:
 * done / (new + in_progress + done + error + processing) * 100 —
 * файлы в обработке тоже ждут разметки, поэтому входят в знаменатель.
 * Пустой проект (total 0) — 0; результат округляется до целого.
 */
export function progressPercent(counts: FileCounts): number {
	const total =
		counts.new + counts.in_progress + counts.done + counts.error + counts.processing;
	if (total === 0) return 0;
	return Math.round((counts.done / total) * 100);
}

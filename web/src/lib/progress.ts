import type { FileCounts } from './types';

/**
 * Процент готовности проекта: done / (new + in_progress + done + error) * 100.
 * Пустой проект (total 0) — 0; результат округляется до целого.
 */
export function progressPercent(counts: FileCounts): number {
	const total = counts.new + counts.in_progress + counts.done + counts.error;
	if (total === 0) return 0;
	return Math.round((counts.done / total) * 100);
}

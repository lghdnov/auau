import { describe, expect, it } from 'vitest';
import { progressPercent } from './progress';
import type { FileCounts } from './types';

/** counts со всеми нулями, кроме перечисленных полей. */
function counts(partial: Partial<FileCounts>): FileCounts {
	return { new: 0, in_progress: 0, done: 0, error: 0, processing: 0, ...partial };
}

describe('progressPercent', () => {
	it('empty_project_returns_0', () => {
		expect(progressPercent(counts({}))).toBe(0);
	});

	it('half_done_returns_50', () => {
		expect(progressPercent(counts({ new: 2, done: 2 }))).toBe(50);
	});

	it('counts_every_status_in_total', () => {
		expect(progressPercent(counts({ new: 1, in_progress: 1, done: 1, error: 1 }))).toBe(25);
	});

	it('processing_counts_in_total', () => {
		expect(
			progressPercent(counts({ new: 1, in_progress: 1, done: 1, error: 1, processing: 1 }))
		).toBe(20);
		expect(progressPercent(counts({ done: 1, processing: 1 }))).toBe(50);
	});

	it('in_progress_and_error_are_not_done', () => {
		expect(progressPercent(counts({ in_progress: 3, error: 2 }))).toBe(0);
	});

	it('all_done_returns_100', () => {
		expect(progressPercent(counts({ done: 7 }))).toBe(100);
	});

	it('rounds_to_integer', () => {
		expect(progressPercent(counts({ done: 1, new: 1, in_progress: 1 }))).toBe(33);
		expect(progressPercent(counts({ done: 2, new: 1 }))).toBe(67);
	});
});

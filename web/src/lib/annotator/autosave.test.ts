import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createAutosaver } from './autosave';

describe('createAutosaver', () => {
	beforeEach(() => {
		vi.useFakeTimers();
	});

	afterEach(() => {
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	it('two_notifications_within_1_4s_debounce_into_single_save', async () => {
		const save = vi.fn().mockResolvedValue(undefined);
		const autosaver = createAutosaver(() => true, save);

		autosaver.notifyChanged();
		await vi.advanceTimersByTimeAsync(1400);
		expect(save).not.toHaveBeenCalled();

		autosaver.notifyChanged();
		await vi.advanceTimersByTimeAsync(1499);
		expect(save).not.toHaveBeenCalled();

		await vi.advanceTimersByTimeAsync(1);
		expect(save).toHaveBeenCalledTimes(1);
	});

	it('flush_saves_immediately_and_cancels_pending_timer', async () => {
		const save = vi.fn().mockResolvedValue(undefined);
		const autosaver = createAutosaver(() => true, save);

		autosaver.notifyChanged();
		await autosaver.flush();
		expect(save).toHaveBeenCalledTimes(1);

		await vi.advanceTimersByTimeAsync(5000);
		expect(save).toHaveBeenCalledTimes(1);
	});

	it('not_dirty_at_expiry_skips_save', async () => {
		const save = vi.fn().mockResolvedValue(undefined);
		let dirty = true;
		const autosaver = createAutosaver(() => dirty, save);

		autosaver.notifyChanged();
		dirty = false;
		await vi.advanceTimersByTimeAsync(1500);
		expect(save).not.toHaveBeenCalled();
	});

	it('flush_when_not_dirty_is_noop', async () => {
		const save = vi.fn().mockResolvedValue(undefined);
		const autosaver = createAutosaver(() => false, save);

		await autosaver.flush();
		await autosaver.flush();
		expect(save).not.toHaveBeenCalled();
	});

	it('save_error_is_swallowed_and_logged', async () => {
		const error = new Error('сеть недоступна');
		const save = vi.fn().mockRejectedValue(error);
		const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
		const autosaver = createAutosaver(() => true, save);

		autosaver.notifyChanged();
		await vi.advanceTimersByTimeAsync(1500);
		expect(save).toHaveBeenCalledTimes(1);
		expect(consoleError).toHaveBeenCalledWith(expect.anything(), error);
	});

	it('debounce_default_is_1500ms', async () => {
		const save = vi.fn().mockResolvedValue(undefined);
		const autosaver = createAutosaver(() => true, save);

		autosaver.notifyChanged();
		await vi.advanceTimersByTimeAsync(1499);
		expect(save).not.toHaveBeenCalled();
		await vi.advanceTimersByTimeAsync(1);
		expect(save).toHaveBeenCalledTimes(1);
	});
});

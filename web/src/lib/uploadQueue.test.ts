import { describe, expect, it, vi } from 'vitest';
import { ApiError } from './api';
import { runUploadQueue } from './uploadQueue';
import type { UploadResult } from './types';

/** Файл-заглушка с уникальным именем. */
function file(name: string): File {
	return new File([new Uint8Array([1])], name);
}

/** UploadResult-заглушка. */
function result(name: string, status: UploadResult['status'], message: string | null): UploadResult {
	return { original_name: name, status, file_id: null, message };
}

describe('runUploadQueue', () => {
	it('queue_continues_after_duplicate_and_error', async () => {
		const files = [file('a.wav'), file('b.wav'), file('c.wav')];
		const duplicate = result('b.wav', 'duplicate', 'duplicate of existing file');
		const accepted = result('a.wav', 'accepted', null);

		let active = 0;
		let maxActive = 0;
		const uploadFile = vi.fn(async (_projectId: string, f: File): Promise<UploadResult> => {
			active++;
			maxActive = Math.max(maxActive, active);
			try {
				if (f.name === 'b.wav') return duplicate;
				if (f.name === 'c.wav') throw new ApiError('http_502', 'bad gateway');
				return accepted;
			} finally {
				active--;
			}
		});

		const onResult = vi.fn();
		await runUploadQueue('p1', files, { uploadFile }, onResult);

		expect(uploadFile).toHaveBeenCalledTimes(3);
		expect(uploadFile).toHaveBeenNthCalledWith(1, 'p1', files[0]);
		expect(uploadFile).toHaveBeenNthCalledWith(2, 'p1', files[1]);
		expect(uploadFile).toHaveBeenNthCalledWith(3, 'p1', files[2]);
		expect(maxActive).toBe(1);

		expect(onResult).toHaveBeenCalledTimes(3);
		expect(onResult).toHaveBeenNthCalledWith(1, files[0], accepted);
		expect(onResult).toHaveBeenNthCalledWith(2, files[1], duplicate);
		expect(onResult).toHaveBeenNthCalledWith(3, files[2], 'error', 'bad gateway');
	});
});

import { ApiError } from './api';
import type { UploadResult } from './types';

/** Доля API-клиента, нужная очереди; легко подменяется в тестах. */
export interface UploadApi {
	uploadFile(projectId: string, file: File): Promise<UploadResult>;
}

/** Итог одного файла: вердикт бэкенда или транспортная ошибка. */
export type UploadOutcome = UploadResult | 'error';

export type UploadResultCallback = (
	file: File,
	result: UploadOutcome,
	errorMessage?: string
) => void;

/**
 * Последовательная загрузка: по одному файлу на запрос, в порядке списка.
 * Ошибка одного файла (ApiError/сеть) не роняет очередь — колбэк получает
 * `'error'` и сообщение, очередь переходит к следующему файлу.
 */
export async function runUploadQueue(
	projectId: string,
	files: File[],
	apiLike: UploadApi,
	onResult: UploadResultCallback
): Promise<void> {
	for (const file of files) {
		try {
			onResult(file, await apiLike.uploadFile(projectId, file));
		} catch (err) {
			const message =
				err instanceof ApiError ? err.message : 'Не удалось связаться с сервером';
			onResult(file, 'error', message);
		}
	}
}

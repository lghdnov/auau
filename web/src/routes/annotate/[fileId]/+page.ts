import { error } from '@sveltejs/kit';
import { api, ApiError } from '$lib/api';
import type { PageLoad } from './$types';

/**
 * Экран разметки: файл, его сегменты и палитра меток проекта.
 * 404 (файла нет) — страница ошибки.
 */
export const load: PageLoad = async ({ params }) => {
	try {
		const file = await api.getFile(params.fileId);
		const [annotations, detail] = await Promise.all([
			api.getAnnotations(params.fileId),
			api.getProject(file.project_id)
		]);
		return { file, annotations, project: detail.project, labels: detail.labels };
	} catch (err) {
		if (err instanceof ApiError && err.code === 'not_found') {
			error(404, 'Файл не найден');
		}
		throw err;
	}
};

import { error } from '@sveltejs/kit';
import { api, ApiError } from '$lib/api';
import type { FileStatus } from '$lib/types';
import type { PageLoad } from './$types';

const STATUSES: FileStatus[] = ['processing', 'new', 'in_progress', 'done', 'error'];

/** Дашборд проекта: палитра меток, счётчики и файлы (опционально по статусу ?status=). */
export const load: PageLoad = async ({ params, url }) => {
	const raw = url.searchParams.get('status');
	const status = STATUSES.includes(raw as FileStatus) ? (raw as FileStatus) : undefined;

	try {
		const detail = await api.getProject(params.id);
		const files = await api.listFiles(params.id, status);
		return { detail, files, status: status ?? null };
	} catch (err) {
		if (err instanceof ApiError && err.code === 'not_found') {
			error(404, 'Проект не найден');
		}
		throw err;
	}
};

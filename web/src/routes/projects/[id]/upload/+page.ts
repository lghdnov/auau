import { error } from '@sveltejs/kit';
import { api, ApiError } from '$lib/api';
import type { PageLoad } from './$types';

/** Страница загрузки: проверяем проект до рендера, 404 — как на дашборде. */
export const load: PageLoad = async ({ params }) => {
	try {
		const detail = await api.getProject(params.id);
		return { project: detail.project };
	} catch (err) {
		if (err instanceof ApiError && err.code === 'not_found') {
			error(404, 'Проект не найден');
		}
		throw err;
	}
};

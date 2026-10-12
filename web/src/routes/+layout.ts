import { api, ApiError } from '$lib/api';

export const ssr = false;
export const prerender = false;

/** Auth guard: без сессии уходим на вход Pocket ID. */
export const load = async () => {
	try {
		const user = await api.me();
		return { user };
	} catch (err) {
		if (err instanceof ApiError && err.code === 'unauthorized') {
			window.location.href = '/api/v1/auth/login';
			return {};
		}
		throw err;
	}
};

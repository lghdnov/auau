import { afterEach, describe, expect, it, vi } from 'vitest';
import { api, ApiError } from './api';

/** Ответ с JSON-телом и заданным статусом. */
function jsonResponse(status: number, body: unknown): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

/** Подменить глобальный fetch моком; вызвать `use(...)`, чтобы задать ответ. */
function mockFetch() {
	const use = vi.fn(
		async (_input: RequestInfo | URL, _init?: RequestInit): Promise<Response> =>
			new Response(null, { status: 500 })
	);
	vi.stubGlobal('fetch', use);
	return use;
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe('api', () => {
	it('parse_error_body_throws_api_error', async () => {
		const fetchMock = mockFetch();
		fetchMock.mockResolvedValueOnce(
			jsonResponse(409, {
				error: { code: 'duplicate', message: 'label already exists' }
			})
		);

		const thrown = await api
			.createLabel('b1c2', { name: 'смех' })
			.catch((err: unknown) => err);

		expect(thrown).toBeInstanceOf(ApiError);
		expect(thrown).toBeInstanceOf(Error);
		expect((thrown as ApiError).code).toBe('duplicate');
		expect((thrown as ApiError).message).toBe('label already exists');
	});

	it('claim_204_resolves_null', async () => {
		const fetchMock = mockFetch();
		fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));

		await expect(api.claimNext('c3d4')).resolves.toBeNull();

		const [input, init] = fetchMock.mock.calls[0];
		expect(input).toBe('/api/v1/projects/c3d4/queue/claim?order=created');
		expect(init?.method).toBe('POST');
		expect(init?.credentials).toBe('same-origin');
	});

	it('claim_200_returns_file', async () => {
		const fetchMock = mockFetch();
		const file = { id: 'f1', project_id: 'p1', status: 'in_progress' };
		fetchMock.mockResolvedValueOnce(jsonResponse(200, file));

		await expect(api.claimNext('p1')).resolves.toEqual(file);
	});

	it('me_parses_user', async () => {
		const fetchMock = mockFetch();
		const user = { id: 'u1', name: 'Тест Тестов', email: 't@example.org' };
		fetchMock.mockResolvedValueOnce(jsonResponse(200, user));

		await expect(api.me()).resolves.toEqual(user);
		const [input, init] = fetchMock.mock.calls[0];
		expect(input).toBe('/api/v1/me');
		expect(init?.credentials).toBe('same-origin');
	});

	it('put_annotations_sends_camelcase_body', async () => {
		const fetchMock = mockFetch();
		const stored = [{ id: 'a1', label_id: 'l1', start_ms: 0, end_ms: 1000 }];
		fetchMock.mockResolvedValueOnce(jsonResponse(200, stored));

		const segments = [{ labelId: 'l1', startMs: 0, endMs: 1000 }];
		await expect(api.putAnnotations('f1', segments)).resolves.toEqual(stored);

		const [input, init] = fetchMock.mock.calls[0];
		expect(input).toBe('/api/v1/files/f1/annotations');
		expect(init?.method).toBe('PUT');
		expect(init?.headers).toEqual({ 'content-type': 'application/json' });
		expect(JSON.parse(String(init?.body))).toEqual([
			{ labelId: 'l1', startMs: 0, endMs: 1000 }
		]);
	});

	it('upload_file_unwraps_first_result', async () => {
		const fetchMock = mockFetch();
		const results = [
			{ original_name: 'a.wav', status: 'accepted', file_id: 'f9', message: null }
		];
		fetchMock.mockResolvedValueOnce(jsonResponse(201, results));

		const blob = new Blob([new Uint8Array([1, 2, 3])], { type: 'audio/wav' });
		await expect(api.uploadFile('p1', new File([blob], 'a.wav'))).resolves.toEqual(
			results[0]
		);

		const [input, init] = fetchMock.mock.calls[0];
		expect(input).toBe('/api/v1/projects/p1/files');
		expect(init?.method).toBe('POST');
		expect(init?.body).toBeInstanceOf(FormData);
		expect((init?.body as FormData).get('file')).toBeInstanceOf(File);
	});

	it('upload_empty_results_throws_api_error', async () => {
		const fetchMock = mockFetch();
		fetchMock.mockResolvedValueOnce(jsonResponse(201, []));

		const thrown = await api
			.uploadFile('p1', new File([new Uint8Array()], 'a.wav'))
			.catch((err: unknown) => err);

		expect(thrown).toBeInstanceOf(ApiError);
		expect((thrown as ApiError).code).toBe('empty_upload_result');
	});

	it('error_without_json_body_falls_back_to_http_code', async () => {
		const fetchMock = mockFetch();
		fetchMock.mockResolvedValueOnce(
			new Response('gateway exploded', { status: 502, statusText: '' })
		);

		const thrown = await api.me().catch((err: unknown) => err);
		expect(thrown).toBeInstanceOf(ApiError);
		expect((thrown as ApiError).code).toBe('http_502');
	});

	it('exportUrl_builds_download_link', () => {
		expect(api.exportUrl('p1', 'csv')).toBe('/api/v1/projects/p1/export?format=csv');
		expect(api.exportUrl('p1', 'json')).toBe('/api/v1/projects/p1/export?format=json');
	});
});

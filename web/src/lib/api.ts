import type {
	AnnotationDto,
	AudioFile,
	ExportFormat,
	FileStatus,
	Label,
	Project,
	ProjectDetail,
	ProjectSummary,
	SegmentInput,
	UploadResult,
	User
} from './types';

/**
 * Ошибка API: код и сообщение из конверта `{"error":{"code","message"}}`.
 * Для 422 полярные ошибки лежат только на бэкенде — клиенту достаточно кода.
 */
export class ApiError extends Error {
	readonly code: string;

	constructor(code: string, message: string) {
		super(message);
		this.name = 'ApiError';
		this.code = code;
	}
}

/** Разобрать тело ошибки в ApiError; не-JSON тело даёт код `http_<статус>`. */
async function toApiError(res: Response): Promise<ApiError> {
	let code = `http_${res.status}`;
	let message = res.statusText || `HTTP ${res.status}`;
	try {
		const body = (await res.json()) as { error?: { code?: string; message?: string } };
		if (body?.error) {
			code = body.error.code ?? code;
			message = body.error.message ?? message;
		}
	} catch {
	}
	return new ApiError(code, message);
}

/** fetch с сессией; не-2xx превращается в ApiError. */
async function raw(path: string, init?: RequestInit): Promise<Response> {
	const res = await fetch(path, { credentials: 'same-origin', ...init });
	if (!res.ok) throw await toApiError(res);
	return res;
}

/** Запрос с JSON-ответом. */
async function json<T>(path: string, init?: RequestInit): Promise<T> {
	const res = await raw(path, init);
	return (await res.json()) as T;
}

/** Запрос, который может ответить 204 — тогда результат null. */
async function optional<T>(path: string, init?: RequestInit): Promise<T | null> {
	const res = await raw(path, init);
	if (res.status === 204) return null;
	return (await res.json()) as T;
}

/** Init для JSON-метода с телом. */
function jsonInit(method: string, body: unknown): RequestInit {
	return {
		method,
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify(body)
	};
}

/** Тело PATCH /api/v1/labels/:id; отсутствующие поля сохраняют значения. */
export interface LabelPatch {
	name?: string;
	color?: string;
	hotkey?: string | null;
	sort_order?: number;
}

/** Единственное место, где фронт ходит в сеть. */
export const api = {
	/** Текущий пользователь; без сессии — ApiError с кодом `unauthorized`. */
	me: () => json<User>('/api/v1/me'),

	/** Все проекты со счётчиками файлов. */
	listProjects: () => json<ProjectSummary[]>('/api/v1/projects'),

	/** Создать проект. */
	createProject: (name: string, description?: string) =>
		json<Project>('/api/v1/projects', jsonInit('POST', { name, description })),

	/** Проект с палитрой меток и счётчиками. */
	getProject: (id: string) => json<ProjectDetail>(`/api/v1/projects/${id}`),

	/** Добавить метку в палитру проекта. */
	createLabel: (projectId: string, label: { name: string; hotkey?: string }) =>
		json<Label>(`/api/v1/projects/${projectId}/labels`, jsonInit('POST', label)),

	/** Частично обновить метку. */
	patchLabel: (id: string, patch: LabelPatch) =>
		json<Label>(`/api/v1/labels/${id}`, jsonInit('PATCH', patch)),

	/** Заархивировать метку (DELETE на бэкенде архивирует, а не удаляет). */
	archiveLabel: (id: string) => json<Label>(`/api/v1/labels/${id}`, { method: 'DELETE' }),

	/** Постраничный список файлов проекта, опционально по статусу. */
	listFiles: (projectId: string, status?: FileStatus) =>
		json<AudioFile[]>(
			`/api/v1/projects/${projectId}/files${status ? `?status=${status}` : ''}`
		),

	/** Забрать следующий файл из очереди; 204 (очередь пуста) → null. */
	claimNext: (projectId: string) =>
		optional<AudioFile>(`/api/v1/projects/${projectId}/queue/claim?order=created`, {
			method: 'POST'
		}),

	/** Один файл. */
	getFile: (id: string) => json<AudioFile>(`/api/v1/files/${id}`),

	/** Сегменты файла в порядке таймлайна. */
	getAnnotations: (id: string) =>
		json<AnnotationDto[]>(`/api/v1/files/${id}/annotations`),

	/** Полная замена сегментов файла (тело — camelCase SegmentInput). */
	putAnnotations: (id: string, segments: SegmentInput[]) =>
		json<AnnotationDto[]>(`/api/v1/files/${id}/annotations`, jsonInit('PUT', segments)),

	/** Пометить файл готовым. */
	completeFile: (id: string) =>
		json<AudioFile>(`/api/v1/files/${id}/complete`, { method: 'POST' }),

	/** Вернуть файл в очередь. */
	releaseFile: (id: string) =>
		json<AudioFile>(`/api/v1/files/${id}/release`, { method: 'POST' }),

	/** Открыть готовый файл заново. */
	reopenFile: (id: string) =>
		json<AudioFile>(`/api/v1/files/${id}/reopen`, { method: 'POST' }),

	/** Загрузить один файл; бэкенд отвечает массивом — берём первый элемент. */
	uploadFile: async (projectId: string, file: File): Promise<UploadResult> => {
		const form = new FormData();
		form.append('file', file);
		const results = await json<UploadResult[]>(`/api/v1/projects/${projectId}/files`, {
			method: 'POST',
			body: form
		});
		return results[0];
	},

	/** Удалить файл (204 без тела). */
	deleteFile: async (id: string): Promise<void> => {
		await raw(`/api/v1/files/${id}`, { method: 'DELETE' });
	},

	/** Ссылка на выгрузку датасета — для <a href> / window.open. */
	exportUrl: (projectId: string, format: ExportFormat) =>
		`/api/v1/projects/${projectId}/export?format=${format}`
};

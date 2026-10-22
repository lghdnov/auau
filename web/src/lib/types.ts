/**
 * DTO бэкенда, один к одному.
 *
 * Поля — snake_case ровно как их отдаёт API; единственное исключение —
 * `SegmentInput` тела PUT /files/:id/annotations, который бэкенд принимает
 * в camelCase.
 */

/**
 * Идентичность вошедшего пользователя (GET /api/v1/me).
 *
 * Ровно три поля, как их отдаёт хендлер: oidc_subject и created_at
 * никогда не сериализуются (app/src/api/mod.rs, `me`).
 */
export interface User {
	id: string;
	name: string;
	email: string;
}

/** Проект разметки. */
export interface Project {
	id: string;
	name: string;
	description: string | null;
	created_by: string;
	created_at: string;
}

/** Метка в палитре проекта. */
export interface Label {
	id: string;
	project_id: string;
	name: string;
	color: string;
	hotkey: string | null;
	sort_order: number;
	archived: boolean;
}

/** Жизненный цикл аудиофайла: подготовка медиа и очередь в одном автомате. */
export type FileStatus = 'processing' | 'new' | 'in_progress' | 'done' | 'error';

/** Загруженный аудиофайл с состоянием обработки/очереди. */
export interface AudioFile {
	id: string;
	project_id: string;
	original_name: string;
	storage_key: string;
	sha256: string;
	duration_ms: number | null;
	sample_rate: number | null;
	channels: number | null;
	mime: string;
	size_bytes: number;
	status: FileStatus;
	error_message: string | null;
	claimed_by: string | null;
	claim_expires_at: string | null;
	uploaded_by: string;
	created_at: string;
}

/** Хранимый сегмент как он приходит по проводу (GET, и ответ PUT). */
export interface AnnotationDto {
	id: string;
	label_id: string;
	start_ms: number;
	end_ms: number;
}

/** Сегмент в теле PUT /files/:id/annotations — camelCase по контракту. */
export interface SegmentInput {
	labelId: string;
	startMs: number;
	endMs: number;
}

/** Счётчики файлов проекта по статусам (processing — своя метка «Обработка»). */
export interface FileCounts {
	new: number;
	in_progress: number;
	done: number;
	error: number;
	processing: number;
}

/** Проект в списке дашборда: строка + счётчики файлов. */
export interface ProjectSummary {
	project: Project;
	counts: FileCounts;
}

/** Один проект с живой палитрой меток и счётчиками файлов. */
export interface ProjectDetail {
	project: Project;
	labels: Label[];
	counts: FileCounts;
}

/** Вердикт для одного загруженного файла. */
export type UploadStatus = 'accepted' | 'duplicate' | 'rejected';

/** Результат загрузки одного файла (элемент массива-ответа POST .../files). */
export interface UploadResult {
	original_name: string;
	status: UploadStatus;
	file_id: string | null;
	message: string | null;
}

/** Формат экспорта датасета. */
export type ExportFormat = 'json' | 'csv';

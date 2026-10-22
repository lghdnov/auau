import type { FileStatus } from '$lib/types';

/**
 * Сообщение при 409 `lease_required`: контекст зависит от статуса файла.
 * Файл принят (done) — правки невозможны, нужно переоткрыть; прочее —
 * лиза ушла (другой разметчик / истекла и файл вернулся в очередь).
 */
export function leaseLostMessage(fileStatus: FileStatus | null): string {
	if (fileStatus === 'done') {
		return 'Файл принят (done) и read-only — переоткройте его кнопкой Переоткрыть';
	}
	return 'Файл взят другим разметчиком или вернулся в очередь — ваши правки не сохранены';
}

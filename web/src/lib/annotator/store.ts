import type { SegmentInput } from '../types';

/**
 * Сегмент в сторе аннотатора: серверные поля + клиентский `uid`.
 * uid генерируется только на клиенте (crypto.randomUUID) — ключ реактивности
 * и адресация в UI, на сервер не отправляется.
 */
export interface Segment {
	uid: string;
	label_id: string;
	start_ms: number;
	end_ms: number;
}

/**
 * Чистое состояние редактора аннотаций: без Svelte и без сети.
 * Биндинг к компонентам и сохранение — в слое выше (Task 19).
 */
export class AnnotationState {
	segments: Segment[] = [];
	selectedLabelId: string | null = null;

	/** Выбрать метку палитры для следующих сегментов. */
	selectLabel(id: string): void {
		this.selectedLabelId = id;
	}

	/** Добавить сегмент в конец таймлайна и вернуть его. */
	addSegment(label_id: string, start_ms: number, end_ms: number): Segment {
		const segment: Segment = { uid: crypto.randomUUID(), label_id, start_ms, end_ms };
		this.segments.push(segment);
		return segment;
	}

	/** Подправить границы сегмента; несуществующий uid — безопасный no-op. */
	updateSegment(uid: string, patch: Partial<Pick<Segment, 'start_ms' | 'end_ms'>>): void {
		const segment = this.segments.find((s) => s.uid === uid);
		if (!segment) return;
		if (patch.start_ms !== undefined) segment.start_ms = patch.start_ms;
		if (patch.end_ms !== undefined) segment.end_ms = patch.end_ms;
	}

	/** Удалить сегмент; несуществующий uid — безопасный no-op. */
	removeSegment(uid: string): void {
		const index = this.segments.findIndex((s) => s.uid === uid);
		if (index !== -1) this.segments.splice(index, 1);
	}

	/** Тело PUT /files/:id/annotations: camelCase, без uid. */
	toPayload(): SegmentInput[] {
		return this.segments.map((s) => ({
			labelId: s.label_id,
			startMs: s.start_ms,
			endMs: s.end_ms
		}));
	}
}

import { describe, expect, it } from 'vitest';
import { leaseLostMessage } from './lease';

describe('leaseLostMessage', () => {
	it('done — read-only и подсказка про Переоткрыть', () => {
		const msg = leaseLostMessage('done');
		expect(msg).toContain('read-only');
		expect(msg).toContain('Переоткрыть');
	});

	it('прочие статусы — лиза ушла, правки не сохранены', () => {
		for (const status of ['new', 'in_progress', 'processing', 'error'] as const) {
			const msg = leaseLostMessage(status);
			expect(msg).toContain('ваши правки не сохранены');
			expect(msg).not.toContain('read-only');
		}
	});

	it('статус неизвестен (API недоступен) — тот же нейтральный текст', () => {
		expect(leaseLostMessage(null)).toBe(leaseLostMessage('in_progress'));
	});
});

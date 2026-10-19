import { expect, test, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), 'fixtures', 'sine3s-8khz.wav');
const FIXTURE_NAME = 'sine3s-8khz.wav';

/** Уникальный суффикс: даже переиспользованный сервер со старой БД не даёт коллизий имён. */
function uid(): string {
	return Math.random().toString(36).slice(2, 8);
}

/** Войти без OIDC: dev-login (route существует только в e2e-сборке приложения). */
async function devLogin(page: Page, name: string) {
	const res = await page.request.post('/api/v1/auth/dev-login', { data: { name } });
	expect(res.status(), `dev-login должен дать 200, получил ${res.status()}`).toBe(200);
}

/** id файла из URL экрана разметки /annotate/:fileId. */
function fileIdFromUrl(url: string): string {
	const match = /\/annotate\/([0-9a-f-]{36})/.exec(url);
	if (!match) throw new Error(`URL не похож на экран разметки: ${url}`);
	return match[1]!;
}

/** Статус единственного файла проекта (по API). */
async function soleFileStatus(page: Page, projectId: string): Promise<string> {
	const res = await page.request.get(`/api/v1/projects/${projectId}/files`);
	const files = (await res.json()) as { status: string }[];
	if (files.length !== 1) return `unexpected-files-${files.length}`;
	return files[0]!.status;
}

test('happy_path', async ({ page }) => {
	await devLogin(page, 'Э2Э');

	const projectName = `E2E-${uid()}`;
	await page.goto('/');
	await page.getByLabel('Название').fill(projectName);
	await page.getByRole('button', { name: 'Создать' }).click();

	const card = page.locator('a.card', { hasText: projectName });
	await expect(card).toBeVisible();
	await card.click();

	await expect(page).toHaveURL(/\/projects\/[0-9a-f-]{36}$/);
	const projectId = /\/projects\/([0-9a-f-]{36})$/.exec(page.url())![1]!;
	await expect(page.getByRole('heading', { name: projectName })).toBeVisible();

	async function createLabel(name: string, hotkey: string) {
		const form = page.locator('form.create');
		await form.getByLabel('Название').fill(name);
		await form.getByLabel('Хоткей (необязательно)').fill(hotkey);
		await form.getByRole('button', { name: 'Добавить метку' }).click();
		await expect(page.locator('ul.labels li', { hasText: name })).toContainText(hotkey);
	}
	await createLabel('смех', '1');
	await createLabel('пауза', '2');

	await page.getByRole('link', { name: 'Загрузить файлы' }).click();
	await expect(page).toHaveURL(new RegExp(`/projects/${projectId}/upload$`));
	await page.setInputFiles('input[type="file"]', FIXTURE);
	await expect(page.locator('ul.files li', { hasText: FIXTURE_NAME })).toContainText('Ожидает');
	await page.getByRole('button', { name: 'Загрузить', exact: true }).click();
	await expect(page.locator('ul.files li', { hasText: FIXTURE_NAME })).toContainText('Принято');

	await expect
		.poll(() => soleFileStatus(page, projectId), { timeout: 30_000 })
		.toBe('new');

	await page.getByRole('link', { name: 'К дашборду' }).click();
	await expect(page).toHaveURL(new RegExp(`/projects/${projectId}$`));
	await page.getByRole('button', { name: 'Взять файл' }).click();

	await expect(page).toHaveURL(/\/annotate\/[0-9a-f-]{36}$/);
	const fileId = fileIdFromUrl(page.url());
	await expect(page.locator('.topbar .name')).toHaveText(FIXTURE_NAME);
	await expect(page.locator('.topbar .status')).toHaveText(/В работе|Новый/);

	await expect
		.poll(
			async () =>
				page.evaluate(() => {
					const host = document.querySelector('.wave');
					if (!host) return 0;
					const shadowCanvases = (root: ShadowRoot | null) =>
						root ? root.querySelectorAll('canvas').length : 0;
					const nested = [...host.querySelectorAll('*')].reduce(
						(sum, el) => sum + shadowCanvases(el.shadowRoot),
						0
					);
					return shadowCanvases(host.shadowRoot) + nested + host.querySelectorAll('canvas').length;
				}),
			{ timeout: 30_000 }
		)
		.toBeGreaterThan(0);
	await expect(page.locator('.error')).toHaveCount(0);

	const labelButton = page.locator('button.label', { hasText: 'смех' });
	await labelButton.click();
	await expect(labelButton).toHaveClass(/active/);

	const box = await page.locator('.wave').boundingBox();
	if (!box) throw new Error('нет bounding box у .wave');
	const y = box.y + box.height / 2;
	const x1 = box.x + box.width * 0.25;
	const x2 = box.x + box.width * 0.65;
	await page.mouse.move(x1, y);
	await page.mouse.down();
	await page.mouse.move((x1 + x2) / 2, y, { steps: 6 });
	await page.mouse.move(x2, y, { steps: 6 });
	await page.mouse.up();

	const segRow = page.locator('table.segments tbody tr').first();
	await expect(segRow).toContainText('смех', { timeout: 20_000 });

	await page.getByRole('button', { name: 'Готово' }).click();
	await expect(page.locator('.banner')).toContainText('Все файлы разобраны');

	await expect
		.poll(async () => {
			const res = await page.request.get(`/api/v1/files/${fileId}`);
			return ((await res.json()) as { status: string }).status;
		})
		.toBe('done');

	const exportRes = await page.request.get(`/api/v1/projects/${projectId}/export?format=json`);
	expect(exportRes.status()).toBe(200);
	const dataset = (await exportRes.json()) as {
		schema_version: number;
		files: { name: string; annotations: { label: string }[] }[];
	};
	expect(dataset.schema_version).toBe(1);
	expect(dataset.files).toHaveLength(1);
	expect(dataset.files[0]!.name).toBe(FIXTURE_NAME);
	expect(dataset.files[0]!.annotations.length).toBeGreaterThanOrEqual(1);
	expect(dataset.files[0]!.annotations[0]!.label).toBe('смех');
});

test('two_tabs_claim_race', async ({ browser }) => {
	const ctx1 = await browser.newContext();
	const ctx2 = await browser.newContext();
	const tab1 = await ctx1.newPage();
	const tab2 = await ctx2.newPage();
	await devLogin(tab1, 'race-first');
	await devLogin(tab2, 'race-second');

	const projectName = `E2E-race-${uid()}`;
	const created = await tab1.request.post('/api/v1/projects', { data: { name: projectName } });
	expect(created.status()).toBe(201);
	const projectId = ((await created.json()) as { id: string }).id;

	const uploaded = await tab1.request.post(`/api/v1/projects/${projectId}/files`, {
		multipart: {
			file: { name: FIXTURE_NAME, mimeType: 'audio/wav', buffer: await readFile(FIXTURE) }
		}
	});
	expect(uploaded.status()).toBe(201);
	const [verdict] = (await uploaded.json()) as { status: string }[];
	expect(verdict?.status).toBe('accepted');

	await expect
		.poll(() => soleFileStatus(tab1, projectId), { timeout: 30_000 })
		.toBe('new');

	await tab1.goto(`/projects/${projectId}`);
	await tab2.goto(`/projects/${projectId}`);
	await expect(tab1.getByRole('heading', { name: projectName })).toBeVisible();
	await expect(tab2.getByRole('heading', { name: projectName })).toBeVisible();

	await tab1.getByRole('button', { name: 'Взять файл' }).click();
	await expect(tab1).toHaveURL(/\/annotate\/[0-9a-f-]{36}$/);

	await tab2.getByRole('button', { name: 'Взять файл' }).click();
	await expect(tab2.getByText('Все файлы разобраны')).toBeVisible();

	const claim = await tab2.request.post(
		`/api/v1/projects/${projectId}/queue/claim?order=created`
	);
	expect(claim.status()).toBe(204);

	await ctx1.close();
	await ctx2.close();
});

import { defineConfig } from '@playwright/test';

export default defineConfig({
	testDir: 'e2e',
	timeout: 120_000,
	expect: { timeout: 20_000 },
	workers: 1,
	fullyParallel: false,
	retries: 0,
	reporter: [['list']],
	use: {
		baseURL: 'http://localhost:8080',
		trace: 'retain-on-failure'
	},
	webServer: {
		command: './e2e/up.sh',
		url: 'http://localhost:8080/api/v1/health',
		reuseExistingServer: true,
		timeout: 240_000,
		gracefulShutdown: { signal: 'SIGTERM', timeout: 10_000 }
	}
});

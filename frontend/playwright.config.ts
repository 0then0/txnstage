import { defineConfig } from "@playwright/test";
export default defineConfig({
	testDir: "./tests",
	fullyParallel: false,
	workers: 1,
	globalSetup: "./tests/setup.ts",
	timeout: 30000,
	use: {
		headless: true,
		viewport: { width: 1440, height: 1100 },
		trace: "retain-on-failure",
	},
	reporter: [
		["list"],
		["html", { open: "never" }],
		["json", { outputFile: "../.cache/reports/browser-tests.json" }],
	],
	projects: [
		{
			name: "chromium",
			use: {
				browserName: "chromium",
				...(process.env.PLAYWRIGHT_CHROME ? { channel: "chrome" } : {}),
			},
		},
	],
});

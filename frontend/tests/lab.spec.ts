import { readFile } from "node:fs/promises";
import { expect, type Page, type Request, test } from "@playwright/test";

async function open(page: Page) {
	const { url } = JSON.parse(
		await readFile("../.cache/e2e-launch.json", "utf8"),
	);
	await page.goto(url);
	await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
}
async function choose(page: Page, name: string) {
	await page
		.locator(".scenario-list")
		.getByRole("button", { name: new RegExp(name) })
		.click();
	await expect(page.getByRole("heading", { name, exact: true })).toBeVisible();
	await expect(
		page.getByRole("button", { name: "Run schedule", exact: true }),
	).toBeEnabled();
}
async function next(page: Page) {
	await page.getByRole("button", { name: "Next ready step" }).click();
}
async function edit(page: Page, c: string, sql: string) {
	const editor = page.getByRole("textbox", {
		name: `SQL connection ${c}`,
		exact: true,
	});
	await editor.click();
	await editor.press(process.platform === "darwin" ? "Meta+A" : "Control+A");
	await editor.fill(sql);
}
async function observed(page: Page, id: string, kind: string) {
	await expect(
		page
			.locator(".event")
			.filter({ has: page.locator("code", { hasText: new RegExp(`^${id}$`) }) })
			.filter({
				has: page.locator(".event-kind", { hasText: new RegExp(`^${kind}$`) }),
			}),
	).toHaveCount(1);
}

async function result(page: Page, id: string) {
	await page
		.locator(".event")
		.filter({ has: page.locator("code", { hasText: new RegExp(`^${id}$`) }) })
		.filter({ has: page.locator(".event-kind", { hasText: /^completed$/ }) })
		.click();
	return page.locator(".event-detail").getByRole("table", {
		name: `Result for ${id}`,
		exact: true,
	});
}

for (const [file, name, preserved] of [
	["07", "FK rebuild · cascade loss", false],
	["08", "FK rebuild · OFF inside BEGIN", false],
	["09", "FK rebuild · OFF before BEGIN", true],
	["10", "FK rebuild · deferred checks", false],
	["11", "FK rebuild · WAL observer", false],
	["12", "FK rebuild · preserve children", true],
] as const) {
	test(`FK lab ${file}: catalog, recording import and fresh replay`, async ({
		page,
	}) => {
		await open(page);
		await choose(page, name);
		await expect(page.locator(".scenario-list")).toContainText(
			"FK cascade & data preservation",
		);
		const sqlRequests: string[] = [];
		page.on("request", (request) => {
			if (/\/api\/(execute|reset|replay)$/.test(request.url()))
				sqlRequests.push(request.url());
		});
		await page
			.locator("input[type=file]")
			.setInputFiles(`../examples/recordings/${file}.recording.json`);
		await expect(
			page.getByText("Historical recording · read only"),
		).toBeVisible();
		await expect(
			page.getByRole("button", { name: "Run A", exact: true }),
		).toBeDisabled();
		expect(sqlRequests).toEqual([]);
		await page
			.getByRole("button", { name: "Rerun in fresh DB", exact: true })
			.click();
		await expect(page.getByRole("status")).toContainText(
			"All declared expectations matched",
		);
		const before = await result(page, "a_before_drop");
		await expect(before.locator("tbody tr")).toHaveCount(5);
		await expect(before).toContainText("9007199254740993");
		await expect(before).toContainText("9223372036854775807");
		const dropped = await result(page, "a_after_drop");
		await expect(dropped.locator("tbody tr")).toHaveCount(
			file === "09" ? 5 : 2,
		);
		if (file === "11") {
			const stale = await result(page, "b_stale");
			await expect(stale.locator("tbody tr")).toHaveCount(5);
		}
		const fresh = await result(page, "b_after_commit");
		await expect(fresh.locator("tbody tr")).toHaveCount(preserved ? 5 : 2);
		await expect(fresh).toContainText("north");
		if (preserved) {
			await expect(fresh).toContainText("alpha");
			await expect(fresh).toContainText("9223372036854775807");
			await expect(fresh).toContainText("blob");
			await expect(fresh).toContainText("real");
		}
		await expect(await result(page, "b_integrity")).toContainText("ok");
		await expect(
			(await result(page, "b_fk_check")).locator("tbody tr"),
		).toHaveCount(0);
		await expect(await result(page, "a_fk_effective")).toContainText(
			file === "09" ? "0" : "1",
		);
		await expect(await result(page, "a_defer_effective")).toContainText(
			file === "10" ? "1" : "0",
		);
		if (file === "11") {
			await result(page, "b_stale");
			await page.screenshot({
				path: "../.cache/reports/fk-lab-desktop.png",
				fullPage: true,
			});
			await page.setViewportSize({ width: 390, height: 844 });
			await expect(page.getByLabel("Experiment", { exact: true })).toHaveValue(
				name,
			);
			expect(
				await page.evaluate(
					() => document.documentElement.scrollWidth <= innerWidth,
				),
			).toBe(true);
		}
		for (const connection of ["A", "B"]) {
			await expect(
				page.getByRole("textbox", {
					name: `SQL connection ${connection}`,
					exact: true,
				}),
			).toHaveText("SELECT 1");
			await page
				.getByRole("button", { name: `Run ${connection}`, exact: true })
				.click();
			const card = page.getByRole("article", {
				name: `Connection ${connection}`,
				exact: true,
			});
			const custom = card.getByRole("table", { name: /^Result for manual_/ });
			await expect(custom.locator("tbody tr")).toHaveCount(1);
			await expect(custom.locator(".type-tag")).toHaveText("integer");
			await expect(custom.getByRole("cell")).toContainText("1");
			await expect(card.locator(".sql-error")).toHaveCount(0);
		}
	});
}

test("empty imported scenarios offer executable custom SQL and show the running version", async ({
	page,
}) => {
	await open(page);
	const metadata = JSON.parse(await readFile("./package.json", "utf8"));
	await expect(page.locator(".brand")).toContainText(`v${metadata.version}`);
	await page.locator("input[type=file]").setInputFiles({
		name: "empty.scenario.json",
		mimeType: "application/json",
		buffer: Buffer.from(
			JSON.stringify({
				format_version: 1,
				name: "Empty database",
				description: "Start with an empty database and run custom SQL.",
				settings: {
					journal_mode: "wal",
					busy_ms: 2000,
					deadline_ms: 5000,
					row_limit: 100,
					connections: 2,
				},
				initial: [],
				operations: [],
			}),
		),
	});
	await expect(
		page.getByRole("heading", { name: "Empty database", exact: true }),
	).toBeVisible();
	// An imported scenario has no environment metadata of its own.
	await expect(page.locator(".brand")).toContainText(`v${metadata.version}`);
	await page
		.getByRole("button", { name: "Rerun in fresh DB", exact: true })
		.click();
	await expect(page.getByRole("status")).toContainText(
		"All declared expectations matched",
	);
	await expect(
		page.getByRole("textbox", { name: "SQL connection A", exact: true }),
	).toHaveText("SELECT 1");
	await page.getByRole("button", { name: "Run A", exact: true }).click();
	const card = page.getByRole("article", { name: "Connection A", exact: true });
	await expect(card.getByRole("table").locator("tbody tr")).toHaveCount(1);
	await expect(card.locator(".sql-error")).toHaveCount(0);
});

test("FK preservation assertion fails for loss and passes for the restoration control", async ({
	page,
}) => {
	await open(page);
	await page
		.locator("input[type=file]")
		.setInputFiles(
			"../examples/recordings/fk-preservation-fails.recording.json",
		);
	await expect(
		page.getByText("Historical recording · read only"),
	).toBeVisible();
	await page
		.getByRole("button", { name: "Rerun in fresh DB", exact: true })
		.click();
	await expect(page.getByRole("alert")).toContainText(
		"Row expectation mismatch for b_preservation",
	);
	await observed(page, "a_commit", "completed");
	await observed(page, "b_after_commit", "completed");
	await expect(
		page.locator(".event-kind").filter({ hasText: /^failed$/ }),
	).toHaveCount(0);
	await expect(page.getByRole("status")).toHaveCount(0);
	await expect(
		(await result(page, "b_after_commit")).locator("tbody tr"),
	).toHaveCount(2);
	await choose(page, "FK rebuild · preserve children");
	await page.getByRole("button", { name: "Run schedule", exact: true }).click();
	await expect(page.getByRole("status")).toContainText(
		"All declared expectations matched",
	);
	await expect(
		(await result(page, "b_after_commit")).locator("tbody tr"),
	).toHaveCount(5);
});

test.describe("Documentation screenshot", () => {
	test.use({ deviceScaleFactor: 2 });
	test("snapshot through real UI and processes, typed result and expected 517", async ({
		page,
	}) => {
		await open(page);
		await choose(page, "Stale snapshot");
		await page
			.getByRole("button", { name: "Run schedule", exact: true })
			.click();
		await expect(page.getByRole("status")).toContainText(
			"All declared expectations matched",
		);
		await expect(
			page.locator(".step").filter({ hasText: "UPDATE t SET v = 3" }),
		).toContainText("expected SQL error");
		await observed(page, "a_promote", "failed");
		await page
			.locator(".event")
			.filter({ hasText: "a_promote" })
			.filter({ hasText: "failed" })
			.click();
		await expect(page.locator(".event-detail")).toContainText("517");
		await expect(
			page.locator(".connection").first().locator("td"),
		).toContainText("2");
		await page.screenshot({
			path: "../.cache/reports/workbench-desktop.png",
			fullPage: true,
		});
	});
});
test("B waits while A commits; coordinator stays usable", async ({ page }) => {
	await open(page);
	await choose(page, "Two writers · retry");
	await next(page);
	await observed(page, "a_begin", "completed");
	await next(page);
	await observed(page, "a_write", "completed");
	await next(page);
	await expect(page.locator(".status-busy-observed")).toHaveText(
		"busy observed",
	);
	await expect(
		page.getByRole("button", { name: "Run A", exact: true }),
	).toBeEnabled();
	await page.getByRole("button", { name: "Run A", exact: true }).click();
	await observed(page, "a_commit", "completed");
	await observed(page, "b_write", "completed");
	await expect(page.locator(".status-busy-observed")).toHaveCount(0);
});
test("export, inspect without SQL, explicit rerun in fresh database", async ({
	page,
}) => {
	await open(page);
	await choose(page, "Stale snapshot");
	await page.getByRole("button", { name: "Run schedule", exact: true }).click();
	await expect(page.getByRole("status")).toContainText(
		"All declared expectations matched",
	);
	const download = page.waitForEvent("download");
	await page.getByRole("button", { name: "Export run", exact: true }).click();
	const artifact = await download;
	const file = await artifact.path();
	expect(file).not.toBeNull();
	await page.getByRole("button", { name: "Reset", exact: false }).click();
	await expect(page.locator(".event")).toHaveCount(0);
	await page.locator("input[type=file]").setInputFiles(file as string);
	await expect(
		page.getByText("Historical recording · read only"),
	).toBeVisible();
	await expect(
		page.getByRole("button", { name: "Run A", exact: true }),
	).toBeDisabled();
	await page
		.getByRole("button", { name: "Rerun in fresh DB", exact: true })
		.click();
	await expect(page.getByRole("status")).toContainText(
		"All declared expectations matched",
	);
	await expect(page.getByText("Historical recording · read only")).toHaveCount(
		0,
	);
});
test("SQL error, long SQL cancellation, busy reset, narrow layout", async ({
	page,
}) => {
	await open(page);
	await choose(page, "Stale snapshot");
	await page.getByLabel("Step for connection A").selectOption("");
	await edit(page, "A", "SELECT missing FROM t");
	await page.getByRole("button", { name: "Run A", exact: true }).click();
	await expect(page.locator(".sql-error").first()).toContainText(
		"no such column",
	);
	await edit(
		page,
		"A",
		"WITH RECURSIVE c(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM c WHERE x<1000000000) SELECT sum(x) FROM c",
	);
	await page.getByRole("button", { name: "Run A", exact: true }).click();
	await page.getByRole("button", { name: "Cancel A", exact: true }).click();
	await expect(page.locator(".status-cancelled")).toHaveText("cancelled");
	await choose(page, "Two writers · retry");
	await next(page);
	await observed(page, "a_begin", "completed");
	await next(page);
	await observed(page, "a_write", "completed");
	await next(page);
	await expect(page.locator(".status-busy-observed")).toHaveText(
		"busy observed",
	);
	await page.getByRole("button", { name: "Reset", exact: false }).click();
	await expect(page.locator(".event")).toHaveCount(0);
	await expect(page.locator(".status-busy-observed")).toHaveCount(0);
	await page.setViewportSize({ width: 390, height: 844 });
	await expect(
		page.getByRole("button", { name: "Next ready step" }),
	).toBeVisible();
	const overflow = await page.evaluate(() =>
		Array.from(document.querySelectorAll("body *"))
			.filter((el) => el.getBoundingClientRect().right > innerWidth + 1)
			.map((el) => ({
				tag: el.tagName,
				class: el.className,
				right: el.getBoundingClientRect().right,
			}))
			.slice(0, 12),
	);
	expect(
		await page.evaluate(
			() => document.documentElement.scrollWidth <= innerWidth,
		),
		JSON.stringify(overflow),
	).toBe(true);
	await page.screenshot({
		path: "../.cache/reports/workbench-mobile.png",
		fullPage: true,
	});
});
test("loopback HTTP policy rejects missing token, hostile Host and Origin", async ({
	page,
	request,
}) => {
	await open(page);
	const origin = new URL(page.url()).origin;
	expect((await request.get(`${origin}/api/state`)).status()).toBe(401);
	const token = await page.evaluate(() =>
		sessionStorage.getItem("txnstage-token"),
	);
	expect(
		(
			await request.get(`${origin}/api/state`, {
				headers: { "x-txnstage-token": token ?? "", Host: "evil.test" },
			})
		).status(),
	).toBe(403);
	expect(
		(
			await request.post(`${origin}/api/reset`, {
				headers: {
					"x-txnstage-token": token ?? "",
					Origin: "https://evil.test",
				},
				data: {},
			})
		).status(),
	).toBe(403);
});

async function freshSnapshot(page: Page) {
	await open(page);
	await choose(page, "Stale snapshot");
	await expect(page.getByRole("status")).toContainText(
		"Fresh disposable database",
	);
}
async function exportJson(page: Page, label: string) {
	const downloaded = page.waitForEvent("download");
	await page.getByRole("button", { name: label, exact: true }).click();
	const path = await (await downloaded).path();
	if (!path) throw new Error("Download missing");
	const bytes = await readFile(path);
	return { bytes, value: JSON.parse(bytes.toString("utf8")) };
}
async function painted(page: Page) {
	await page.evaluate(
		() =>
			new Promise<void>((resolve) =>
				requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
			),
	);
}
test("large compact UI recording survives export, import and rerun", async ({
	page,
}) => {
	// Two full replays and multi-MiB artifact I/O run under deliberate CPU throttling.
	test.slow();
	const cdp = await page.context().newCDPSession(page);
	// Exercise the large result on a slower CPU without relaxing completion deadlines.
	await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
	await freshSnapshot(page);
	const columns = Array.from({ length: 100 }, (_, i) => `x AS c${i}`).join(",");
	const scenario = {
		format_version: 1,
		name: "Large recording",
		description: "",
		settings: {
			journal_mode: "wal",
			busy_ms: 0,
			deadline_ms: 5000,
			row_limit: 200,
			connections: 2,
		},
		initial: [],
		operations: Array.from({ length: 8 }, (_, i) => ({
			id: `read_${i}`,
			connection: "A",
			sql: `WITH RECURSIVE c(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM c WHERE x<200) SELECT ${columns} FROM c`,
			params: [],
			after: i ? [{ operation: `read_${i - 1}`, event: "completed" }] : [],
			expect: null,
			note: "",
		})),
	};
	await page.locator("input[type=file]").setInputFiles({
		name: "large.scenario.json",
		mimeType: "application/json",
		buffer: Buffer.from(JSON.stringify(scenario)),
	});
	// Wait for the actual schedule response before checking its rendered outcome.
	const replayResponse = page.waitForResponse(
		(response) =>
			new URL(response.url()).pathname === "/api/replay" &&
			response.request().method() === "POST",
	);
	await page
		.getByRole("button", { name: "Rerun in fresh DB", exact: true })
		.click();
	const replayed = await replayResponse;
	expect(replayed.ok()).toBe(true);
	await replayed.finished();
	await expect(page.getByRole("status")).toContainText(
		"All declared expectations matched",
	);
	const idleResponse = await page.waitForResponse((response) => {
		const url = new URL(response.url());
		return (
			url.pathname === "/api/state" && url.searchParams.get("after") === "26"
		);
	});
	const idleBody = await idleResponse.text();
	expect(Buffer.byteLength(idleBody)).toBeLessThan(50_000);
	expect(JSON.parse(idleBody).events).toEqual([]);
	const table = page.getByRole("table", {
		name: "Result for read_7",
		exact: true,
	});
	const pages = page.getByRole("navigation", {
		name: "Result pages for read_7",
	});
	await expect(table.locator("tbody tr")).toHaveCount(20);
	await expect(pages).toContainText(/Rows 1–20 of \d+/);
	await pages.getByRole("button", { name: "Next rows" }).click();
	await expect(pages).toContainText(/Rows 21–40 of \d+/);
	await expect(
		table.locator("tbody tr").first().locator("td").first(),
	).toHaveText("integer21");
	await pages.getByRole("button", { name: "Previous rows" }).click();
	await expect(pages).toContainText(/Rows 1–20 of \d+/);
	await pages.getByRole("button", { name: "Next rows" }).click();
	const { bytes, value } = await exportJson(page, "Export run");
	expect(bytes.length).toBeLessThanOrEqual(8 * 1024 * 1024);
	expect(Buffer.byteLength(JSON.stringify(value, null, 2))).toBeGreaterThan(
		8 * 1024 * 1024,
	);
	expect(
		value.events.filter((e: { kind: string }) => e.kind === "completed"),
	).toHaveLength(8);
	await page.locator("input[type=file]").setInputFiles({
		name: "large.recording.json",
		mimeType: "application/json",
		buffer: bytes,
	});
	const rerunResponse = page.waitForResponse(
		(response) =>
			new URL(response.url()).pathname === "/api/replay" &&
			response.request().method() === "POST",
	);
	await page
		.getByRole("button", { name: "Rerun in fresh DB", exact: true })
		.click();
	const reran = await rerunResponse;
	expect(reran.ok()).toBe(true);
	await reran.finished();
	await expect(page.getByRole("status")).toContainText(
		"All declared expectations matched",
	);
	await expect(pages).toContainText(/Rows 1–20 of \d+/);
});
test("editing an observed step changes the draft and preserves recorded SQL", async ({
	page,
}) => {
	await freshSnapshot(page);
	await next(page);
	await observed(page, "a_begin", "completed");
	await page.getByLabel("Step for connection A").selectOption("a_begin");
	await edit(page, "A", "SELECT 42");
	await expect(
		page.locator(".step").filter({ hasText: "SELECT 42" }),
	).toContainText("historical observation");
	expect(
		(await exportJson(page, "Export scenario")).value.operations.find(
			(o: { id: string }) => o.id === "a_begin",
		).sql,
	).toBe("SELECT 42");
	const recording = (await exportJson(page, "Export run")).value;
	expect(
		recording.scenario.operations.find(
			(o: { id: string }) => o.id === "a_begin",
		).sql,
	).toBe("BEGIN");
	expect(
		recording.events.find(
			(e: { operation: string; kind: string }) =>
				e.operation === "a_begin" && e.kind === "created",
		).data.sql,
	).toBe("BEGIN");
	await page.getByRole("button", { name: "Reset", exact: false }).click();
	await expect(page.getByRole("status")).toContainText(
		"Fresh disposable database",
	);
	await next(page);
	await observed(page, "a_begin", "completed");
	await expect(page.locator(".connection").first().locator("td")).toContainText(
		"42",
	);
});
for (const reply of ["success", "error"] as const) {
	for (const phase of ["pending", "completed"] as const) {
		test(`late replay ${reply} cannot overwrite ${phase} reset`, async ({
			page,
		}) => {
			await freshSnapshot(page);
			let releaseReplay!: () => void,
				capturedReplay!: () => void,
				releaseReset!: () => void,
				capturedReset!: () => void;
			const replayGate = new Promise<void>((r) => {
				releaseReplay = r;
			});
			const replayCaptured = new Promise<void>((r) => {
				capturedReplay = r;
			});
			const resetGate = new Promise<void>((r) => {
				releaseReset = r;
			});
			const resetCaptured = new Promise<void>((r) => {
				capturedReset = r;
			});
			await page.route("**/api/replay", async (route) => {
				const response = await route.fetch();
				capturedReplay();
				await replayGate;
				if (reply === "success") await route.fulfill({ response });
				else
					await route.fulfill({
						status: 500,
						contentType: "text/plain",
						body: "stale replay failure",
					});
			});
			try {
				await page
					.getByRole("button", { name: "Run schedule", exact: true })
					.click();
				await replayCaptured;
				if (phase === "pending")
					await page.route("**/api/reset", async (route) => {
						const response = await route.fetch();
						capturedReset();
						await resetGate;
						await route.fulfill({ response });
					});
				await page.getByRole("button", { name: "Reset", exact: false }).click();
				if (phase === "pending") await resetCaptured;
				else
					await expect(page.getByRole("status")).toContainText(
						"Fresh disposable database",
					);
				const finished = page.waitForEvent("requestfinished", (r) =>
					r.url().endsWith("/api/replay"),
				);
				releaseReplay();
				await finished;
				await painted(page);
				await expect(page.getByRole("alert")).toHaveCount(0);
				if (phase === "pending") {
					await expect(
						page.getByRole("button", {
							name: "Schedule running…",
							exact: true,
						}),
					).toBeDisabled();
					await expect(page.getByRole("status")).toHaveCount(0);
					releaseReset();
				}
				await expect(page.getByRole("status")).toContainText(
					"Fresh disposable database",
				);
				await expect(page.locator(".event")).toHaveCount(0);
				await expect(page.getByLabel("Step for connection A")).toHaveValue(
					"a_begin",
				);
			} finally {
				releaseReplay();
				releaseReset();
			}
		});
	}
}
test("late execute response cannot select a step after reset", async ({
	page,
}) => {
	await freshSnapshot(page);
	let release!: () => void, captured!: () => void;
	const gate = new Promise<void>((r) => {
		release = r;
	});
	const ready = new Promise<void>((r) => {
		captured = r;
	});
	await page.route("**/api/execute", async (route) => {
		const response = await route.fetch();
		captured();
		await gate;
		await route.fulfill({ response });
	});
	try {
		await next(page);
		await ready;
		await page.getByRole("button", { name: "Reset", exact: false }).click();
		await expect(page.getByRole("status")).toContainText(
			"Fresh disposable database",
		);
		const finished = page.waitForEvent("requestfinished", (r) =>
			r.url().endsWith("/api/execute"),
		);
		release();
		await finished;
		await painted(page);
		await expect(page.getByLabel("Step for connection A")).toHaveValue(
			"a_begin",
		);
		await expect(page.locator(".event")).toHaveCount(0);
		await expect(page.getByRole("alert")).toHaveCount(0);
	} finally {
		release();
	}
});
test("out of order state response cannot restore pre-reset events", async ({
	page,
}) => {
	await freshSnapshot(page);
	await next(page);
	await observed(page, "a_begin", "completed");
	let release!: () => void, captured!: () => void;
	let held = false;
	let heldRequest: Request | undefined;
	const gate = new Promise<void>((r) => {
		release = r;
	});
	const ready = new Promise<void>((r) => {
		captured = r;
	});
	await page.route("**/api/state*", async (route) => {
		if (held) {
			await route.continue();
			return;
		}
		held = true;
		heldRequest = route.request();
		const response = await route.fetch();
		captured();
		await gate;
		await route.fulfill({ response });
	});
	try {
		await ready;
		await page.getByRole("button", { name: "Reset", exact: false }).click();
		await expect(page.getByRole("status")).toContainText(
			"Fresh disposable database",
		);
		await expect(page.locator(".event")).toHaveCount(0);
		const finished = page.waitForEvent(
			"requestfinished",
			(r) => r === heldRequest,
		);
		release();
		await finished;
		await painted(page);
		await expect(page.locator(".event")).toHaveCount(0);
		await expect(page.getByLabel("Step for connection A")).toHaveValue(
			"a_begin",
		);
	} finally {
		release();
	}
});

test("next-step context, keyboard execution and readable event inspection", async ({
	page,
}) => {
	await freshSnapshot(page);
	await expect(page.locator("#next-step-context")).toContainText("BEGIN");
	const progress = page.getByRole("progressbar", {
		name: "Observed schedule steps",
	});
	await expect(progress).toHaveAttribute("value", "0");
	await page.screenshot({
		path: "../.cache/reports/workbench-ready.png",
		fullPage: false,
	});
	await page.getByRole("button", { name: "Next ready step" }).focus();
	await page.keyboard.press("Enter");
	await observed(page, "a_begin", "completed");
	await expect(progress).toHaveAttribute("value", "1");
	await expect(page.locator("#next-step-context")).toContainText(
		"SELECT v FROM t",
	);
	await page.getByRole("button", { name: "Run schedule", exact: true }).click();
	await expect(page.getByRole("status")).toContainText(
		"All declared expectations matched",
	);
	const filter = page.getByLabel("Connection", { exact: true });
	await filter.selectOption("C");
	await expect(page.locator(".timeline-empty")).toContainText(
		"No events for this connection",
	);
	await filter.selectOption("A");
	await expect(page.locator(".event-conn")).not.toContainText(["B"]);
	const failure = page
		.locator(".event")
		.filter({ has: page.locator("code", { hasText: "a_promote" }) })
		.filter({ hasText: "failed" });
	await failure.focus();
	await page.keyboard.press("Enter");
	await expect(failure).toHaveAttribute("aria-pressed", "true");
	await expect(page.locator(".detail-error")).toContainText("517");
	await expect(page.locator(".detail-sql")).toHaveText(
		"UPDATE t SET v = 3 WHERE id = 1",
	);
	await expect(page.locator(".event-facts")).toContainText("Transaction");
	await expect(page.locator(".raw-event")).not.toHaveAttribute("open", "");
	await page.getByText("Raw event JSON", { exact: true }).focus();
	await page.keyboard.press("Enter");
	await expect(page.locator(".raw-event")).toHaveAttribute("open", "");
	await page.screenshot({
		path: "../.cache/reports/workbench-inspection.png",
		fullPage: true,
	});
});

test("responsive experiment selection, settings and long Cyrillic content", async ({
	page,
}) => {
	await freshSnapshot(page);
	await page.setViewportSize({ width: 320, height: 900 });
	const experiment = page.getByLabel("Experiment", { exact: true });
	await expect(experiment).toBeVisible();
	await experiment.selectOption({ label: "06 · Reader & checkpoint" });
	await expect(page.getByRole("heading", { level: 1 })).toHaveText(
		"Reader & checkpoint",
	);
	await experiment.selectOption({ label: "01 · Stale snapshot" });
	await expect(page.getByRole("heading", { level: 1 })).toHaveText(
		"Stale snapshot",
	);
	const settings = page.getByRole("button", {
		name: "Environment & settings",
		exact: true,
	});
	await settings.focus();
	await page.keyboard.press("Enter");
	await expect(settings).toHaveAttribute("aria-expanded", "true");
	await expect(
		page.getByRole("region", { name: "Environment and settings" }),
	).toBeVisible();
	expect(
		await page.evaluate(
			() => document.documentElement.scrollWidth <= innerWidth,
		),
	).toBe(true);
	await settings.click();
	const scenario = JSON.parse(
		await readFile("../examples/01.scenario.json", "utf8"),
	);
	scenario.name = "Конкурентные транзакции: проверка длинного названия";
	scenario.description =
		"Проверка переноса длинного описания и кириллицы. ".repeat(6);
	scenario.operations[0].sql = `SELECT '${"Длинное значение без пробелов".replaceAll(" ", "").repeat(5)}'`;
	await page.locator("input[type=file]").setInputFiles({
		name: "long.scenario.json",
		mimeType: "application/json",
		buffer: Buffer.from(JSON.stringify(scenario)),
	});
	await expect(
		page.getByText("Historical recording · read only"),
	).toBeVisible();
	await expect(
		page.getByRole("button", { name: "Run A", exact: true }),
	).toBeDisabled();
	await expect(page.locator(".step").first()).toContainText("not recorded");
	await expect(page.locator(".next-note")).toContainText("Read only");
	for (const width of [320, 768, 1440]) {
		await page.setViewportSize({ width, height: 1000 });
		const overflow = await page.evaluate(() =>
			Array.from(document.querySelectorAll("body *"))
				.filter(
					(el) =>
						el.getBoundingClientRect().right > innerWidth + 1 &&
						getComputedStyle(el).position !== "absolute" &&
						!el.closest(".cm-scroller,.table-scroll,pre"),
				)
				.map((el) => ({ tag: el.tagName, class: el.className }))
				.slice(0, 8),
		);
		expect(
			await page.evaluate(
				() => document.documentElement.scrollWidth <= innerWidth,
			),
			JSON.stringify(overflow),
		).toBe(true);
		await expect(
			page.getByRole("button", { name: "Rerun in fresh DB", exact: true }),
		).toBeVisible();
	}
	await page.setViewportSize({ width: 390, height: 844 });
	await page.screenshot({
		path: "../.cache/reports/workbench-long-mobile.png",
		fullPage: true,
	});
});

test("background polling does not overlap a pending state request", async ({
	page,
}) => {
	await page.clock.install();
	await freshSnapshot(page);
	let count = 0;
	let release!: () => void, captured!: () => void;
	const gate = new Promise<void>((resolve) => {
		release = resolve;
	});
	const ready = new Promise<void>((resolve) => {
		captured = resolve;
	});
	await page.route("**/api/state*", async (route) => {
		count++;
		captured();
		await gate;
		await route.continue();
	});
	try {
		await page.clock.runFor(250);
		await ready;
		await page.clock.runFor(1000);
		expect(count).toBe(1);
	} finally {
		release();
		await page.clock.resume();
	}
});

test("replay completion waits for the pending poll before requesting final state", async ({
	page,
}) => {
	await page.clock.install();
	await freshSnapshot(page);
	let releaseReplay!: () => void, replayCaptured!: () => void;
	let releaseState!: () => void, stateCaptured!: () => void;
	const replayGate = new Promise<void>((r) => {
		releaseReplay = r;
	});
	const replayReady = new Promise<void>((r) => {
		replayCaptured = r;
	});
	const stateGate = new Promise<void>((r) => {
		releaseState = r;
	});
	const stateReady = new Promise<void>((r) => {
		stateCaptured = r;
	});
	let count = 0;
	await page.route("**/api/replay", async (route) => {
		const response = await route.fetch();
		replayCaptured();
		await replayGate;
		await route.fulfill({ response });
	});
	try {
		await page
			.getByRole("button", { name: "Run schedule", exact: true })
			.click();
		await replayReady;
		await page.route("**/api/state*", async (route) => {
			count++;
			if (count === 1) {
				const response = await route.fetch();
				stateCaptured();
				await stateGate;
				await route.fulfill({ response });
			} else await route.continue();
		});
		await page.clock.runFor(250);
		await stateReady;
		const response = page.waitForResponse(
			(r) => new URL(r.url()).pathname === "/api/replay",
		);
		releaseReplay();
		await (await response).finished();
		await page.clock.runFor(1000);
		// Drain browser promise callbacks after delivering the command response.
		await page.evaluate(() => Promise.resolve());
		expect(count).toBe(1);
		releaseState();
		await expect(page.getByRole("status")).toContainText(
			"All declared expectations matched",
		);
	} finally {
		releaseReplay();
		releaseState();
	}
});

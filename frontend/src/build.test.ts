import { build } from "vite";
import { expect, it } from "vitest";

it("builds the application with a separate editor chunk", async () => {
	const result = await build({
		configFile: "vite.config.ts",
		logLevel: "silent",
		build: { write: false },
	});
	const outputs = Array.isArray(result) ? result : [result];
	const chunks = outputs.flatMap((output) => {
		if (!("output" in output)) throw new Error("Expected a completed build");
		return output.output.filter((item) => item.type === "chunk");
	});
	const editor = chunks.find((chunk) => chunk.name === "editor");
	expect(editor).toBeDefined();
	expect(editor?.isEntry).toBe(false);
	expect(
		Object.keys(editor?.modules ?? {}).some((id) =>
			/[/\\]node_modules[/\\]@codemirror[/\\]/.test(id),
		),
	).toBe(true);
	expect(chunks.some((chunk) => chunk.isEntry)).toBe(true);
}, 15000);

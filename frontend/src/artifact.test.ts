import { expect, it } from "vitest";
import { artifactBlob, MAX_ARTIFACT } from "./artifact";

it("exports compact JSON without altering typed observations", async () => {
	const data = {
		rows: [
			[
				{ type: "integer", value: "9223372036854775807" },
				{ type: "null" },
				{ type: "blob", value: "00ff" },
			],
		],
	};
	const blob = artifactBlob(data);
	expect(await blob.text()).toBe(JSON.stringify(data));
	expect(JSON.parse(await blob.text())).toEqual(data);
});
it("limits actual UTF-8 bytes, including multibyte strings and JSON overhead", () => {
	expect(artifactBlob({ v: "é".repeat(100) }).size).toBe(
		new TextEncoder().encode(JSON.stringify({ v: "é".repeat(100) })).length,
	);
	expect(() => artifactBlob("é".repeat(MAX_ARTIFACT / 2))).toThrow("8 MiB");
	expect(() => artifactBlob("x".repeat(MAX_ARTIFACT))).toThrow("8 MiB");
});

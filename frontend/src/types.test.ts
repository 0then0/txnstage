import { describe, expect, it } from "vitest";
import {
	cellText,
	type Event,
	type Operation,
	parseParams,
	ready,
} from "./types";

describe("artifact and scheduler presentation", () => {
	it("preserves INTEGER, NULL and BLOB in display and typed parameters", () => {
		expect(cellText({ type: "integer", value: "9223372036854775807" })).toBe(
			"9223372036854775807",
		);
		expect(cellText({ type: "null" })).toBe("NULL");
		expect(cellText({ type: "blob", value: "00ff" })).toBe("x'00ff'");
		expect(
			parseParams('[{"type":"integer","value":"9223372036854775807"}]')[0],
		).toEqual({ type: "integer", value: "9223372036854775807" });
		expect(() => parseParams('[{"type":"integer","value":42}]')).toThrow();
	});
	it("requires observed busy event rather than elapsed time", () => {
		const op: Operation = {
			id: "release",
			connection: "A",
			sql: "COMMIT",
			params: [],
			expect: null,
			note: "",
			after: [{ operation: "writer", event: "busy" }],
		};
		const event: Event = {
			seq: 1,
			elapsed_ms: 99999,
			connection: "B",
			operation: "writer",
			kind: "running",
			data: {},
		};
		expect(ready(op, [event])).toBe(false);
		expect(ready(op, [{ ...event, kind: "busy" }])).toBe(true);
	});
});

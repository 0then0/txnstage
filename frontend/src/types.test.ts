import { describe, expect, it } from "vitest";
import {
	cellText,
	closedInEvents,
	type Event,
	eventLabel,
	lifecycleOf,
	type Operation,
	parseParams,
	ready,
	stepLabel,
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
	it("presents crash and reopen as typed steps and connection states", () => {
		const step = (kind?: Operation["kind"]): Operation => ({
			id: "step",
			connection: "A",
			kind,
			sql: kind && kind !== "sql" ? "" : "SELECT 1",
			params: [],
			after: [],
			expect: null,
			note: "",
		});
		expect(lifecycleOf(step())).toBeNull();
		expect(lifecycleOf(step("sql"))).toBeNull();
		expect(stepLabel(step())).toBe("SELECT 1");
		expect(stepLabel(step("crash"))).toBe("crash worker process");
		expect(stepLabel(step("reopen"))).toBe("reopen same database");
		const event = (
			seq: number,
			connection: string,
			kind: string,
			data: Event["data"],
		): Event => ({
			seq,
			elapsed_ms: 0,
			connection,
			operation: "x",
			kind,
			data,
		});
		const crashed = event(1, "A", "completed", { lifecycle: "crash" });
		const reopened = event(3, "A", "completed", {
			lifecycle: "reopen",
			generation: 2,
		});
		expect(eventLabel(crashed)).toBe("process exited · closed");
		expect(eventLabel(reopened)).toBe("reopened · generation 2");
		expect(
			eventLabel(event(4, "A", "timed_out", { lifecycle: "reopen" })),
		).toBe("reopen timed_out");
		expect(eventLabel(event(5, "A", "completed", {}))).toBe("completed");
		// A recording carries no live state: closed is derived from its events.
		const failedReopen = event(2, "A", "failed", { lifecycle: "reopen" });
		expect(closedInEvents([], "A")).toBe(false);
		expect(closedInEvents([crashed, failedReopen], "A")).toBe(true);
		expect(closedInEvents([crashed, failedReopen], "B")).toBe(false);
		expect(closedInEvents([crashed, failedReopen, reopened], "A")).toBe(false);
	});
});

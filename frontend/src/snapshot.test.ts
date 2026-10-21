import { describe, expect, it } from "vitest";
import { mergeSnapshot, statePath } from "./snapshot";
import type { Event, Snapshot } from "./types";

function snapshot(
	experiment: string,
	offset: number,
	...seq: number[]
): Snapshot {
	return {
		experiment,
		event_offset: offset,
		events: seq.map((seq) => ({ seq }) as Event),
	} as Snapshot;
}

describe("incremental state", () => {
	it("keeps existing results and appends new events once", () => {
		const base = snapshot("one", 0, 1, 2);
		const merged = mergeSnapshot(base, snapshot("one", 2, 3));
		expect(merged.events.map((e) => e.seq)).toEqual([1, 2, 3]);
		expect(merged.events[0]).toBe(base.events[0]);
		expect(mergeSnapshot(merged, snapshot("one", 3)).events).toEqual(
			merged.events,
		);
		expect(statePath(merged)).toBe("state?experiment=one&after=3");
	});
	it("replaces the history when a new experiment reuses sequence numbers", () => {
		const reset = snapshot("two", 0, 1);
		expect(mergeSnapshot(snapshot("one", 0, 1, 2), reset)).toBe(reset);
	});
	it("rejects a delta from another cursor", () => {
		expect(() =>
			mergeSnapshot(snapshot("one", 0, 1), snapshot("two", 1, 2)),
		).toThrow();
		expect(() =>
			mergeSnapshot(snapshot("one", 0, 1), snapshot("one", 2, 3)),
		).toThrow();
	});
});

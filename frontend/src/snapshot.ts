import type { Snapshot } from "./types";

export function statePath(base: Snapshot | null): string {
	if (!base?.experiment) return "state";
	return `state?experiment=${encodeURIComponent(base.experiment)}&after=${base.events.length}`;
}

export function mergeSnapshot(
	base: Snapshot | null,
	update: Snapshot,
): Snapshot {
	if (!update.event_offset) return update;
	if (
		base?.experiment !== update.experiment ||
		base.events.length !== update.event_offset
	)
		throw new Error("State update does not match the requested event history");
	return {
		...update,
		event_offset: 0,
		events: [...base.events, ...update.events],
	};
}

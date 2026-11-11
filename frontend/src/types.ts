export type Cell =
	| { type: "null" }
	| { type: "integer" | "text" | "blob"; value: string }
	| { type: "real"; value: number };
export interface Settings {
	journal_mode: string;
	busy_ms: number;
	deadline_ms: number;
	row_limit: number;
	connections: number;
}
export interface Operation {
	id: string;
	connection: string;
	// Absent for SQL steps. Lifecycle steps carry no SQL, parameters or expectation.
	kind?: "sql" | "crash" | "reopen";
	sql: string;
	params: Cell[];
	after: { operation: string; event: string }[];
	expect: unknown;
	note: string;
}
export interface Scenario {
	format_version: number;
	name: string;
	description: string;
	settings: Settings;
	initial: string[];
	operations: Operation[];
}
export interface Event {
	seq: number;
	elapsed_ms: number;
	connection: string;
	operation: string;
	kind: string;
	data: {
		result?: {
			columns: string[];
			rows: Cell[][];
			row_count: string;
			rows_truncated: boolean;
			cell_previews: boolean;
			statement: string;
		};
		error?: {
			category: string;
			primary?: number;
			extended?: number;
			message: string;
		};
		autocommit?: boolean;
		transaction?: string;
		busy_calls?: number;
		duration_ms?: number;
		[key: string]: unknown;
	};
}
export interface Recording {
	format_version: number;
	scenario: Scenario;
	environment: Record<string, unknown>;
	events: Event[];
}
export interface Snapshot {
	experiment: string;
	event_offset: number;
	scenario: Scenario;
	environment: Record<string, unknown>;
	events: Event[];
	connections?: Record<string, { phase: Phase; generation: number }>;
	active: Record<string, string>;
	lost: string[];
	stopped: boolean;
	operations: Operation[];
	files: { db_bytes: number; wal_bytes: number; elapsed_ms: number };
}
export type Lifecycle = "crash" | "reopen";
export type Phase = "open" | "crashing" | "closed" | "reopening";
export const lifecycleOf = (op: Pick<Operation, "kind">): Lifecycle | null =>
	op.kind === "crash" || op.kind === "reopen" ? op.kind : null;
export function stepLabel(op: Operation): string {
	const kind = lifecycleOf(op);
	if (kind === "crash") return "crash worker process";
	return kind === "reopen" ? "reopen same database" : op.sql;
}
export function eventLabel(e: Event): string {
	if (e.kind === "busy") return "busy observed";
	const labels: Record<string, Record<string, string>> = {
		crash: {
			created: "crash requested",
			running: "kill sent",
			completed: "process exited · closed",
		},
		reopen: {
			created: "reopen requested",
			running: "worker started",
			completed: `reopened · generation ${e.data.generation}`,
		},
	};
	const lifecycle = String(e.data.lifecycle ?? "");
	if (!labels[lifecycle]) return e.kind;
	return labels[lifecycle][e.kind] ?? `${lifecycle} ${e.kind}`;
}
// Recordings have no live coordinator state: a connection is closed from a
// completed crash until a completed reopen.
export function closedInEvents(events: Event[], connection: string): boolean {
	for (let i = events.length - 1; i >= 0; i--) {
		const e = events[i];
		if (
			e.connection === connection &&
			e.kind === "completed" &&
			e.data.lifecycle
		)
			return e.data.lifecycle === "crash";
	}
	return false;
}
export const terminal = (kind: string) =>
	["completed", "failed", "cancelled", "timed_out"].includes(kind);
export function cellText(cell: Cell): string {
	if (cell.type === "null") return "NULL";
	if (cell.type === "blob") return `x'${cell.value}'`;
	return String(cell.value);
}
export function ready(op: Operation, events: Event[]) {
	return op.after.every((d) =>
		events.some((e) => e.operation === d.operation && e.kind === d.event),
	);
}
export function parseParams(text: string): Cell[] {
	const v: unknown = JSON.parse(text);
	if (
		!Array.isArray(v) ||
		v.some(
			(x) =>
				!x ||
				typeof x !== "object" ||
				!["null", "integer", "real", "text", "blob"].includes(x.type) ||
				(x.type === "real"
					? typeof x.value !== "number" || !Number.isFinite(x.value)
					: x.type !== "null" && typeof x.value !== "string"),
		)
	)
		throw new Error(
			'Use typed parameters, e.g. [{"type":"integer","value":"42"}]',
		);
	return v as Cell[];
}

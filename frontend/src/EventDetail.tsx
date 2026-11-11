import { ResultTable } from "./ResultTable";
import { type Event, eventLabel } from "./types";

export function EventDetail({
	event,
	events,
}: {
	event: Event;
	events: Event[];
}) {
	const created = events.find(
		(e) => e.operation === event.operation && e.kind === "created",
	);
	const sql =
		typeof event.data.sql === "string" ? event.data.sql : created?.data.sql;
	const termination = event.data.termination as
		| { method?: string; exit_confirmed?: boolean }
		| undefined;
	const session = event.data.session_state as
		| string
		| {
				note?: string;
				last_sampled?: { transaction?: string; autocommit?: boolean } | null;
		  }
		| undefined;
	const sampled = typeof session === "object" ? session.last_sampled : null;
	// Coordinator descriptions of a planned crash or reopen, shown verbatim.
	const notes = [
		typeof session === "string" ? session : session?.note,
		event.data.database,
		event.data.settings_reapplied,
	].filter((v): v is string => typeof v === "string");
	const facts: [string, unknown][] = [
		["Lifecycle", event.data.lifecycle],
		["Worker generation", event.data.generation],
		[
			"Termination",
			termination?.method &&
				`${termination.method}${termination.exit_confirmed ? " · exit confirmed, process reaped" : ""}`,
		],
		[
			"Sampled before crash",
			sampled &&
				`transaction ${sampled.transaction}, autocommit ${sampled.autocommit ? "on" : "off"}`,
		],
		[
			"Duration",
			event.data.duration_ms === undefined
				? undefined
				: `${event.data.duration_ms} ms`,
		],
		["Transaction", event.data.transaction],
		[
			"Autocommit",
			event.data.autocommit === undefined
				? undefined
				: event.data.autocommit
					? "on"
					: "off",
		],
		["Busy callbacks", event.data.busy_calls],
	];
	return (
		<>
			<div className="detail-identity">
				<span className={`step-conn conn-${event.connection}`}>
					{event.connection}
				</span>
				<strong>{event.operation}</strong>
				<span className={`event-kind kind-${event.kind}`}>
					{eventLabel(event)}
				</span>
			</div>
			<p className="detail-time">
				Event #{event.seq} · observed at +{event.elapsed_ms} ms
			</p>
			{typeof sql === "string" && (
				<figure className="recorded-sql">
					<figcaption>Recorded SQL</figcaption>
					<pre className="detail-sql">{sql}</pre>
				</figure>
			)}
			{event.data.error && (
				<div className="detail-error">
					<strong>
						{event.data.error.category === "sql"
							? "SQL error"
							: event.data.error.category}
						{event.data.error.extended !== undefined
							? ` · ${event.data.error.extended}`
							: ""}
					</strong>
					<p>{event.data.error.message}</p>
					{event.data.error.primary !== undefined && (
						<small>
							Primary code {event.data.error.primary} · extended code{" "}
							{event.data.error.extended ?? "unavailable"}
						</small>
					)}
				</div>
			)}
			<dl className="event-facts">
				{facts.map(
					([label, value]) =>
						value !== undefined &&
						value !== null && (
							<div key={label}>
								<dt>{label}</dt>
								<dd>{String(value)}</dd>
							</div>
						),
				)}
			</dl>
			{notes.map((note) => (
				<p className="detail-note" key={note}>
					{note}
				</p>
			))}
			{event.data.result && event.data.result.columns.length > 0 && (
				<div className="detail-result">
					<ResultTable key={event.seq} event={event} />
					<p className="result-foot">
						{event.data.result.row_count} rows
						{event.data.result.rows_truncated ? " · row preview limited" : ""}
						{event.data.result.cell_previews ? " · cell previews limited" : ""}
					</p>
				</div>
			)}
			<details className="raw-event">
				<summary>Raw event JSON</summary>
				<pre>{JSON.stringify(event.data, null, 2)}</pre>
			</details>
		</>
	);
}

import type { Event } from "./types";

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
	const facts = [
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
				<span className={`event-kind kind-${event.kind}`}>{event.kind}</span>
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
						value !== undefined && (
							<div key={label}>
								<dt>{label}</dt>
								<dd>{String(value)}</dd>
							</div>
						),
				)}
			</dl>
			<details className="raw-event">
				<summary>Raw event JSON</summary>
				<pre>{JSON.stringify(event.data, null, 2)}</pre>
			</details>
		</>
	);
}

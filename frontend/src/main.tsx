import React, { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { artifactBlob, MAX_ARTIFACT } from "./artifact";
import { EventDetail } from "./EventDetail";
import { ResultTable } from "./ResultTable";
import { SqlEditor } from "./SqlEditor";
import { mergeSnapshot, statePath } from "./snapshot";
import {
	type Event,
	type Operation,
	parseParams,
	type Recording,
	ready,
	type Scenario,
	type Snapshot,
	terminal,
} from "./types";
import "./style.css";

const token = new URLSearchParams(location.hash.slice(1)).get("token");
if (token) {
	sessionStorage.setItem("txnstage-token", token);
	history.replaceState(null, "", location.pathname);
}
async function api<T>(path: string, body?: unknown): Promise<T> {
	const r = await fetch(`/api/${path}`, {
		method: body === undefined ? "GET" : "POST",
		headers: {
			"content-type": "application/json",
			"x-txnstage-token": sessionStorage.getItem("txnstage-token") ?? "",
		},
		body: body === undefined ? undefined : JSON.stringify(body),
	});
	if (!r.ok) throw new Error(await r.text());
	return r.json() as Promise<T>;
}
function download(name: string, data: unknown) {
	const url = URL.createObjectURL(artifactBlob(data));
	const a = document.createElement("a");
	a.href = url;
	a.download = name;
	a.click();
	setTimeout(() => URL.revokeObjectURL(url), 1000);
}
type Conn = "A" | "B" | "C";
const connections: Conn[] = ["A", "B", "C"];
function editorValues(s: Scenario, used: string[] = []) {
	const text = { A: "", B: "", C: "" },
		ps = { A: "[]", B: "[]", C: "[]" };
	const selectedIds: Record<string, string> = {};
	for (const c of connections) {
		const o = s.operations.find(
			(o) => o.connection === c && !used.includes(o.id),
		);
		text[c] = o?.sql ?? "SELECT v FROM t WHERE id = 1";
		ps[c] = JSON.stringify(o?.params ?? []);
		selectedIds[c] = o?.id ?? "";
	}
	return { text, ps, selectedIds };
}
function App() {
	const [state, setState] = useState<Snapshot | null>(null),
		[draft, setDraft] = useState<Scenario | null>(null),
		[presets, setPresets] = useState<Scenario[]>([]),
		[sql, setSql] = useState({ A: "", B: "", C: "" }),
		[params, setParams] = useState({ A: "[]", B: "[]", C: "[]" }),
		[ids, setIds] = useState<Record<string, string>>({}),
		[selected, setSelected] = useState<Event | null>(null),
		[eventConnection, setEventConnection] = useState("all"),
		[recording, setRecording] = useState<Recording | null>(null),
		[differences, setDifferences] = useState<unknown[]>([]),
		[error, setError] = useState(""),
		[notice, setNotice] = useState(""),
		[pending, setPending] = useState(false),
		[settingsOpen, setSettingsOpen] = useState(false),
		[dirty, setDirty] = useState(false);
	const file = useRef<HTMLInputElement>(null),
		generation = useRef(0),
		alive = useRef(true);
	const latestSnapshot = useRef<Snapshot | null>(null);
	const refreshSequence = useRef(0),
		appliedRefresh = useRef(0);
	const refreshQueue = useRef<{
		gen: number;
		done: Promise<Snapshot | null>;
	} | null>(null);
	const refresh = useCallback(() => {
		const gen = generation.current;
		// A new experiment must not wait for a stale request from the previous one.
		const previous =
			refreshQueue.current?.gen === gen
				? refreshQueue.current.done
				: Promise.resolve();
		const done = previous
			.catch(() => null)
			.then(async () => {
				if (!alive.current || gen !== generation.current) return null;
				const sequence = ++refreshSequence.current;
				try {
					const base = latestSnapshot.current;
					const update = await api<Snapshot>(statePath(base));
					const v = mergeSnapshot(base, update);
					if (
						alive.current &&
						gen === generation.current &&
						sequence > appliedRefresh.current
					) {
						appliedRefresh.current = sequence;
						latestSnapshot.current = v;
						setState(v);
						return v;
					}
				} catch (error) {
					if (
						alive.current &&
						gen === generation.current &&
						sequence > appliedRefresh.current
					)
						throw error;
				}
				return null;
			});
		refreshQueue.current = { gen, done };
		return done;
	}, []);
	function current(gen: number) {
		return alive.current && gen === generation.current;
	}

	useEffect(() => {
		alive.current = true;
		let active = true;
		api<Scenario[]>("scenarios")
			.then((v) => {
				if (active) setPresets(v);
			})
			.catch((e) => setError(String(e)));
		api<Snapshot>("state")
			.then((v) => {
				if (active) {
					latestSnapshot.current = v;
					setState(v);
					setDraft(v.scenario);
					const loaded = editorValues(v.scenario);
					setSql(loaded.text);
					setParams(loaded.ps);
					setIds(loaded.selectedIds);
				}
			})
			.catch((e) => setError(String(e)));
		let refreshing = false;
		const timer = setInterval(() => {
			if (refreshing) return;
			refreshing = true;
			refresh()
				.catch((e) => {
					if (active) setError(String(e));
				})
				.finally(() => {
					refreshing = false;
				});
		}, 200);
		return () => {
			active = false;
			alive.current = false;
			clearInterval(timer);
		};
	}, [refresh]);
	function loadEditors(s: Scenario, used: string[] = []) {
		const v = editorValues(s, used);
		setSql(v.text);
		setParams(v.ps);
		setIds(v.selectedIds);
	}

	async function action(fn: () => Promise<void>) {
		setError("");
		let gen = generation.current;
		try {
			const result = fn();
			// reset/replay/import enter a new generation synchronously before awaiting I/O.
			gen = generation.current;
			await result;
		} catch (e) {
			if (current(gen)) setError(e instanceof Error ? e.message : String(e));
		}
	}
	async function reset(s: Scenario) {
		const gen = ++generation.current;
		setPending(true);
		setNotice("");
		try {
			await api("reset", s);
			if (!current(gen)) return;
			setDraft(s);
			setRecording(null);
			setDifferences([]);
			setSelected(null);
			setDirty(false);
			loadEditors(s);
			await refresh();
			if (!current(gen)) return;
			setNotice("Fresh disposable database. No scenario steps have run.");
		} finally {
			if (current(gen)) setPending(false);
		}
	}
	async function replay() {
		if (!draft) return;
		const gen = ++generation.current;
		setPending(true);
		setNotice("");
		setRecording(null);
		setSelected(null);
		setDirty(false);
		loadEditors(draft);
		try {
			await api("replay", draft);
			if (!current(gen)) return;
			await refresh();
			if (!current(gen)) return;
			loadEditors(
				draft,
				draft.operations.map((o) => o.id),
			);
			setNotice("Schedule finished. All declared expectations matched.");
		} finally {
			if (current(gen)) setPending(false);
		}
	}
	async function stop() {
		const gen = ++generation.current;
		setNotice("");
		try {
			await api("cancel/all", {});
			if (!current(gen)) return;
			await refresh();
			if (!current(gen)) return;
			setNotice("Experiment stopped. Workers terminated. Reset to continue.");
		} finally {
			if (current(gen)) setPending(false);
		}
	}
	async function cancelConnection(c: Conn) {
		const gen = generation.current;
		await api(`cancel/${c}`, {});
		if (current(gen)) await refresh();
	}

	async function execute(c: Conn, supplied?: Operation) {
		if (!state || !draft || recording) return;
		const gen = generation.current;
		const original = supplied ?? draft.operations.find((o) => o.id === ids[c]);
		const text = supplied?.sql ?? sql[c],
			bound = supplied?.params ?? parseParams(params[c]);
		const op: Operation = {
			id:
				original && !state.operations.some((o) => o.id === original.id)
					? original.id
					: `manual_${crypto.randomUUID()}`,
			connection: c,
			sql: text,
			params: bound,
			after: [],
			expect:
				original?.sql === text &&
				JSON.stringify(original.params) === JSON.stringify(bound)
					? original.expect
					: null,
			note: original?.note ?? "Manual operation",
		};
		await api("execute", op);
		if (!current(gen)) return;
		await refresh();
		if (!current(gen)) return;
		const used = [...state.operations.map((o) => o.id), op.id];
		const next = draft.operations.find(
			(o) => o.connection === c && !used.includes(o.id),
		);
		setIds((v) => ({ ...v, [c]: next?.id ?? "" }));
		setSql((v) => ({ ...v, [c]: next?.sql ?? text }));
		setParams((v) => ({ ...v, [c]: JSON.stringify(next?.params ?? []) }));
	}
	async function importFile(f: File) {
		if (f.size > MAX_ARTIFACT) throw new Error("Artifact limit is 8 MiB");
		const gen = ++generation.current;
		setPending(true);
		try {
			const v: unknown = JSON.parse(await f.text());
			if (!current(gen)) return;
			if (!v || typeof v !== "object") throw new Error("Invalid artifact");
			if ("scenario" in v) {
				const result = await api<{
					recording: Recording;
					differences: unknown[];
				}>("inspect", v);
				if (!current(gen)) return;
				setRecording(result.recording);
				setDraft(result.recording.scenario);
				setDifferences(result.differences);
				loadEditors(result.recording.scenario);
			} else {
				const s = await api<Scenario>("validate", v);
				if (!current(gen)) return;
				setDraft(s);
				setRecording({
					format_version: 1,
					scenario: s,
					environment: {},
					events: [],
				});
				setDifferences([]);
				loadEditors(s);
			}
			setSelected(null);
			setDirty(false);
			setNotice(
				"Artifact opened for viewing. No SQL executed. Rerun explicitly creates a fresh database.",
			);
		} finally {
			if (current(gen)) setPending(false);
		}
	}

	if (!state || !draft)
		return (
			<main className="boot">
				<img src="/icon.svg" alt="" width="50" />
				<h1>TxnStage</h1>
				<p>{error || "Starting isolated SQLite workers…"}</p>
				<p>Use the full launch URL printed by the CLI, including its token.</p>
			</main>
		);
	const events = recording?.events ?? state.events,
		visible = events.filter(
			(e) => e.operation && !e.operation.startsWith("seed_"),
		),
		used = new Set(recording ? [] : state.operations.map((o) => o.id)),
		next = draft.operations.find((o) => !used.has(o.id) && ready(o, events)),
		finished = draft.operations.filter(
			(o) =>
				used.has(o.id) &&
				events.some((e) => e.operation === o.id && terminal(e.kind)),
		).length,
		activeCount = Object.keys(state.active).length,
		env = recording?.environment ?? state.environment;
	function selectOp(o: Operation) {
		const c = o.connection as Conn;
		setIds((v) => ({ ...v, [c]: o.id }));
		setSql((v) => ({ ...v, [c]: o.sql }));
		setParams((v) => ({ ...v, [c]: JSON.stringify(o.params) }));
	}
	function editSql(c: Conn, text: string) {
		setSql((v) => ({ ...v, [c]: text }));
		if (ids[c])
			setDraft((s) =>
				s
					? {
							...s,
							operations: s.operations.map((o) =>
								o.id === ids[c] ? { ...o, sql: text, expect: null } : o,
							),
						}
					: s,
			);
		setDirty(true);
	}
	const filteredEvents = visible.filter(
		(e) => eventConnection === "all" || e.connection === eventConnection,
	);
	const link = draft.description.match(/https:\/\/\S+/)?.[0];
	return (
		<>
			<a className="skip-link" href="#workbench">
				Skip to workbench
			</a>
			<header className="topbar">
				<a className="brand" href="/">
					<img src="/icon.svg" width="32" height="32" alt="" />
					TxnStage <span>v0.1</span>
				</a>
				<div className="top-subtitle">SQLITE CONCURRENCY LAB</div>
				<div className="local">
					<i />
					LOCAL · DISPOSABLE
				</div>
				<button
					type="button"
					className="header-button"
					aria-expanded={settingsOpen}
					aria-controls="experiment-settings"
					onClick={() => setSettingsOpen((v) => !v)}
				>
					Environment & settings
				</button>
			</header>
			<div className="workspace">
				<aside className="sidebar">
					<div className="eyebrow">EXPERIMENT LIBRARY</div>
					<h2>Experiments</h2>
					<p className="muted">
						Choose a schedule, step through SQL and inspect what SQLite reports.
					</p>
					<label className="mobile-experiment">
						Experiment
						<select
							aria-label="Experiment"
							value={
								presets.some((p) => p.name === draft.name) ? draft.name : ""
							}
							disabled={pending}
							onChange={(e) => {
								const preset = presets.find((p) => p.name === e.target.value);
								if (preset) action(() => reset(preset));
							}}
						>
							<option value="" disabled>
								Imported scenario
							</option>
							{presets.map((p, i) => (
								<option key={p.name} value={p.name}>
									{String(i + 1).padStart(2, "0")} · {p.name}
								</option>
							))}
						</select>
					</label>
					<div className="scenario-list">
						{presets.map((p, i) => (
							<button
								type="button"
								className={`scenario-button ${p.name === draft.name ? "chosen" : ""}`}
								aria-current={p.name === draft.name ? "true" : undefined}
								key={p.name}
								disabled={pending}
								onClick={() => action(() => reset(p))}
							>
								<span className="scenario-number">
									{String(i + 1).padStart(2, "0")}
								</span>
								<span>
									{p.name}
									<small>
										{i === 0
											? "Snapshot isolation"
											: i < 3
												? "Writer contention"
												: i < 5
													? "Transaction scheduling"
													: "WAL maintenance"}
									</small>
								</span>
								<span className="arrow">↗</span>
							</button>
						))}
					</div>
					<div className="sidebar-bottom">
						<p>FACTS, THEN EXPLANATIONS</p>
						<span>
							Native SQLite. One process per connection. No simulated locks.
						</span>
						<a
							href="https://www.sqlite.org/isolation.html"
							target="_blank"
							rel="noreferrer"
						>
							SQLite isolation reference ↗
						</a>
					</div>
				</aside>
				<main className="main" id="workbench" tabIndex={-1}>
					<section className="experiment-heading">
						<div>
							<div className="eyebrow">
								WORKBENCH / {recording ? "RECORDED RUN" : "LIVE EXPERIMENT"}
							</div>
							<h1>{draft.name}</h1>
							<p>
								{draft.description.replace(/https:\/\/\S+/g, "")}{" "}
								{link && (
									<a href={link} target="_blank" rel="noreferrer">
										Reference ↗
									</a>
								)}
							</p>
						</div>
						<div className="artifact-actions">
							<button
								type="button"
								disabled={pending || activeCount > 0}
								onClick={() => file.current?.click()}
							>
								Import
							</button>
							<button
								type="button"
								onClick={() =>
									action(async () => {
										download(
											"txnstage.recording.json",
											recording ?? (await api("recording")),
										);
									})
								}
							>
								Export run
							</button>
							<button
								type="button"
								onClick={() =>
									action(async () => {
										download("txnstage.scenario.json", draft);
									})
								}
							>
								Export scenario
							</button>
							<input
								ref={file}
								type="file"
								accept=".json,application/json"
								hidden
								onChange={(e) => {
									const f = e.target.files?.[0];
									if (f) action(() => importFile(f));
									e.target.value = "";
								}}
							/>
						</div>
					</section>
					{error && (
						<div role="alert" className="banner error">
							<strong>Action failed</strong> {error}
							<button
								type="button"
								aria-label="Dismiss error"
								onClick={() => setError("")}
							>
								×
							</button>
						</div>
					)}
					{notice && (
						<div role="status" className="banner">
							{notice}
							<button
								type="button"
								aria-label="Dismiss notice"
								onClick={() => setNotice("")}
							>
								×
							</button>
						</div>
					)}
					{recording && (
						<div className="banner historical">
							<strong>Historical recording · read only</strong> Import never
							executes SQL.{" "}
							{differences.length > 0 && (
								<details>
									<summary>
										{differences.length} environment differences
									</summary>
									<pre>{JSON.stringify(differences, null, 2)}</pre>
								</details>
							)}
						</div>
					)}
					{dirty && (
						<div className="banner historical">
							Draft changed. Earlier observations belong to the SQL and settings
							in event details. Apply settings with Reset or Rerun.
						</div>
					)}
					<section className="control-strip" aria-label="Experiment controls">
						<div className="schedule-meta">
							<span className="dot" />
							{recording
								? "Viewing artifact"
								: state.stopped
									? "Stopped · reset to continue"
									: `${finished} / ${draft.operations.length} steps observed`}
							<span className="divider" />
							{draft.settings.journal_mode.toUpperCase()}
							<span className="divider" />
							retry {draft.settings.busy_ms} ms
						</div>
						<div className="controls">
							<button
								type="button"
								disabled={pending || !!recording || !next || state.stopped}
								className="primary"
								aria-describedby="next-step-context"
								onClick={() =>
									next && action(() => execute(next.connection as Conn, next))
								}
							>
								▶ Next ready step
							</button>
							<button
								type="button"
								disabled={pending}
								onClick={() => action(replay)}
							>
								{pending
									? "Schedule running…"
									: recording
										? "Rerun in fresh DB"
										: "Run schedule"}
							</button>
							<button type="button" onClick={() => action(() => reset(draft))}>
								↻ Reset
							</button>
							<button
								type="button"
								className="danger"
								disabled={!!recording || state.stopped}
								onClick={() => action(stop)}
							>
								Stop
							</button>
						</div>
						<div className="next-step-context" id="next-step-context">
							<div className="progress-copy">
								<span>
									{recording ? "RECORDED SCHEDULE" : "SCHEDULE PROGRESS"}
								</span>
								{!recording && (
									<progress
										aria-label="Observed schedule steps"
										max={Math.max(1, draft.operations.length)}
										value={finished}
									/>
								)}
							</div>
							{recording ? (
								<p>
									Read-only observations. Rerun in a fresh database to execute
									this schedule.
								</p>
							) : state.stopped ? (
								<p>Workers stopped. Reset to start a fresh experiment.</p>
							) : next ? (
								<div className="next-step-preview">
									<span className={`step-conn conn-${next.connection}`}>
										{next.connection}
									</span>
									<strong>Next ready</strong>
									<code>{next.sql}</code>
									<span>{next.note || next.id}</span>
								</div>
							) : (
								<p>
									{activeCount
										? "Waiting for an observed event. Other connections remain available."
										: "All steps observed. Select a step to edit SQL, or Reset to start again."}
								</p>
							)}
						</div>
					</section>
					{settingsOpen && (
						<section
							className="settings"
							id="experiment-settings"
							aria-label="Environment and settings"
						>
							<div>
								<h2>Experiment settings</h2>
								<p>
									Changes apply after Reset or Rerun. Autocheckpoint is off.
								</p>
								<div className="setting-grid">
									{(
										[
											"busy_ms",
											"deadline_ms",
											"row_limit",
											"connections",
										] as const
									).map((key) => (
										<label key={key}>
											{
												{
													busy_ms: "Busy retry budget (ms)",
													deadline_ms: "Operation deadline (ms)",
													row_limit: "Displayed rows",
													connections: "Connections",
												}[key]
											}
											<input
												type="number"
												value={draft.settings[key]}
												onChange={(e) => {
													setDraft({
														...draft,
														settings: {
															...draft.settings,
															[key]: Number(e.target.value),
														},
													});
													setDirty(true);
												}}
											/>
										</label>
									))}
									<label>
										Journal mode
										<select
											value={draft.settings.journal_mode}
											onChange={(e) => {
												setDraft({
													...draft,
													settings: {
														...draft.settings,
														journal_mode: e.target.value,
													},
												});
												setDirty(true);
											}}
										>
											<option value="wal">WAL</option>
											<option value="delete">DELETE</option>
										</select>
									</label>
								</div>
								<details>
									<summary>
										Initial schema & seed (one statement per entry)
									</summary>
									<textarea
										aria-label="Initial statements JSON"
										defaultValue={JSON.stringify(draft.initial, null, 2)}
										onBlur={(e) =>
											action(async () => {
												const v: unknown = JSON.parse(e.target.value);
												if (
													!Array.isArray(v) ||
													!v.every((x) => typeof x === "string")
												)
													throw new Error(
														"Initial SQL must be an array of statements",
													);
												setDraft({ ...draft, initial: v });
												setDirty(true);
											})
										}
									/>
								</details>
							</div>
							<div>
								<h2>Observed environment</h2>
								<p>
									SQLite {String(env.sqlite_version ?? "not recorded")} ·{" "}
									{String(env.os ?? "")} / {String(env.arch ?? "")}
								</p>
								<details>
									<summary>Source ID, compile options & policy</summary>
									<pre>{JSON.stringify(env, null, 2)}</pre>
								</details>
								<p>
									Filesystem sample at {state.files.elapsed_ms} ms: .db{" "}
									{state.files.db_bytes.toLocaleString()} B · WAL{" "}
									{state.files.wal_bytes.toLocaleString()} B. Bytes are not
									frame counts.
								</p>
							</div>
						</section>
					)}
					<div className="section-title workspace-title">
						<h2>Connections</h2>
						<span>Independent processes · shared disposable database</span>
					</div>
					<section className="stage" aria-label="Connections">
						{connections.slice(0, draft.settings.connections).map((c) => {
							const runningId = recording ? undefined : state.active[c],
								last = [...events]
									.reverse()
									.find(
										(e) =>
											e.connection === c &&
											terminal(e.kind) &&
											!e.operation.startsWith("seed_"),
									),
								isBusy =
									runningId &&
									events.some(
										(e) => e.operation === runningId && e.kind === "busy",
									),
								status =
									!recording && state.lost.includes(c)
										? "state lost"
										: runningId
											? isBusy
												? "busy observed"
												: "running"
											: last?.kind === "failed"
												? "error"
												: (last?.kind ?? "ready"),
								tx = [...events]
									.reverse()
									.find((e) => e.connection === c && e.data.transaction);
							return (
								<article
									className="connection"
									key={c}
									aria-label={`Connection ${c}`}
								>
									<div className="connection-head">
										<div className={`connection-id conn-${c}`}>{c}</div>
										<div>
											<h2>Connection {c}</h2>
											<span className="worker-label">
												Independent worker process
											</span>
										</div>
										<span
											className={`status status-${status.replaceAll(" ", "-")}`}
										>
											{status}
										</span>
									</div>
									<div className="transaction">
										<span>TRANSACTION</span>
										<strong>{tx?.data.transaction ?? "none"}</strong>
										<span>
											autocommit {tx?.data.autocommit === false ? "off" : "on"}{" "}
											· last statement
										</span>
									</div>
									<div className="editor-caption">
										<label htmlFor={`step-${c}`}>SQL / one statement</label>
										<select
											id={`step-${c}`}
											aria-label={`Step for connection ${c}`}
											value={ids[c] ?? ""}
											onChange={(e) => {
												const o = draft.operations.find(
													(o) => o.id === e.target.value,
												);
												if (o) selectOp(o);
												else setIds((v) => ({ ...v, [c]: "" }));
											}}
										>
											<option value="">Custom SQL</option>
											{draft.operations
												.filter((o) => o.connection === c)
												.map((o) => (
													<option key={o.id} value={o.id}>
														{o.id}
														{used.has(o.id) ? " · observed" : ""}
													</option>
												))}
										</select>
									</div>
									<SqlEditor
										label={`SQL connection ${c}`}
										value={sql[c]}
										onChange={(v) => editSql(c, v)}
										onRun={() => {
											if (!runningId && !recording && !pending)
												action(() => execute(c));
										}}
									/>
									<details className="params">
										<summary>
											Typed parameters{" "}
											<code>{params[c] === "[]" ? "none" : "JSON"}</code>
										</summary>
										<textarea
											aria-label={`Parameters connection ${c}`}
											value={params[c]}
											spellCheck={false}
											onChange={(e) => {
												setParams((v) => ({ ...v, [c]: e.target.value }));
												try {
													const parsed = parseParams(e.target.value);
													setDraft((s) =>
														s
															? {
																	...s,
																	operations: s.operations.map((o) =>
																		o.id === ids[c]
																			? { ...o, params: parsed, expect: null }
																			: o,
																	),
																}
															: s,
													);
												} catch {
													/* Validated before execute. */
												}
												setDirty(true);
											}}
										/>
										<small>INTEGER: decimal string. BLOB: hex.</small>
									</details>
									<div className="run-row">
										<span>{runningId ?? "⌘ / Ctrl + Enter"}</span>
										{runningId ? (
											<button
												type="button"
												className="danger"
												onClick={() => action(() => cancelConnection(c))}
											>
												Cancel {c}
											</button>
										) : (
											<button
												type="button"
												disabled={
													!!recording ||
													state.stopped ||
													pending ||
													state.lost.includes(c)
												}
												aria-label={`Run ${c}`}
												onClick={() => action(() => execute(c))}
											>
												▶ Run {c}
											</button>
										)}
									</div>
									<div className="result">
										<div className="result-heading">
											<strong>LAST OBSERVATION</strong>
											<span>
												{last?.operation ?? "No operation yet"}
												{last?.data.duration_ms !== undefined
													? ` · ${last.data.duration_ms} ms`
													: ""}
											</span>
										</div>
										{last?.data.error ? (
											<div className="sql-error">
												<strong>
													{last.kind === "failed" ? "SQL error" : last.kind}{" "}
													{last.data.error.extended !== undefined
														? `· ${last.data.error.extended}`
														: ""}
												</strong>
												<p>{last.data.error.message}</p>
												<small>
													primary {last.data.error.primary ?? "n/a"} · extended{" "}
													{last.data.error.extended ?? "n/a"} ·{" "}
													{last.data.busy_calls ?? 0} busy callbacks
												</small>
											</div>
										) : last?.data.result ? (
											<>
												<ResultTable
													key={`${generation.current}-${last.seq}`}
													event={last}
												/>
												<p className="result-foot">
													{last.data.result.row_count} rows ·{" "}
													{last.data.busy_calls} busy callbacks
													<br />
													{last.data.result.statement}
													{last.data.result.rows_truncated
														? " · row preview limited"
														: ""}
													{last.data.result.cell_previews
														? " · cell preview limited"
														: ""}
												</p>
											</>
										) : (
											<p className="empty-result">
												Run a statement to observe its result.
												<br />
												Every operation uses native SQLite.
											</p>
										)}
									</div>
								</article>
							);
						})}
					</section>
					<section className="runbook">
						<div className="section-title">
							<h2>Schedule</h2>
							<span>
								User order & event dependencies · select a step to load SQL
							</span>
						</div>
						<div className="step-list">
							{draft.operations.map((o, i) => {
								const ev = [...events]
										.reverse()
										.find((e) => e.operation === o.id && terminal(e.kind)),
									expected =
										!!ev &&
										!!o.expect &&
										typeof o.expect === "object" &&
										"extended" in o.expect &&
										o.expect.extended === ev.data.error?.extended,
									runnable = !recording && !used.has(o.id) && ready(o, events);
								const historical =
									!recording &&
									state.operations.some(
										(p) =>
											p.id === o.id &&
											(p.sql !== o.sql ||
												JSON.stringify(p.params) !== JSON.stringify(o.params)),
									);

								return (
									<button
										type="button"
										key={o.id}
										className={`step ${used.has(o.id) ? "observed" : runnable ? "next" : ""}`}
										aria-pressed={ids[o.connection] === o.id}
										onClick={() => selectOp(o)}
										title={`${o.note}\n${o.after.map((d) => `after ${d.operation}/${d.event}`).join(", ")}`}
									>
										<span className="step-num">
											{String(i + 1).padStart(2, "0")}
										</span>
										<span className={`step-conn conn-${o.connection}`}>
											{o.connection}
										</span>
										<code>{o.sql}</code>
										<span className="step-state">
											{historical
												? "historical observation"
												: expected
													? "expected SQL error"
													: (ev?.kind ??
														(recording
															? "not recorded"
															: runnable
																? "ready"
																: "await event"))}
										</span>
									</button>
								);
							})}
						</div>
						<p className="next-note">
							{recording
								? "Read only. Select a step to inspect SQL, or rerun in a fresh database."
								: next
									? `Next ready: ${next.connection} · ${next.note}`
									: activeCount
										? "An operation is in flight. Other connections remain available."
										: "All steps observed. Edit SQL to explore, or reset for a fresh database."}
						</p>
					</section>
					<section className="observations" aria-label="Observed events">
						<div className="timeline">
							<div className="section-title">
								<h2>
									Event timeline <span>{visible.length}</span>
								</h2>
								<span>Coordinator observation order</span>
							</div>
							<div className="timeline-tools">
								<label>
									Connection{" "}
									<select
										aria-label="Connection"
										value={eventConnection}
										onChange={(e) => setEventConnection(e.target.value)}
									>
										<option value="all">All connections</option>
										{connections
											.slice(0, draft.settings.connections)
											.map((c) => (
												<option key={c} value={c}>
													{c}
												</option>
											))}
									</select>
								</label>
								<span>
									{filteredEvents.length} of {visible.length} events
								</span>
							</div>
							<div
								className="timeline-scroll"
								role="log"
								aria-label="Event timeline"
							>
								{filteredEvents.length === 0 ? (
									<div className="timeline-empty">
										<span>○</span>
										<p>
											{visible.length
												? "No events for this connection."
												: "Nothing observed yet."}
										</p>
										<small>
											{visible.length
												? "Choose All connections to see the full timeline."
												: "Execute the first step. Events appear as workers report them."}
										</small>
									</div>
								) : (
									filteredEvents.map((e) => (
										<button
											type="button"
											key={e.seq}
											className={`event ${selected?.seq === e.seq ? "selected" : ""}`}
											aria-pressed={selected?.seq === e.seq}
											onClick={() => setSelected(e)}
										>
											<span className="event-seq">
												{String(e.seq).padStart(3, "0")}
											</span>
											<span className="event-time">+{e.elapsed_ms} ms</span>
											<span className={`event-conn conn-${e.connection}`}>
												{e.connection}
											</span>
											<code>{e.operation}</code>
											<span className={`event-kind kind-${e.kind}`}>
												{e.kind === "busy" ? "busy observed" : e.kind}
											</span>
										</button>
									))
								)}
							</div>
						</div>
						<div className="event-detail">
							<div className="section-title">
								<h2>Event detail</h2>
								<span>Observed fact</span>
							</div>
							{selected ? (
								<EventDetail event={selected} events={events} />
							) : (
								<div className="detail-empty">
									Select an event to inspect its SQL, typed results, error codes
									and transaction state.
									<p>
										Busy means the callback ran. It does not identify a lock
										owner.
									</p>
								</div>
							)}
						</div>
					</section>
					<footer>
						TxnStage / real SQLite, explicit schedules
						<span>Durations vary. Internal lock states are not inferred.</span>
					</footer>
				</main>
			</div>
		</>
	);
}
const root = document.getElementById("root");
if (root)
	createRoot(root).render(
		<React.StrictMode>
			<App />
		</React.StrictMode>,
	);

import { memo, useState } from "react";
import { cellText, type Event } from "./types";

const PAGE_SIZE = 20;

export const ResultTable = memo(function ResultTable({
	event,
}: {
	event: Event;
}) {
	const [page, setPage] = useState(0);
	const result = event.data.result;
	if (!result) return null;
	const pages = Math.ceil(result.rows.length / PAGE_SIZE);
	const start = page * PAGE_SIZE;
	return (
		<>
			<div className="table-scroll">
				<table aria-label={`Result for ${event.operation}`}>
					<thead>
						<tr>
							{result.columns.map((name, i) => (
								<th scope="col" key={`${i}-${name}`}>
									{name}
								</th>
							))}
						</tr>
					</thead>
					<tbody>
						{result.rows.slice(start, start + PAGE_SIZE).map((row, i) => (
							<tr key={start + i}>
								{row.map((cell, j) => (
									<td key={`${start + i}-${j}`}>
										<span className="type-tag">{cell.type}</span>
										{cellText(cell)}
									</td>
								))}
							</tr>
						))}
					</tbody>
				</table>
			</div>
			{pages > 1 && (
				<nav
					className="result-pages"
					aria-label={`Result pages for ${event.operation}`}
				>
					<button
						type="button"
						disabled={page === 0}
						onClick={() => setPage(page - 1)}
					>
						Previous rows
					</button>
					<span>
						{" "}
						Rows {start + 1}–{Math.min(start + PAGE_SIZE, result.rows.length)}{" "}
						of {result.rows.length}{" "}
					</span>
					<button
						type="button"
						disabled={page + 1 >= pages}
						onClick={() => setPage(page + 1)}
					>
						Next rows
					</button>
				</nav>
			)}
		</>
	);
});

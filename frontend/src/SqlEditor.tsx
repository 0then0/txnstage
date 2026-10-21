import { SQLite, sql } from "@codemirror/lang-sql";
import { EditorView, keymap } from "@codemirror/view";
import { basicSetup } from "codemirror";
import { useEffect, useRef } from "react";
export function SqlEditor({
	value,
	onChange,
	onRun,
	label,
}: {
	value: string;
	onChange: (v: string) => void;
	onRun: () => void;
	label: string;
}) {
	const host = useRef<HTMLDivElement>(null);
	const editor = useRef<EditorView | null>(null);
	const callbacks = useRef({ onChange, onRun });
	const programmatic = useRef(false);
	const initial = useRef(value);
	callbacks.current = { onChange, onRun };
	useEffect(() => {
		if (!host.current) return;
		const view = new EditorView({
			parent: host.current,
			doc: initial.current,
			extensions: [
				basicSetup,
				sql({ dialect: SQLite }),
				EditorView.contentAttributes.of({ "aria-label": label }),
				EditorView.lineWrapping,
				keymap.of([
					{
						key: "Mod-Enter",
						run: () => {
							callbacks.current.onRun();
							return true;
						},
					},
				]),
				EditorView.updateListener.of((update) => {
					if (update.docChanged && !programmatic.current)
						callbacks.current.onChange(update.state.doc.toString());
				}),
				EditorView.theme({
					"&": { fontSize: "13px" },
					".cm-content": {
						fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace",
						minHeight: "112px",
						padding: "14px 0",
					},
					".cm-gutters": {
						backgroundColor: "#fafaf7",
						borderRight: "0",
						color: "#92948f",
					},
					"&.cm-focused": {
						outline: "2px solid #a74a25",
						outlineOffset: "-2px",
					},
				}),
			],
		});
		editor.current = view;
		return () => {
			view.destroy();
			editor.current = null;
		};
	}, [label]);
	useEffect(() => {
		const view = editor.current;
		if (view && view.state.doc.toString() !== value) {
			programmatic.current = true;
			view.dispatch({
				changes: { from: 0, to: view.state.doc.length, insert: value },
			});
			programmatic.current = false;
		}
	}, [value]);
	return <div className="sql-editor" ref={host} />;
}

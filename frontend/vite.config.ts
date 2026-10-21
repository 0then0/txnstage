import { defineConfig } from "vite";
export default defineConfig({
	build: {
		rollupOptions: {
			output: {
				manualChunks: {
					editor: ["codemirror", "@codemirror/lang-sql", "@codemirror/view"],
				},
			},
		},
	},
});

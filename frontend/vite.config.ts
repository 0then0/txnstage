import { defineConfig } from "vite";
export default defineConfig({
	build: {
		rolldownOptions: {
			output: {
				codeSplitting: {
					groups: [
						{
							name: "editor",
							test: /[/\\]node_modules[/\\](?:codemirror|@codemirror)[/\\]/,
						},
					],
				},
			},
		},
	},
});

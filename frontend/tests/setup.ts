import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
export default async function setup() {
	const binary =
		process.env.TXNSTAGE_BINARY ??
		path.resolve(
			"../target/release/txnstage" +
				(process.platform === "win32" ? ".exe" : ""),
		);
	const proc = spawn(binary, ["serve"], { stdio: ["pipe", "pipe", "pipe"] });
	let output = "";
	const url = await new Promise<string>((resolve, reject) => {
		const timer = setTimeout(() => {
			proc.kill();
			reject(new Error(`Server startup timeout: ${output}`));
		}, 15000);
		proc.stdout.on("data", (data) => {
			output += data;
			const match = output.match(
				/http:\/\/127\.0\.0\.1:\d+\/#token=[a-f0-9-]+/,
			);
			if (match) {
				clearTimeout(timer);
				resolve(match[0]);
			}
		});
		proc.on("error", reject);
		proc.on("exit", (code) => reject(new Error(`Server exited ${code}`)));
	});
	await mkdir("../.cache/reports", { recursive: true });
	await writeFile("../.cache/e2e-launch.json", JSON.stringify({ url }));
	return async () => {
		proc.kill("SIGINT");
		await new Promise<void>((resolve) => {
			const timer = setTimeout(() => {
				proc.kill("SIGKILL");
				resolve();
			}, 5000);
			proc.once("exit", () => {
				clearTimeout(timer);
				resolve();
			});
		});
	};
}

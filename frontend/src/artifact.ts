export const MAX_ARTIFACT = 8 * 1024 * 1024;

// Size the actual UTF-8 bytes, using the same compact JSON as the server and CLI.
export function artifactBlob(data: unknown): Blob {
	const blob = new Blob([JSON.stringify(data)], { type: "application/json" });
	if (blob.size > MAX_ARTIFACT) throw new Error("Artifact limit is 8 MiB");
	return blob;
}

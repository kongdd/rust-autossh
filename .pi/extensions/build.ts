/**
 * /build — cargo build the workspace for Windows and report exe sizes.
 *
 *   /build                → cross-compile via x86_64-pc-windows-gnu (Linux host)
 *   /build <target>       → cross-compile via <target>
 *   On a Windows host the --target flag is omitted (cargo uses the host triple).
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { spawn } from "node:child_process";
import { readdir, stat } from "node:fs/promises";
import { join } from "node:path";
import { platform } from "node:process";

const DEFAULT_TARGET = "x86_64-pc-windows-gnu";

async function findExes(dir: string): Promise<{ path: string; bytes: number }[]> {
	const out: { path: string; bytes: number }[] = [];
	let entries;
	try {
		entries = await readdir(dir, { withFileTypes: true });
	} catch {
		return out;
	}
	for (const e of entries) {
		const full = join(dir, e.name);
		if (e.isDirectory()) out.push(...(await findExes(full)));
		else if (e.isFile() && e.name.endsWith(".exe"))
			out.push({ path: full, bytes: (await stat(full)).size });
	}
	return out;
}

const fmt = (b: number) =>
	b < 1024
		? `${b} B`
		: b < 1024 * 1024
			? `${(b / 1024).toFixed(1)} KB`
			: `${(b / 1024 / 1024).toFixed(2)} MB`;

export default function (pi: ExtensionAPI) {
	pi.registerCommand("build", {
		description: "Build the workspace for Windows and report exe sizes",
		async handler(args, ctx) {
			const target = platform === "win32" ? undefined : args.trim() || DEFAULT_TARGET;
			const cargoArgs = ["build", "--release", "--workspace", ...(target ? ["--target", target] : [])];

			const code = await new Promise<number>((done) => {
				spawn("cargo", cargoArgs, { cwd: ctx.cwd, stdio: "inherit" }).on(
					"close",
					(c) => done(c ?? 1),
				);
			});
			if (code !== 0) return ctx.ui.notify(`Build failed (exit ${code})`, "error");

			const outDir = join(ctx.cwd, "target", ...(target ? [target] : []), "release");
			const exes = (await findExes(outDir)).sort((a, b) => a.bytes - b.bytes);
			if (exes.length === 0)
				return ctx.ui.notify(`Build OK, no .exe in ${outDir}`, "warning");

			const lines = exes.map((e) => `${fmt(e.bytes).padStart(10)}  ${e.path}`);
			ctx.ui.notify(`Built ${exes.length} exe(s):\n${lines.join("\n")}`, "info");
		},
	});
}

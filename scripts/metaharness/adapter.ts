import {
	finalAnswer,
	rulerDataset,
	rulerProvenance,
	scoreRuler,
	type RulerSpec,
} from "./ruler";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
	cp,
	lstat,
	mkdir,
	readFile,
	readdir,
	realpath,
	stat,
	writeFile,
} from "node:fs/promises";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

export const VERSION = 1;
const activeProcesses = new Set<() => void>();
const MAX_TRACE_BYTES = 64 * 1024 * 1024;
export type Command = string[];
export interface Arm {
	id: string;
	source: string;
	binary: string;
	provenance: string;
	args: string[];
}
export interface Task {
	datasetIndex?: number;
	id: string;
	name: string;
	input: string;
	expected: string;
	prompt: string;
}
export interface Manifest {
	answerVerifier?: RulerSpec;
	version: 1;
	model: string;
	tasks: Task[];
	baseline: Arm;
	candidate: Arm;
	repetitions: number;
	timeoutMs: number;
	output: string;
	verifier: {
		id: string;
		mode: "exact" | "formatted";
		command?: Command;
		executableSha256?: string;
	};
}
export interface Telemetry {
	input: number;
	output: number;
	cacheRead: number;
	cacheWrite: number;
	spend: number;
	reads: number;
	edits: number;
	writes: number;
	editSuccesses: number;
	toolInputChars: number;
	retries: number;
	terminal: boolean;
	models: string[];
	errors: string[];
}
export interface Run extends Telemetry {
	answer?: string;
	answerScore?: number;
	taskId: string;
	name: string;
	runIndex: number;
	arm: string;
	phase: "aa" | "ab";
	model: string;
	success: boolean;
	duration: number;
	timedOut: boolean;
	exitCode: number;
	error?: string;
	telemetryError?: string;
	verification: string[];
	tracePath: string;
	transportExhausted: boolean;
}
const zero = (): Telemetry => ({
	input: 0,
	output: 0,
	cacheRead: 0,
	cacheWrite: 0,
	spend: 0,
	reads: 0,
	edits: 0,
	writes: 0,
	editSuccesses: 0,
	toolInputChars: 0,
	retries: 0,
	terminal: false,
	models: [],
	errors: [],
});
export function digest(bytes: string | Uint8Array) {
	return createHash("sha256").update(bytes).digest("hex");
}
function requireNumber(value: unknown, label: string): number {
	if (typeof value !== "number" || !Number.isFinite(value) || value < 0)
		throw new Error(`Invalid ${label}`);
	return value;
}
/** Only message_end contributes usage; agent_end repeats messages and must not double-count. */
export function telemetry(trace: string, out: Telemetry = zero()): Telemetry {
	const starts = new Set<string>();
	const ends = new Set<string>();
	for (const line of trace.split("\n").filter((s) => s.trim())) {
		const e = JSON.parse(line);
		if (!e || typeof e.type !== "string")
			throw new Error("Invalid JSON event envelope");
		if (e.type === "message_end" && e.message?.role === "assistant") {
			const m = e.message,
				u = m.usage;
			if (!u) throw new Error("Assistant completion missing usage");
			out.input += requireNumber(u.input, "usage.input");
			out.output += requireNumber(u.output, "usage.output");
			out.cacheRead += requireNumber(u.cacheRead, "usage.cacheRead");
			out.cacheWrite += requireNumber(u.cacheWrite, "usage.cacheWrite");
			out.spend += requireNumber(u.cost?.total, "usage.cost.total");
			if (typeof m.model !== "string" || !m.model)
				throw new Error("Missing observed model");
			const model =
				m.provider && !m.model.startsWith(`${m.provider}/`)
					? `${m.provider}/${m.model}`
					: m.model;
			if (!out.models.includes(model)) out.models.push(model);
			if (m.errorMessage) out.errors.push(String(m.errorMessage));
		}
		if (e.type === "tool_execution_start") {
			if (typeof e.toolCallId !== "string" || starts.has(e.toolCallId))
				throw new Error("Duplicate or missing tool call identity");
			starts.add(e.toolCallId);
			if (e.toolName === "read") out.reads++;
			if (
				[
					"edit",
					"replace",
					"patch",
					"apply_patch",
					"sloppy",
					"ast_edit",
				].includes(e.toolName)
			)
				out.edits++;
			if (e.toolName === "write") out.writes++;
			if (e.args === undefined) throw new Error("Tool call omitted arguments");
			out.toolInputChars += JSON.stringify(e.args).length;
		}
		if (e.type === "tool_execution_end") {
			if (!starts.has(e.toolCallId) || ends.has(e.toolCallId))
				throw new Error("Unpaired tool completion");
			ends.add(e.toolCallId);
			if (typeof e.isError !== "boolean")
				throw new Error("Tool result omitted isError");
			if (
				[
					"edit",
					"replace",
					"patch",
					"apply_patch",
					"sloppy",
					"ast_edit",
				].includes(e.toolName) &&
				!e.isError
			)
				out.editSuccesses++;
		}
		if (e.type === "auto_retry_start") out.retries++;
		if (e.type === "agent_end" && e.isTerminal === true) out.terminal = true;
	}
	if (out.terminal && starts.size !== ends.size)
		throw new Error("Terminal trace contains unsettled tools");
	return out;
}
export async function files(root: string): Promise<string[]> {
	const result: string[] = [];
	async function walk(dir: string) {
		for (const name of (await readdir(dir)).sort()) {
			const path = join(dir, name),
				info = await lstat(path);
			if (info.isSymbolicLink())
				throw new Error(`Symlink refused in fixture/output: ${path}`);
			if (info.isDirectory()) await walk(path);
			else if (info.isFile()) result.push(relative(root, path));
			else throw new Error(`Non-regular benchmark file: ${path}`);
		}
	}
	await walk(root);
	return result;
}
export async function treeHash(root: string) {
	const h = createHash("sha256");
	for (const name of await files(root)) {
		h.update(name);
		h.update("\0");
		h.update(digest(await readFile(join(root, name))));
		h.update("\0");
	}
	return h.digest("hex");
}
async function capture(command: Command, cwd: string) {
	if (!command.length) throw new Error("Empty command");
	const { stdout, stderr, exitCode, timedOut, error } = await execute(
		command,
		cwd,
		3_600_000,
	);
	if (exitCode !== 0 || timedOut || error)
		throw new Error(
			`Command failed (${exitCode}): ${error ?? (timedOut ? "deadline exceeded" : stderr)}`,
		);
	return stdout;
}
/** Includes tracked working-tree bytes and untracked, nonignored files, never mtime alone. */
export async function sourceIdentity(root: string) {
	const commit = (await capture(["git", "rev-parse", "HEAD"], root)).trim();
	const names = (
		await capture(
			["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
			root,
		)
	)
		.split("\0")
		.filter(Boolean)
		.sort();
	const h = createHash("sha256");
	let latestMs = 0;
	for (const name of names) {
		const path = join(root, name);
		h.update(name);
		h.update("\0");
		try {
			const info = await lstat(path);
			if (!info.isFile()) throw new Error(`Nonregular source: ${name}`);
			h.update(digest(await readFile(path)));
			latestMs = Math.max(latestMs, info.mtimeMs);
		} catch (e: any) {
			if (e.code !== "ENOENT") throw e;
			h.update("DELETED");
		}
		h.update("\0");
	}
	return { commit, sourceSha256: h.digest("hex"), latestMs };
}
export async function verifyArm(arm: Arm) {
	const proof = JSON.parse(await readFile(arm.provenance, "utf8"));
	const source = await sourceIdentity(arm.source),
		binarySha256 = digest(await readFile(arm.binary));
	if (
		proof.version !== VERSION ||
		proof.commit !== source.commit ||
		proof.sourceSha256 !== source.sourceSha256 ||
		proof.binarySha256 !== binarySha256
	)
		throw new Error(`Stale or mismatched source/build provenance: ${arm.id}`);
	if ((await stat(arm.binary)).mtimeMs < source.latestMs)
		throw new Error(`Binary older than source: ${arm.id}`);
	return { ...source, binarySha256, buildCommand: proof.buildCommand };
}
/** Must wrap the real build; no separate stamp-existing-binary command. */
export async function build(
	source: string,
	binary: string,
	provenance: string,
	command: Command,
) {
	const before = await sourceIdentity(source);
	const started = Date.now();
	await capture(command, source);
	const after = await sourceIdentity(source);
	if (
		before.sourceSha256 !== after.sourceSha256 ||
		before.commit !== after.commit
	)
		throw new Error("Source changed during build");
	const info = await stat(binary);
	if (info.mtimeMs < Math.max(started - 1000, after.latestMs))
		throw new Error("Build did not produce a fresh executable");
	await mkdir(dirname(provenance), { recursive: true });
	await writeFile(
		provenance,
		JSON.stringify(
			{
				version: VERSION,
				...after,
				binarySha256: digest(await readFile(binary)),
				buildCommand: command,
				builtAt: new Date().toISOString(),
			},
			null,
			2,
		),
	);
}
/** Own the process group so a timeout also closes descendant-held pipes. */
export async function execute(
	command: Command,
	cwd: string,
	timeoutMs: number,
	input?: Uint8Array,
	env: NodeJS.ProcessEnv = process.env,
) {
	if (process.platform === "win32")
		throw new Error("Benchmark process-tree ownership currently requires Unix");
	return await new Promise<{
		stdout: string;
		stderr: string;
		exitCode: number;
		timedOut: boolean;
		error?: string;
	}>((resolveResult) => {
		const child = spawn(command[0]!, command.slice(1), {
			cwd,
			env,
			detached: true,
			stdio: ["pipe", "pipe", "pipe"],
		});
		let stdout = "",
			stderr = "",
			size = 0,
			timedOut = false,
			error: string | undefined,
			settled = false;
		const kill = () => {
			if (child.pid)
				try {
					process.kill(-child.pid, "SIGKILL");
				} catch {
					child.kill("SIGKILL");
				}
		};
		activeProcesses.add(kill);
		const finish = (exitCode: number) => {
			if (settled) return;
			settled = true;
			activeProcesses.delete(kill);
			clearTimeout(timer);
			clearTimeout(reaper);
			kill();
			resolveResult({ stdout, stderr, exitCode, timedOut, error });
		};
		const timer = setTimeout(() => {
			timedOut = true;
			kill();
		}, timeoutMs);
		const reaper = setTimeout(() => {
			error ??= "Process pipes failed to settle after cancellation";
			child.stdout.destroy();
			child.stderr.destroy();
			finish(-1);
		}, timeoutMs + 2000);
		child.stdout.setEncoding("utf8");
		child.stderr.setEncoding("utf8");
		const collect = (chunk: string, kind: "stdout" | "stderr") => {
			size += Buffer.byteLength(chunk);
			if (size > MAX_TRACE_BYTES) {
				error = "Trace byte limit exceeded";
				kill();
				return;
			}
			if (kind === "stdout") stdout += chunk;
			else stderr += chunk;
		};
		child.stdout.on("data", (chunk) => collect(chunk, "stdout"));
		child.stderr.on("data", (chunk) => collect(chunk, "stderr"));
		child.on("error", (e) => {
			error = String(e);
			finish(-1);
		});
		child.on("close", (code) => finish(code ?? -1));
		child.stdin.on("error", () => {});
		child.stdin.end(input);
	});
}
async function normalized(path: string, verifier: Manifest["verifier"]) {
	const bytes = await readFile(path);
	if (verifier.mode === "exact") return bytes;
	const command = verifier.command;
	if (
		!command?.length ||
		!isAbsolute(command[0]!) ||
		!verifier.executableSha256
	)
		throw new Error(
			"Formatted verification needs a pinned absolute formatter executable",
		);
	if (digest(await readFile(command[0]!)) !== verifier.executableSha256)
		throw new Error("Formatter executable changed");
	const result = await execute(
		command.map((s) => s.replaceAll("{path}", path)),
		dirname(path),
		30_000,
		bytes,
	);
	if (result.exitCode !== 0 || result.error || result.timedOut)
		throw new Error(`Formatter failed: ${result.error ?? result.stderr}`);
	return Buffer.from(result.stdout);
}
/** Full tree comparison catches missing/extra files and changes outside the requested edit. */
export async function verifyFiles(
	expected: string,
	actual: string,
	verifier: Manifest["verifier"],
): Promise<string[]> {
	const want = await files(expected),
		got = await files(actual),
		failures: string[] = [];
	for (const name of want)
		if (!got.includes(name)) failures.push(`Missing: ${name}`);
	for (const name of got)
		if (!want.includes(name)) failures.push(`Unexpected: ${name}`);
	for (const name of want.filter((n) => got.includes(n)))
		if (
			!(await normalized(join(expected, name), verifier)).equals(
				await normalized(join(actual, name), verifier),
			)
		)
			failures.push(`Different: ${name}`);
	return failures;
}
export function ghost(r: Run) {
	return (
		!r.success &&
		!r.telemetryError &&
		((r.input + r.output + r.cacheRead + r.cacheWrite === 0 &&
			r.reads + r.edits + r.writes === 0) ||
			r.transportExhausted)
	);
}
export function summarize(runs: Run[], taskIds: string[]) {
	const best = taskIds.flatMap((id) =>
		runs
			.filter((r) => r.taskId === id)
			.sort(
				(a, b) =>
					Number(b.success) - Number(a.success) ||
					Number(ghost(a)) - Number(ghost(b)) ||
					a.input +
						a.output +
						a.cacheRead +
						a.cacheWrite -
						(b.input + b.output + b.cacheRead + b.cacheWrite) ||
					a.runIndex - b.runIndex,
			)
			.slice(0, 1),
	);
	const sum = (key: keyof Telemetry, list = best) =>
			list.reduce((n, r) => n + Number(r[key]), 0),
		denom = best.length || 1;
	const input = sum("input") / denom,
		cacheRead = sum("cacheRead") / denom,
		cacheWrite = sum("cacheWrite") / denom;
	const nonGhost = runs.filter((r) => !ghost(r)),
		successes = runs.filter((r) => r.success).length;
	const summary = {
		totalTasks: taskIds.length,
		totalRuns: nonGhost.length,
		successfulRuns: successes,
		successfulTasks: best.filter((r) => r.success).length,
		taskSuccessRate:
			best.filter((r) => r.success).length / (taskIds.length || 1),
		editSuccessRate: sum("edits") ? sum("editSuccesses") / sum("edits") : 1,
		totalTokens: { input: sum("input"), output: sum("output") },
		avgTokensPerTask: {
			input: Math.round(input),
			output: Math.round(sum("output") / denom),
		},
		avgDurationPerTask: Math.round(
			best.reduce((s, r) => s + r.duration, 0) / denom,
		),
		ghostRuns: runs.filter(ghost).length,
		timeoutRuns: nonGhost.filter((r) => r.timedOut).length,
		transportFailureRuns: runs.filter((r) => r.transportExhausted).length,
	};
	return {
		summary,
		metrics: {
			success_pct: summary.taskSuccessRate * 100,
			edit_success_pct: summary.editSuccessRate * 100,
			avg_tok_in: Math.round(input),
			avg_tok_out: summary.avgTokensPerTask.output,
			reads: sum("reads") / denom,
			edits: sum("edits") / denom,
			writes: sum("writes") / denom,
			tool_input_chars: sum("toolInputChars") / denom,
			avg_time_ms: summary.avgDurationPerTask,
			ghost_runs: summary.ghostRuns,
			timeout_runs: summary.timeoutRuns,
			retries_used: sum("retries", runs),
			cacheRead,
			cacheWrite,
			cacheReadTokens: cacheRead,
			cacheWriteTokens: cacheWrite,
			spend: sum("spend") / denom,
			cache_hit_ratio:
				cacheRead + input ? cacheRead / (cacheRead + input) : null,
		},
		allRuns: {
			attempted: runs.length,
			successes,
			failures: runs.length - successes,
			telemetryIncomplete: runs.filter((r) => r.telemetryError).length,
			successRate: runs.length ? successes / runs.length : null,
			timedOut: runs.filter((r) => r.timedOut).length,
			input: sum("input", runs),
			output: sum("output", runs),
			cacheRead: sum("cacheRead", runs),
			cacheWrite: sum("cacheWrite", runs),
			spend: sum("spend", runs),
			retries: sum("retries", runs),
		},
		tasks: taskIds.map((id) => ({
			id,
			name: runs.find((r) => r.taskId === id)?.name ?? id,
			runs: runs
				.filter((r) => r.taskId === id)
				.map((r) => ({
					...r,
					tokens: { input: r.input, output: r.output, reasoning: 0 },
					toolCalls: { read: r.reads, edit: r.edits, write: r.writes },
				})),
		})),
	};
}
export function schedule(
	tasks: Task[],
	repetitions: number,
	baseline: Arm,
	candidate: Arm,
) {
	const plan: {
		task: Task;
		runIndex: number;
		arm: Arm;
		label: string;
		phase: "aa" | "ab";
	}[] = [];
	for (const phase of ["aa", "ab"] as const)
		for (let runIndex = 0; runIndex < repetitions; runIndex++)
			for (const [index, task] of tasks.entries()) {
				const pair =
					phase === "aa"
						? [
								{ arm: baseline, label: "aa-a" },
								{ arm: baseline, label: "aa-b" },
							]
						: [
								{ arm: baseline, label: "baseline" },
								{ arm: candidate, label: "candidate" },
							];
				if ((index + runIndex) % 2) pair.reverse();
				for (const p of pair) plan.push({ task, runIndex, ...p, phase });
			}
	return plan;
}
export function compare(
	aaA: ReturnType<typeof summarize>,
	aaB: ReturnType<typeof summarize>,
	base: ReturnType<typeof summarize>,
	candidate: ReturnType<typeof summarize>,
	count: number,
) {
	return Object.fromEntries(
		Object.keys(base.metrics).map((key) => {
			const k = key as keyof typeof base.metrics,
				a = aaA.metrics[k],
				b = aaB.metrics[k],
				c = base.metrics[k],
				d = candidate.metrics[k];
			if ([a, b, c, d].some((v) => v === null))
				return [
					key,
					{
						aaDelta: null,
						delta: null,
						resolution: "unresolved: no denominator",
					},
				];
			const aaDelta = Math.abs(b! - a!),
				delta = d! - c!;
			return [
				key,
				{
					aaDelta,
					delta,
					resolution: [aaA, aaB, base, candidate].some(
						(s) => s.allRuns.telemetryIncomplete > 0,
					)
						? "unresolved: incomplete telemetry"
						: Math.abs(delta) <= aaDelta ||
							  (k === "success_pct" && count <= 20 && Math.abs(delta) < 5)
							? "unresolved"
							: "exceeds observed A/A floor; no statistical significance claim",
				},
			];
		}),
	);
}
function outside(path: string, root: string) {
	const rel = relative(root, path);
	return rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel);
}
export async function runExperiment(manifest: Manifest) {
	if (
		manifest.version !== 1 ||
		!manifest.model ||
		/(?:^|[-/])latest$/i.test(manifest.model)
	)
		throw new Error("Pin an exact model id, not latest");
	if (
		!Number.isInteger(manifest.repetitions) ||
		manifest.repetitions < 1 ||
		!Number.isFinite(manifest.timeoutMs) ||
		manifest.timeoutMs <= 0 ||
		!manifest.tasks.length
	)
		throw new Error("Invalid run bounds");
	if (manifest.tasks.some((t) => !t.id || t.id === "." || t.id === ".."))
		throw new Error("Invalid task id");
	if (new Set(manifest.tasks.map((t) => t.id)).size !== manifest.tasks.length)
		throw new Error("Duplicate task ids");
	for (const arm of [manifest.baseline, manifest.candidate]) {
		for (const path of [arm.source, arm.binary, arm.provenance])
			if (!isAbsolute(path)) throw new Error("Arm paths must be absolute");
		if (
			arm.args.some((arg) =>
				/^(?:--(?:model|project|session|session-dir|mode|config-dir)(?:=|$)|-m$|-p$|--print$|--no-tools$)/.test(
					arg,
				),
			)
		)
			throw new Error(
				"Arm argv overrides an adapter-owned setting or disables tools",
			);
	}
	if (
		manifest.verifier.mode !== "exact" &&
		manifest.verifier.mode !== "formatted"
	)
		throw new Error("Unknown verifier mode");
	const evaluatorPath = new URL(import.meta.url),
		evaluatorSha256 = digest(await readFile(evaluatorPath));
	const rulerEvaluatorPath = new URL("./ruler.ts", import.meta.url),
		rulerEvaluatorSha256 = digest(await readFile(rulerEvaluatorPath));
	const output = resolve(manifest.output);
	for (const arm of [manifest.baseline, manifest.candidate])
		if (!outside(output, resolve(arm.source)))
			throw new Error("Output must be outside source trees");
	for (const task of manifest.tasks)
		for (const dir of [task.input, task.expected])
			if (!outside(output, await realpath(dir)))
				throw new Error("Output overlaps fixtures");
	await mkdir(output, { recursive: true });
	if ((await readdir(output)).length)
		throw new Error("Output directory must be empty");
	const identities = {
		baseline: await verifyArm(manifest.baseline),
		candidate: await verifyArm(manifest.candidate),
	};
	const fixtureHashes = await Promise.all(
		manifest.tasks.map(async (t) => ({
			id: t.id,
			input: await treeHash(t.input),
			expected: await treeHash(t.expected),
		})),
	);
	const answerRecords = manifest.answerVerifier
		? await rulerDataset(manifest.answerVerifier)
		: [];
	const answerProvenance = manifest.answerVerifier
		? await rulerProvenance(manifest.answerVerifier)
		: undefined;
	if (manifest.answerVerifier) {
		const selected = new Set<number>();
		for (const task of manifest.tasks) {
			const record = answerRecords.find((r) => r.index === task.datasetIndex);
			if (!record || record.input !== task.prompt)
				throw new Error(
					"Task prompt must exactly match its pinned RULER dataset input",
				);
			if (selected.has(record.index))
				throw new Error("Repeated RULER dataset index");
			selected.add(record.index);
		}
	}
	const runs: Run[] = [];
	for (const [ordinal, item] of schedule(
		manifest.tasks,
		manifest.repetitions,
		manifest.baseline,
		manifest.candidate,
	).entries()) {
		await verifyArm(item.arm);
		const rowRoot = join(output, `trial-${ordinal}`),
			project = join(rowRoot, "project");
		await mkdir(rowRoot, { recursive: true });
		await cp(item.task.input, project, { recursive: true });
		const command = [
			item.arm.binary,
			"--mode",
			"json",
			"--model",
			manifest.model,
			"--project",
			project,
			"--session-dir",
			join(rowRoot, "sessions"),
			...item.arm.args,
			item.task.prompt,
		];
		const started = performance.now();
		const result = await execute(
			command,
			project,
			manifest.timeoutMs,
			undefined,
			{ ...process.env, OMP_CONFIG_DIR: join(rowRoot, "config") },
		);
		const { stdout, stderr, exitCode, timedOut } = result;
		let error = result.error;
		const duration = performance.now() - started;
		await writeFile(join(rowRoot, "trace.jsonl"), stdout);
		await writeFile(join(rowRoot, "stderr.txt"), stderr);
		const observed = zero();
		let telemetryError: string | undefined;
		try {
			telemetry(stdout, observed);
		} catch (e) {
			telemetryError = String(e);
			error = `Invalid telemetry: ${e}`;
		}
		if (observed.models.some((m) => m !== manifest.model))
			error = `Observed model mismatch: ${observed.models.join(", ")}`;
		let answer: string | undefined, answerScore: number | undefined;
		let verification: string[] = [];
		try {
			verification = await verifyFiles(
				item.task.expected,
				project,
				manifest.verifier,
			);
		} catch (e) {
			verification = [String(e)];
		}
		if (manifest.answerVerifier) {
			try {
				answer = finalAnswer(stdout);
				const record = answerRecords.find(
					(r) => r.index === item.task.datasetIndex,
				)!;
				answerScore = await scoreRuler(
					manifest.answerVerifier,
					[answer],
					[record.outputs],
					execute,
				);
				if (answerScore !== 100)
					verification.push(`Official RULER score ${answerScore}/100`);
			} catch (e) {
				verification.push(String(e));
			}
		}
		if (!observed.terminal) error ??= "Missing terminal agent_end";
		if (timedOut) error = "Timeout";
		else if (exitCode !== 0) error ??= `Process exit ${exitCode}`;
		if (observed.errors.length) error ??= observed.errors.join("; ");
		const row: Run = {
			...observed,
			answer,
			answerScore,
			taskId: item.task.id,
			name: item.task.name,
			runIndex: item.runIndex,
			arm: item.label,
			phase: item.phase,
			model: manifest.model,
			success: !error && verification.length === 0,
			duration,
			timedOut,
			exitCode,
			error,
			telemetryError,
			verification,
			tracePath: relative(output, join(rowRoot, "trace.jsonl")),
			transportExhausted: stdout.split("\n").some((line) => {
				try {
					const e = JSON.parse(line);
					return (
						e.type === "auto_retry_end" &&
						e.success === false &&
						typeof e.finalError === "string" &&
						/Timeout exhausted/.test(e.finalError)
					);
				} catch {
					return false;
				}
			}),
		};
		runs.push(row);
		await writeFile(join(output, "runs.json"), JSON.stringify(runs, null, 2));
		// Detect corruption immediately, including candidate changes to shared fixture/evaluator paths.
		for (const [i, t] of manifest.tasks.entries())
			if (
				(await treeHash(t.input)) !== fixtureHashes[i]!.input ||
				(await treeHash(t.expected)) !== fixtureHashes[i]!.expected
			)
				throw new Error("Fixture integrity changed; experiment invalid");
		await verifyArm(item.arm);
		if (
			digest(await readFile(evaluatorPath)) !== evaluatorSha256 ||
			digest(await readFile(rulerEvaluatorPath)) !== rulerEvaluatorSha256
		)
			throw new Error("Adapter evaluator changed during experiment");
	}
	const ids = manifest.tasks.map((t) => t.id),
		summaries = Object.fromEntries(
			["aa-a", "aa-b", "baseline", "candidate"].map((label) => [
				label,
				summarize(
					runs.filter((r) => r.arm === label),
					ids,
				),
			]),
		);
	const comparison = compare(
		summaries["aa-a"]!,
		summaries["aa-b"]!,
		summaries.baseline!,
		summaries.candidate!,
		ids.length,
	);
	const answerEvaluation = manifest.answerVerifier
		? {
				provenance: answerProvenance,
				selectedIndices: manifest.tasks.map((t) => t.datasetIndex),
				protocol:
					"OMP agent harness on pinned RULER records; not the unmodified upstream inference harness or full-suite claim",
				arms: await Promise.all(
					["aa-a", "aa-b", "baseline", "candidate"].map(async (arm) => {
						const rows = runs.filter((r) => r.arm === arm);
						const references = rows.map(
							(r) =>
								answerRecords.find(
									(record) =>
										record.index ===
										manifest.tasks.find((t) => t.id === r.taskId)!.datasetIndex,
								)!.outputs,
						);
						return {
							arm,
							attempts: rows.length,
							score: await scoreRuler(
								manifest.answerVerifier!,
								rows.map((r) =>
									r.error || r.telemetryError ? "" : (r.answer ?? ""),
								),
								references,
								execute,
							),
						};
					}),
				),
			}
		: undefined;
	const report = {
		version: VERSION,
		answerEvaluation,
		evaluatorSha256,
		rulerEvaluatorSha256,
		manifest,
		identities,
		fixtureHashes,
		verifier: manifest.verifier,
		comparison,
		summaries,
		limitations: [
			"Process-backed results require review of actual traces and external verifier.",
			"Integrity detection is not evaluator write-denial (#50 remains required).",
			"Observed model is presentation identity; provider-side exact snapshot attestation is not exposed.",
			"Tool input characters are canonical JSON UTF-16 length, not raw provider argument bytes.",
			"Incomplete telemetry makes usage totals lower bounds and all comparisons unresolved.",
			"Per-category cache monetary costs are unavailable; cacheRead/cacheWrite are tokens, spend is total USD.",
		],
	};
	await writeFile(join(output, "report.json"), JSON.stringify(report, null, 2));
	for (const [label, summary] of Object.entries(summaries)) {
		await mkdir(join(output, label));
		await writeFile(
			join(output, label, "result.json"),
			JSON.stringify(summary, null, 2),
		);
		for (const task of summary.tasks)
			for (const run of task.runs) {
				const traceDir = join(
					output,
					label,
					"result.dump",
					task.id.replace(/[^a-zA-Z0-9._-]/g, "_"),
				);
				await mkdir(traceDir, { recursive: true });
				await writeFile(
					join(traceDir, `run-${run.runIndex + 1}.md`),
					`# ${JSON.stringify(task.name)}\n\nOutcome: ${run.success ? "pass" : "fail"}\n\nRaw trace: ${run.tracePath}\n\n${JSON.stringify(run, null, 2)}\n`,
				);
			}
	}
	const table = [
		"# OMP2 paired edit benchmark",
		"",
		"Staleness guard: source and binary hashes verified before and after each trial.",
		"",
		"| Metric | A/A delta | Candidate − baseline | Interpretation |",
		"|---|---:|---:|---|",
	];
	for (const [metric, row] of Object.entries(comparison))
		table.push(
			`| ${metric} | ${row.aaDelta ?? "unavailable"} | ${row.delta ?? "unavailable"} | ${row.resolution} |`,
		);
	table.push(
		"",
		"No automatic promotion. See report.json for all-run outcomes, verifier identity, provenance and limitations.",
	);
	if (answerEvaluation) {
		table.push(
			"",
			"## Official RULER answer scores",
			"",
			answerEvaluation.protocol,
			"",
			"| Arm | All attempts | Official score / 100 |",
			"| --- | ---: | ---: |",
		);
		for (const arm of answerEvaluation.arms)
			table.push(`| ${arm.arm} | ${arm.attempts} | ${arm.score} |`);
		table.push(
			"",
			"Dataset and scorer digests, selected indices, and declared generation provenance are in report.json. These scores do not establish an improvement or a full-suite result.",
		);
	}
	await writeFile(join(output, "summary.md"), table.join("\n") + "\n");
	return report;
}
export async function validateComparison(manifest: Manifest, flags: string[]) {
	if (flags.length === 0) return;
	const baseline = await verifyArm(manifest.baseline),
		candidate = await verifyArm(manifest.candidate);
	if (flags.length === 1 && flags[0] === "--same-commit") {
		if (
			baseline.commit !== candidate.commit ||
			baseline.sourceSha256 !== candidate.sourceSha256 ||
			baseline.binarySha256 !== candidate.binarySha256
		)
			throw new Error(
				"--same-commit requires identical source and binary identities for both prepared arms",
			);
		return;
	}
	if (flags.length === 2 && flags[0] === "--base") {
		const expected = (
			await capture(
				[
					"git",
					"rev-parse",
					"--verify",
					"--end-of-options",
					`${flags[1]}^{commit}`,
				],
				manifest.candidate.source,
			)
		).trim();
		if (baseline.commit !== expected)
			throw new Error("Prepared baseline does not match --base reference");
		return;
	}
	throw new Error("Expected --same-commit or --base REF, exclusively");
}
export async function main(args: string[]) {
	const stop = (code: number) => {
		for (const kill of activeProcesses) kill();
		process.exit(code);
	};
	process.once("SIGINT", () => stop(130));
	process.once("SIGTERM", () => stop(143));
	if (args[0] === "run" && args.length >= 2) {
		const manifest: Manifest = JSON.parse(await readFile(args[1]!, "utf8"));
		await validateComparison(manifest, args.slice(2));
		await runExperiment(manifest);
		return;
	}
	if (args[0] === "build" && args[4] === "--" && args.length > 5) {
		await build(
			resolve(args[1]!),
			resolve(args[2]!),
			resolve(args[3]!),
			args.slice(5),
		);
		return;
	}
	throw new Error(
		"Usage: bun scripts/metaharness-omp2.ts run manifest.json [--same-commit | --base REF] | build SOURCE BINARY PROVENANCE -- BUILD_COMMAND [ARGS...]",
	);
}

import { RULER_REVISION } from "./ruler";
import { afterEach, expect, test } from "bun:test";
import {
	chmod,
	mkdir,
	mkdtemp,
	readFile,
	realpath,
	rm,
	writeFile,
	utimes,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	build,
	compare,
	digest,
	execute,
	ghost,
	runExperiment,
	schedule,
	sourceIdentity,
	summarize,
	telemetry,
	trialEnvironment,
	validateComparison,
	verifyArm,
	verifyFiles,
	type Arm,
	type Manifest,
	type Run,
} from "./adapter";
const roots: string[] = [];
async function temp() {
	const root = await mkdtemp(join(tmpdir(), "omp-adapter-test-"));
	roots.push(root);
	return root;
}
afterEach(async () => {
	for (const root of roots.splice(0))
		await rm(root, { recursive: true, force: true });
});
const message = {
	type: "message_end",
	message: {
		role: "assistant",
		provider: "test",
		model: "snapshot-2026-01-01",
		usage: {
			input: 10,
			output: 3,
			cacheRead: 90,
			cacheWrite: 4,
			cost: { total: 0.2 },
		},
	},
};
const trace = [
	{
		type: "tool_execution_start",
		toolCallId: "1",
		toolName: "edit",
		args: { path: "answer.txt", text: "done" },
	},
	{
		type: "tool_execution_end",
		toolCallId: "1",
		toolName: "edit",
		isError: false,
	},
	message,
	{ type: "agent_end", isTerminal: true, messages: [message.message] },
]
	.map((e) => JSON.stringify(e))
	.join("\n");
function run(overrides: Partial<Run> = {}): Run {
	return {
		...telemetry(trace),
		taskId: "one",
		name: "One",
		runIndex: 0,
		arm: "baseline",
		phase: "ab",
		model: "test/snapshot-2026-01-01",
		success: true,
		duration: 10,
		timedOut: false,
		exitCode: 0,
		verification: [],
		tracePath: "trace.jsonl",
		transportExhausted: false,
		...overrides,
	};
}

test("print events count final usage once and preserve cache and spend", () => {
	const t = telemetry(trace);
	expect(t.input).toBe(10);
	expect(t.cacheRead).toBe(90);
	expect(t.spend).toBe(0.2);
	expect(t.editSuccesses).toBe(1);
	expect(t.terminal).toBe(true);
	expect(() => telemetry(trace + "\nnot-json")).toThrow();
	expect(() =>
		telemetry(
			JSON.stringify({
				type: "message_end",
				message: { role: "assistant", usage: { input: 1 } },
			}),
		),
	).toThrow();
	expect(() =>
		telemetry(
			JSON.stringify({
				type: "tool_execution_end",
				toolCallId: "absent",
				isError: false,
			}),
		),
	).toThrow();
});
test("v1 best-run semantics never hide all-run costs or failures", () => {
	const rows = [
		run({ success: false, input: 100, runIndex: 0, error: "edit rejected" }),
		run({ input: 20, runIndex: 1 }),
		run({ input: 10, runIndex: 2 }),
	];
	const s = summarize(rows, ["one"]);
	expect(s.metrics.success_pct).toBe(100);
	expect(s.metrics.avg_tok_in).toBe(10);
	expect(s.allRuns.input).toBe(130);
	expect(s.allRuns.failures).toBe(1);
	expect(s.summary.totalRuns).toBe(3);
	expect(s.metrics.cache_hit_ratio).toBe(0.9);
	expect(
		ghost(
			run({
				success: false,
				input: 0,
				output: 0,
				cacheRead: 0,
				cacheWrite: 0,
				reads: 0,
				edits: 0,
				writes: 0,
			}),
		),
	).toBe(true);
	expect(
		summarize(
			[run({ success: false, timedOut: true, transportExhausted: true })],
			["one"],
		).allRuns.timedOut,
	).toBe(1);
});
test("A/A precedes comparison, arms interleave and ordering alternates", () => {
	const arm = {} as Arm,
		tasks = [{ id: "one" }, { id: "two" }] as Manifest["tasks"];
	expect(
		schedule(tasks, 1, arm, arm).map(
			(p) => `${p.phase}:${p.task.id}:${p.label}`,
		),
	).toEqual([
		"aa:one:aa-a",
		"aa:one:aa-b",
		"aa:two:aa-b",
		"aa:two:aa-a",
		"ab:one:baseline",
		"ab:one:candidate",
		"ab:two:candidate",
		"ab:two:baseline",
	]);
	const a = summarize([run()], ["one"]),
		b = summarize([run({ success: false })], ["one"]);
	expect(compare(a, b, a, b, 20).success_pct.resolution).toBe("unresolved");
	expect(compare(a, a, a, a, 20).cache_hit_ratio.resolution).toBe("unresolved");
});
test("external verifier rejects extra files and altered unrelated content", async () => {
	const root = await temp(),
		expected = join(root, "expected"),
		actual = join(root, "actual");
	await mkdir(expected);
	await mkdir(actual);
	for (const dir of [expected, actual]) {
		await writeFile(join(dir, "answer.txt"), "done");
		await writeFile(join(dir, "unchanged.txt"), "preserve");
	}
	const verifier = { id: "bytes-v1", mode: "exact" as const };
	expect(await verifyFiles(expected, actual, verifier)).toEqual([]);
	await writeFile(join(actual, "unchanged.txt"), "damaged");
	await writeFile(join(actual, "extra.txt"), "unexpected");
	expect(await verifyFiles(expected, actual, verifier)).toEqual([
		"Unexpected: extra.txt",
		"Different: unchanged.txt",
	]);
});
async function fixtureArm(root: string): Promise<Arm> {
	const source = join(root, "source");
	await mkdir(source);
	await writeFile(join(source, "source.rs"), "fn main() {}\n");
	for (const command of [
		["git", "init", "-q"],
		["git", "add", "source.rs"],
		[
			"git",
			"-c",
			"user.name=Test",
			"-c",
			"user.email=test@example.test",
			"commit",
			"-qm",
			"fixture",
		],
	]) {
		const result = await execute(command, source, 5000);
		if (result.exitCode !== 0) throw new Error(result.stderr);
	}
	const binary = join(root, "fixture-command");
	await writeFile(
		binary,
		`#!/bin/sh\nprintf done > answer.txt\ncat <<'TRACE'\n${trace}\nTRACE\n`,
	);
	await chmod(binary, 0o755);
	const arm = {
		id: "fixture-only",
		source,
		binary,
		provenance: join(root, "proof.json"),
		args: [],
	};
	await writeFile(
		arm.provenance,
		JSON.stringify({
			version: 1,
			...(await sourceIdentity(source)),
			binarySha256: digest(await readFile(binary)),
			buildCommand: ["TEST FIXTURE, NOT PRODUCTION"],
		}),
	);
	return arm;
}
test("provenance rejects changed source and binary rather than trusting a stamp", async () => {
	const root = await temp(),
		arm = await fixtureArm(root);
	await verifyArm(arm);
	await writeFile(join(arm.source, "source.rs"), "changed");
	await expect(verifyArm(arm)).rejects.toThrow("Stale");
});
test("process adapter plumbing produces real filesystem verification and manager rows (fixture executable only)", async () => {
	const root = await temp(),
		arm = await fixtureArm(root),
		input = join(root, "input"),
		expected = join(root, "expected");
	await mkdir(input);
	await mkdir(expected);
	await writeFile(join(input, "answer.txt"), "before");
	await writeFile(join(expected, "answer.txt"), "done");
	const catalog = "[providers.fixture]\nauth = \"none\"\n";
	const modelsPath = join(root, "models.toml");
	await writeFile(modelsPath, catalog);
	const report = await runExperiment({
		version: 1,
		provider: { models: { path: modelsPath, sha256: digest(catalog) } },
		model: "test/snapshot-2026-01-01",
		baseline: arm,
		candidate: arm,
		tasks: [{ id: "one", name: "One", input, expected, prompt: "fix file" }],
		repetitions: 1,
		timeoutMs: 5000,
		output: join(root, "results"),
		verifier: { id: "exact-v1", mode: "exact" },
	});
	expect(report.summaries.baseline!.allRuns.successes).toBe(1);
	expect(report.comparison.success_pct.resolution).toBe("unresolved");
	const row = JSON.parse(
		await readFile(join(root, "results", "baseline", "result.json"), "utf8"),
	);
	expect(row.tasks[0].runs[0].tokens.input).toBe(10);
	for (let ordinal = 0; ordinal < 4; ordinal++)
		expect(await readFile(join(root, "results", `trial-${ordinal}`, "data/models.toml"), "utf8")).toBe(catalog);
});
test("process deadline kills a descendant retaining stdout", async () => {
	const root = await temp();
	const started = performance.now();
	const result = await execute(["/bin/sh", "-c", "sleep 60 & wait"], root, 50);
	expect(result.timedOut).toBe(true);
	expect(performance.now() - started).toBeLessThan(2500);
});

test("guard rejects a changed binary and an older binary with matching hashes", async () => {
	const root = await temp(),
		arm = await fixtureArm(root);
	await writeFile(arm.binary, "changed");
	await expect(verifyArm(arm)).rejects.toThrow("Stale");
	const root2 = await temp(),
		arm2 = await fixtureArm(root2);
	await utimes(arm2.binary, new Date(0), new Date(0));
	await expect(verifyArm(arm2)).rejects.toThrow("older");
	await expect(
		build(arm2.source, arm2.binary, arm2.provenance, ["/usr/bin/true"]),
	).rejects.toThrow("fresh");
});
test("zero cache denominator is unavailable and sub-five-point changes on twenty tasks stay unresolved", () => {
	const base = summarize([run({ input: 0, cacheRead: 0 })], ["one"]);
	expect(base.metrics.cache_hit_ratio).toBeNull();
	const a = summarize([run()], ["one"]),
		c = structuredClone(a);
	c.metrics.success_pct = 96;
	expect(compare(a, a, a, c, 20).success_pct.resolution).toBe("unresolved");
});

test("incomplete telemetry preserves known cost without declaring a ghost or improvement", () => {
	const observed = telemetry(trace);
	expect(() => telemetry("not-json", observed)).toThrow();
	expect(observed.input).toBe(10);
	const partial = run({
		success: false,
		telemetryError: "invalid trace",
		input: 0,
		output: 0,
		cacheRead: 0,
		cacheWrite: 0,
		reads: 0,
		edits: 0,
		writes: 0,
	});
	expect(ghost(partial)).toBe(false);
	const incomplete = summarize([partial], ["one"]),
		complete = summarize([run()], ["one"]);
	expect(incomplete.allRuns.telemetryIncomplete).toBe(1);
	expect(
		compare(complete, complete, incomplete, complete, 1).success_pct.resolution,
	).toBe("unresolved: incomplete telemetry");
});

test("CLI comparison switches validate actual prepared build identities and git refs", async () => {
	const root = await temp(),
		arm = await fixtureArm(root);
	const manifest = { baseline: arm, candidate: arm } as Manifest;
	await validateComparison(manifest, ["--same-commit"]);
	await validateComparison(manifest, ["--base", "HEAD"]);
	await expect(
		validateComparison(manifest, ["--same-commit", "--base", "HEAD"]),
	).rejects.toThrow("exclusively");
	await expect(
		validateComparison(manifest, ["--base", "missing-reference"]),
	).rejects.toThrow();
	const other = await fixtureArm(await temp());
	await writeFile(other.binary, "different executable");
	const proof = JSON.parse(await readFile(other.provenance, "utf8"));
	proof.binarySha256 = digest(await readFile(other.binary));
	await writeFile(other.provenance, JSON.stringify(proof));
	await expect(
		validateComparison({ ...manifest, candidate: other }, ["--same-commit"]),
	).rejects.toThrow("identical");
});

test.skipIf(!process.env.OMP_RULER_SOURCE)(
	"optional RULER verifier uses settled production-shaped answer and keeps all-attempt scores (fixture process only)",
	async () => {
		const root = await temp(),
			arm = await fixtureArm(root),
			input = join(root, "input"),
			expected = join(root, "expected");
		await mkdir(input);
		await mkdir(expected);
		await writeFile(join(expected, "answer.txt"), "done");
		const answerTrace = [
			{
				...message,
				message: {
					...message.message,
					stopReason: "stop",
					content: [
						{ type: "thinking", thinking: "Beta" },
						{ type: "text", text: "ALPHA" },
					],
				},
			},
			{ type: "agent_end", isTerminal: true },
		]
			.map((e) => JSON.stringify(e))
			.join("\n");
		await writeFile(
			arm.binary,
			`#!/bin/sh\nprintf done > answer.txt\ncat <<'TRACE'\n${answerTrace}\nTRACE\n`,
		);
		const proof = JSON.parse(await readFile(arm.provenance, "utf8"));
		proof.binarySha256 = digest(await readFile(arm.binary));
		await writeFile(arm.provenance, JSON.stringify(proof));
		const dataset = join(root, "dataset.jsonl");
		await writeFile(
			dataset,
			JSON.stringify({
				index: 7,
				input: "contract input",
				answer_prefix: "\nAnswer: ",
				outputs: ["Alpha", "Beta"],
			}) + "\n",
		);
		const python = await realpath(
			process.env.OMP_RULER_PYTHON ?? "/usr/bin/python3",
		);
		const manifest: Manifest = {
			version: 1,
			model: "test/snapshot-2026-01-01",
			baseline: arm,
			candidate: arm,
			tasks: [
				{
					id: "one",
					name: "One",
					datasetIndex: 7,
					input,
					expected,
					prompt: "contract input\nAnswer: ",
				},
			],
			repetitions: 1,
			timeoutMs: 5000,
			output: join(root, "results"),
			verifier: { id: "bytes", mode: "exact" },
			answerVerifier: {
				kind: "ruler",
				source: process.env.OMP_RULER_SOURCE!,
				revision: RULER_REVISION,
				python,
				pythonSha256: digest(await readFile(python)),
				family: "niah",
				dataset: {
					path: dataset,
					sha256: digest(await readFile(dataset)),
					origin: "LOCAL CONTRACT TEST ONLY",
					generationCommand: [],
				},
			},
		};
		const report = await runExperiment(manifest);
		expect(report.answerEvaluation!.arms.map((arm) => arm.score)).toEqual([
			50, 50, 50, 50,
		]);
		expect(report.summaries.baseline!.allRuns.successes).toBe(0);
		expect(report.answerEvaluation!.selectedIndices).toEqual([7]);
		await expect(
			runExperiment({
				...manifest,
				output: join(root, "invalid"),
				tasks: [{ ...manifest.tasks[0]!, prompt: "contract input" }],
			}),
		).rejects.toThrow("exactly match");
	},
);


test("trial state isolates native roots and strips ambient context/runtime overrides", async () => {
	const root = await temp();
	const parent = {
		PATH: process.env.PATH, HOME: "/ambient-home", OMP_DATA_DIR: "/ambient-data",
		OMP_PROFILE: "contaminated", CODEX_HOME: "/ambient-codex", PYTHONPATH: "/ambient-python",
		NODE_OPTIONS: "--require /ambient-code", BASH_ENV: "/ambient-shell", XDG_CONFIG_HOME: "/ambient-xdg",
		ANTHROPIC_API_KEY: "fixture-secret-only", OPENAI_API_KEY: "undeclared-fixture-secret",
	};
	const first = await trialEnvironment(join(root, "first"), { env: ["ANTHROPIC_API_KEY"] }, parent);
	await mkdir(join(first.HOME!, ".claude"));
	await writeFile(join(first.HOME!, ".claude/CLAUDE.md"), "FIRST_TRIAL_CONTEXT");
	const second = await trialEnvironment(join(root, "second"), { env: ["ANTHROPIC_API_KEY"] }, parent);
	for (const key of ["HOME", "OMP_CONFIG_DIR", "OMP_DATA_DIR", "OMP_STATE_DIR", "OMP_CACHE_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR", "TMPDIR"]) {
		expect(first[key]).not.toBe(second[key]);
		expect(second[key]!.startsWith(join(root, "second") + "/")).toBe(true);
	}
	expect(await Bun.file(join(second.HOME!, ".claude/CLAUDE.md")).exists()).toBe(false);
	for (const key of ["OMP_PROFILE", "CODEX_HOME", "PYTHONPATH", "NODE_OPTIONS", "BASH_ENV", "OPENAI_API_KEY"]) expect(second[key]).toBeUndefined();
	expect(second.ANTHROPIC_API_KEY).toBe("fixture-secret-only");
	await expect((await import("node:fs/promises")).stat(second.XDG_RUNTIME_DIR!).then(s => s.mode & 0o777)).resolves.toBe(0o700);
	await expect(trialEnvironment(join(root, "bad"), { env: ["OMP_PROFILE"] }, parent)).rejects.toThrow("Unsupported provider");
	await expect(trialEnvironment(join(root, "missing"), { env: ["OMP_ANTHROPIC_API_KEY"] }, parent)).rejects.toThrow("Missing declared");
});

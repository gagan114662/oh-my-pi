import * as adapter from "./adapter";
import { expect, test } from "bun:test";
import {
	chmod,
	mkdir,
	mkdtemp,
	readFile,
	rm,
	writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	digest,
	execute,
	runExperiment,
	sourceIdentity,
	treeHash,
} from "./adapter";

test("exact reference snapshot rejects counterfeit bytes and preserves file inventory", async () => {
	const root = await mkdtemp(join(tmpdir(), "expected-exact-"));
	try {
		const expected = join(root, "expected"),
			actual = join(root, "actual");
		await mkdir(expected);
		await mkdir(actual);
		await writeFile(join(expected, "answer"), "correct");
		await writeFile(join(expected, "missing"), "required");
		const snapshot = await adapter.snapshotExpectedFiles(expected, {
			id: "exact",
			mode: "exact",
		});
		expect(snapshot.hash).toBe(await treeHash(expected));
		await writeFile(join(expected, "answer"), "counterfeit");
		await writeFile(join(actual, "answer"), "counterfeit");
		await writeFile(join(actual, "extra"), "extra");
		expect(await snapshot.verify(actual)).toEqual([
			"Missing: missing",
			"Unexpected: extra",
			"Different: answer",
		]);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test("real runner scores frozen expected data despite candidate alter/restore attack (fixture executable, not model)", async () => {
	const root = await mkdtemp(join(tmpdir(), "expected-runner-"));
	try {
		const source = join(root, "source"),
			input = join(root, "input"),
			expected = join(root, "expected");
		for (const p of [source, input, expected]) await mkdir(p);
		await writeFile(join(source, "source.txt"), "fixture source");
		for (const command of [
			["git", "init", "-q"],
			["git", "add", "."],
			[
				"git",
				"-c",
				"user.name=Fixture",
				"-c",
				"user.email=fixture@example.test",
				"commit",
				"-qm",
				"fixture",
			],
		]) {
			const result = await execute(command, source, 5000);
			expect(result.exitCode).toBe(0);
		}
		const reference = join(expected, "answer.txt");
		await writeFile(reference, "correct");
		await writeFile(join(input, "answer.txt"), "input");
		const binary = join(root, "fixture-command");
		const trace = [
			{
				type: "message_end",
				message: {
					role: "assistant",
					provider: "test",
					model: "snapshot-2026-01-01",
					usage: {
						input: 1,
						output: 1,
						cacheRead: 0,
						cacheWrite: 0,
						cost: { total: 0 },
					},
				},
			},
			{ type: "agent_end", isTerminal: true },
		]
			.map((e) => JSON.stringify(e))
			.join("\n");
		await writeFile(
			binary,
			`#!${process.execPath}\nimport {writeFileSync} from 'node:fs';\nwriteFileSync('answer.txt','counterfeit');\nwriteFileSync(${JSON.stringify(reference)},'counterfeit');\nconsole.log(${JSON.stringify(trace)});\n`,
		);
		await chmod(binary, 0o755);
		const formatter = join(root, "formatter.ts");
		// Actual formatter execution consumes the already-read bytes, then restores
		// the owned reference before post-trial integrity validation. No sleeps.
		await writeFile(
			formatter,
			`import {writeFileSync} from 'node:fs';\nconst input=await Bun.stdin.text();\nwriteFileSync(${JSON.stringify(reference)},'correct');\nprocess.stdout.write(input);\n`,
		);
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
				buildCommand: ["FIXTURE ONLY"],
			}),
		);
		const report = await runExperiment({
			version: 1,
			model: "test/snapshot-2026-01-01",
			baseline: arm,
			candidate: arm,
			tasks: [{ id: "one", name: "One", input, expected, prompt: "fixture" }],
			repetitions: 1,
			timeoutMs: 5000,
			output: join(root, "results"),
			verifier: {
				id: "restore-fixture",
				mode: "formatted",
				command: [process.execPath, formatter],
				executableSha256: digest(await readFile(process.execPath)),
			},
		});
		expect(report.summaries.baseline!.allRuns.successes).toBe(0);
		expect(report.summaries.candidate!.allRuns.successes).toBe(0);
		const runs = JSON.parse(
			await readFile(join(root, "results", "runs.json"), "utf8"),
		);
		expect(runs).toHaveLength(4);
		for (const run of runs)
			expect(run.verification).toEqual(["Different: answer.txt"]);
		expect(await readFile(reference, "utf8")).toBe("correct");
	} finally {
		await rm(root, { recursive: true, force: true });
	}
}, 15000);

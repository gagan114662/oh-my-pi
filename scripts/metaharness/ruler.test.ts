import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execute } from "./adapter";
import {
	finalAnswer,
	rulerDataset,
	scoreRuler,
	RULER_REVISION,
	type RulerSpec,
} from "./ruler";
const trace = (content: any[], stopReason = "stop") =>
	[
		{
			type: "message_end",
			message: {
				role: "assistant",
				stopReason: "toolUse",
				content: [{ type: "text", text: "early answer" }],
			},
		},
		{ type: "tool_execution_end", result: { text: "tool answer" } },
		{
			type: "message_end",
			message: { role: "assistant", stopReason, content },
		},
		{
			type: "agent_end",
			isTerminal: true,
			messages: [
				{
					role: "assistant",
					content: [{ type: "text", text: "duplicated answer" }],
				},
			],
		},
	]
		.map((e) => JSON.stringify(e))
		.join("\n");
test("final answer excludes earlier replies, thinking, tools and agent_end duplicates", () => {
	expect(
		finalAnswer(
			trace([
				{ type: "thinking", thinking: "secret answer" },
				{ type: "text", text: "final " },
				{ type: "text", text: "answer" },
			]),
		),
	).toBe("final answer");
	expect(
		finalAnswer(trace([{ type: "thinking", thinking: "not an answer" }])),
	).toBe("");
});
test("unsettled and errored replies are never scored as final answers", () => {
	for (const reason of ["length", "error", "aborted", "toolUse"])
		expect(() =>
			finalAnswer(trace([{ type: "text", text: "answer" }], reason)),
		).toThrow();
	expect(() => finalAnswer(trace([{ type: "toolCall", id: "x" }]))).toThrow();
	expect(() =>
		finalAnswer(trace([]) + "\n" + JSON.stringify({ type: "message_end" })),
	).toThrow();
});
const officialSource = process.env.OMP_RULER_SOURCE;
test.skipIf(!officialSource)(
	"pinned official RULER scorer executes preprocessing and fractional/QA semantics (contract records, not benchmark results)",
	async () => {
		const root = await mkdtemp(join(tmpdir(), "omp-ruler-contract-"));
		try {
			const dataset = join(root, "records.jsonl");
			await writeFile(
				dataset,
				JSON.stringify({
					index: 7,
					input: "contract input only",
					outputs: ["Alpha", "Beta"],
				}) + "\n",
			);
			const sha = (bytes: Uint8Array) =>
				createHash("sha256").update(bytes).digest("hex");
			const python = await realpath(
				process.env.OMP_RULER_PYTHON ?? "/usr/bin/python3",
			);
			const spec: RulerSpec = {
				kind: "ruler",
				revision: RULER_REVISION,
				source: officialSource!,
				python,
				pythonSha256: sha(await readFile(python)),
				dataset: {
					path: dataset,
					sha256: sha(await readFile(dataset)),
					origin: "LOCAL CONTRACT TEST ONLY; NOT OFFICIAL DATASET",
					generationCommand: [],
				},
				family: "niah",
			};
			expect((await rulerDataset(spec))[0]!.index).toBe(7);
			expect(
				await scoreRuler(
					spec,
					[" ALPHA\u0000Beta "],
					[["alpha", "beta"]],
					execute,
				),
			).toBe(100);
			expect(
				await scoreRuler(spec, ["ALPHA"], [["alpha", "beta"]], execute),
			).toBe(50);
			expect(
				await scoreRuler(
					{ ...spec, family: "qa" },
					["ALPHA"],
					[["alpha", "beta"]],
					execute,
				),
			).toBe(100);
			expect(
				await scoreRuler(spec, ["", "Beta"], [["alpha"], ["beta"]], execute),
			).toBe(50);
			for (const answer_prefix of [null, 42, {}, []]) {
				await writeFile(
					dataset,
					JSON.stringify({
						index: 7,
						input: "question",
						answer_prefix,
						outputs: ["answer"],
					}) + "\n",
				);
				await expect(
					rulerDataset({
						...spec,
						dataset: { ...spec.dataset, sha256: sha(await readFile(dataset)) },
					}),
				).rejects.toThrow("Invalid RULER dataset records");
			}
			await writeFile(dataset, "changed");
			await expect(rulerDataset(spec)).rejects.toThrow("dataset hash mismatch");
		} finally {
			await rm(root, { recursive: true, force: true });
		}
	},
);

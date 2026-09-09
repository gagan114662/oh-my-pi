import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
	cp,
	mkdtemp,
	readFile,
	realpath,
	rm,
	writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execute } from "./adapter";
import { RULER_REVISION, scoreRuler, type RulerSpec } from "./ruler";

const officialSource = process.env.OMP_RULER_SOURCE;
test.skipIf(!officialSource)(
	"scoring executes verified bytes when both source files change before Python starts",
	async () => {
		const root = await mkdtemp(join(tmpdir(), "ruler-source-snapshot-"));
		try {
			const source = join(root, "official");
			await cp(officialSource!, source, { recursive: true });
			const dataset = join(root, "records.jsonl");
			await writeFile(
				dataset,
				JSON.stringify({
					index: 0,
					input: "contract fixture",
					outputs: ["alpha", "beta"],
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
				source,
				python,
				pythonSha256: sha(await readFile(python)),
				dataset: {
					path: dataset,
					sha256: sha(await readFile(dataset)),
					origin: "LOCAL CONTRACT TEST ONLY",
					generationCommand: [],
				},
				family: "niah",
			};
			const paths = [
				"scripts/eval/evaluate.py",
				"scripts/eval/synthetic/constants.py",
			].map((p) => join(source, p));
			const originals = await Promise.all(paths.map((p) => readFile(p)));
			let executions = 0;
			const score = await scoreRuler(
				spec,
				["alpha"],
				[["alpha", "beta"]],
				async (...args) => {
					// This callback is the real executor boundary, after provenance verification.
					// Alter only owned copies, then run the actual Python bridge before restoring.
					executions++;
					try {
						await writeFile(
							paths[0]!,
							"def postprocess_pred(prediction, config):\n    return prediction\n",
						);
						await writeFile(
							paths[1]!,
							"TASKS = {'niah': {'metric_fn': lambda predictions, references: 100}}\n",
						);
						return await execute(...args);
					} finally {
						await Promise.all(
							paths.map((path, i) => writeFile(path, originals[i]!)),
						);
					}
				},
			);
			expect(executions).toBe(1);
			expect(score).toBe(50);
			await writeFile(paths[0]!, "changed before verification");
			await expect(
				scoreRuler(spec, ["alpha"], [["alpha"]], execute),
			).rejects.toThrow("official scorer hash mismatch");
		} finally {
			await rm(root, { recursive: true, force: true });
		}
	},
);

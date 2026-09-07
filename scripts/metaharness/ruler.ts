/** Executes pinned upstream RULER scoring code; never substitutes a local metric. */
import { createHash } from "node:crypto";
import { readFile, realpath } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
export const RULER_REVISION = "c3f5e3b4f87f97e048793bb510a3a6b19a46bf3a";
const SOURCES = {
	"scripts/eval/evaluate.py":
		"b9a5fcbded7209663d97670496c74dbcd6358ddbe3d9887a0c5f0c4f974bfe1c",
	"scripts/eval/synthetic/constants.py":
		"6740467c17b8dc06b6b30f4f97e54ce8de81db0dd879f1538d0b6b5727f4bd5f",
};
export interface RulerSpec {
	kind: "ruler";
	source: string;
	revision: typeof RULER_REVISION;
	python: string;
	pythonSha256: string;
	dataset: {
		path: string;
		sha256: string;
		origin: string;
		generationCommand: string[];
	};
	family:
		| "niah"
		| "variable_tracking"
		| "common_words_extraction"
		| "freq_words_extraction"
		| "qa";
}
export interface RulerRecord {
	index: number;
	input: string;
	outputs: string[];
}
type Executor = (
	command: string[],
	cwd: string,
	timeoutMs: number,
	input?: Uint8Array,
) => Promise<{
	stdout: string;
	stderr: string;
	exitCode: number;
	timedOut: boolean;
	error?: string;
}>;
const hash = (value: Uint8Array) =>
	createHash("sha256").update(value).digest("hex");
async function verifiedInputs(spec: RulerSpec) {
	if (spec.kind !== "ruler" || spec.revision !== RULER_REVISION)
		throw new Error("Unsupported RULER scorer revision");
	for (const path of [spec.source, spec.python, spec.dataset.path])
		if (!isAbsolute(path))
			throw new Error("RULER provenance paths must be absolute");
	if (!spec.dataset.origin || !Array.isArray(spec.dataset.generationCommand))
		throw new Error(
			"Record dataset origin and generation command (empty for a published dataset)",
		);
	const sources: Record<string, string> = {};
	for (const [path, sha] of Object.entries(SOURCES)) {
		const bytes = await readFile(join(spec.source, path));
		if (hash(bytes) !== sha)
			throw new Error(`RULER official scorer hash mismatch: ${path}`);
		sources[path] = bytes.toString("base64");
	}
	if (hash(await readFile(await realpath(spec.python))) !== spec.pythonSha256)
		throw new Error("RULER Python executable hash mismatch");
	const dataset = await readFile(spec.dataset.path);
	if (hash(dataset) !== spec.dataset.sha256)
		throw new Error("RULER dataset hash mismatch");
	return {
		sources,
		dataset,
		provenance: {
			repository: "https://github.com/NVIDIA/RULER",
			revision: RULER_REVISION,
			files: SOURCES,
			pythonSha256: spec.pythonSha256,
			dataset: spec.dataset,
			family: spec.family,
		},
	};
}
export async function rulerProvenance(spec: RulerSpec) {
	return (await verifiedInputs(spec)).provenance;
}
export async function rulerDataset(spec: RulerSpec) {
	const { dataset } = await verifiedInputs(spec);
	const rows = dataset
		.toString("utf8")
		.split("\n")
		.filter((x) => x.trim())
		.map((x) => JSON.parse(x) as RulerRecord);
	if (
		!rows.length ||
		rows.some(
			(r) =>
				!Number.isSafeInteger(r.index) ||
				typeof r.input !== "string" ||
				!Array.isArray(r.outputs) ||
				!r.outputs.length ||
				r.outputs.some((x) => typeof x !== "string" || !x.length),
		)
	)
		throw new Error("Invalid RULER dataset records");
	if (new Set(rows.map((r) => r.index)).size !== rows.length)
		throw new Error("Duplicate RULER dataset indices");
	return rows;
}
/** Only the last settled assistant reply before terminal agent_end, never thinking/tool output. */
export function finalAnswer(trace: string) {
	let last: any,
		terminal = false;
	for (const line of trace.split("\n").filter((x) => x.trim())) {
		const event = JSON.parse(line);
		if (terminal) throw new Error("Events after terminal agent_end");
		if (event.type === "message_end" && event.message?.role === "assistant")
			last = event.message;
		if (event.type === "agent_end" && event.isTerminal === true)
			terminal = true;
	}
	if (
		!terminal ||
		!last ||
		last.stopReason !== "stop" ||
		last.errorMessage ||
		!Array.isArray(last.content)
	)
		throw new Error("No settled final assistant reply");
	if (last.content.some((part: any) => part.type === "toolCall"))
		throw new Error("Final reply contains pending tool calls");
	const parts = last.content.filter((part: any) => part.type === "text");
	if (parts.some((part: any) => typeof part.text !== "string"))
		throw new Error("Malformed final assistant text");
	return parts.map((part: any) => part.text).join("");
}
// The AST extraction executes the upstream function unchanged while avoiding
// evaluate.py's CLI/import side effects (NeMo/pandas and an NLTK auto-download).
const BRIDGE = String.raw`
import ast, base64, json, re, sys
data=json.load(sys.stdin)
evaluate_name='scripts/eval/evaluate.py'
constants_name='scripts/eval/synthetic/constants.py'
module=ast.parse(base64.b64decode(data['sources'][evaluate_name],validate=True),filename=evaluate_name)
functions=[n for n in module.body if isinstance(n, ast.FunctionDef) and n.name=='postprocess_pred']
if len(functions)!=1: raise RuntimeError('Upstream preprocessing function missing')
scope={'re':re}
exec(compile(ast.Module(body=functions,type_ignores=[]),evaluate_name,'exec'),scope)
constants={'__name__':'<run_path>','__file__':constants_name,'__package__':'','__spec__':None,'__cached__':None}
exec(compile(base64.b64decode(data['sources'][constants_name],validate=True),constants_name,'exec'),constants)
metrics=constants['TASKS']
config=metrics[data['family']]
predictions=[scope['postprocess_pred'](x,config) for x in data['predictions']]
print(json.dumps({'score':config['metric_fn'](predictions,data['references'])}))
`;
export async function scoreRuler(
	spec: RulerSpec,
	predictions: string[],
	references: string[][],
	execute: Executor,
) {
	// Retain exactly the bytes whose digests passed; Python never reopens sources.
	const { sources } = await verifiedInputs(spec);
	if (!predictions.length || predictions.length !== references.length)
		throw new Error("RULER score requires aligned nonempty rows");
	const result = await execute(
		[spec.python, "-I", "-c", BRIDGE],
		spec.source,
		30_000,
		new TextEncoder().encode(
			JSON.stringify({ family: spec.family, predictions, references, sources }),
		),
	);
	if (result.exitCode !== 0 || result.timedOut || result.error)
		throw new Error(
			`Official RULER scorer failed: ${result.error ?? result.stderr}`,
		);
	const score = JSON.parse(result.stdout).score;
	if (
		typeof score !== "number" ||
		!Number.isFinite(score) ||
		score < 0 ||
		score > 100
	)
		throw new Error("Invalid official RULER score");
	await rulerProvenance(spec);
	return score as number;
}

# Python design corpus implementation status

The numbered documents in this directory preserve extension design decisions,
proposed interfaces, and historical observations from earlier implementations.
They are not a generated inventory of the currently exported Python API.
Accepted requirements remain requirements even where the runtime is incomplete.

For the current surface, inspect `crates/py/python/omp`,
`crates/py/python/omp_remote.py`, their contract tests in `crates/py/tests`, and
host integration in `crates/envd/src/exthost`. Tracked architecture decisions and
explicit gaps are in `docs/adr`; the current ownership map is
[workspace crate architecture](../architecture/crates.md).

## Known differences

- Python Directors exist as `omp.extensions.director`, bridged by `PyDirector`
  in `crates/envd/src/exthost/extensions.rs`. The proposed `omp.regimes` API and
  `omp.regime` decorator are not exported; the regimes reference describes a
  target interface, not an implementation.
- `omp_remote.remote` packages callable work using pickle/source/code mechanisms.
  Optional `pyo3-introspection` supports stub generation; it is not AST-based
  remote packaging.
- Inference belongs to `crates/ai`; shell parsing/runtime to `crates/shell`;
  telemetry to `crates/observability`; extension-host supervision to
  `crates/envd/src/exthost`; grep authority to `crates/envd/src/grep.rs`.
- Historical `crates/storage` transcript APIs were replaced by `crates/journal`,
  `crates/session`, and `crates/dom`. This is an ownership change, not a direct
  rename of every old symbol. Historical storage/BLAKE3 statements do not define
  current journal artifact addresses, which use SHA-256.
- `PLAN.md` and `.plan/` references identify private historical planning material
  that is gitignored and unavailable in a clean clone. They are not checkout
  prerequisites. Preserve their design provenance without treating their paths,
  line numbers, or closure claims as current implementation evidence.
- `scripts/check-docs-surface.py` is not currently wired into a recipe or CI and
  scans numbered documents 00–14 only. Its historical zero-drift claim cannot
  establish coverage for 15–17 or `docs/pyx`.

Older line numbers and proposed "New module" sections are historical evidence.
Validate a referenced symbol against the current source before using a snippet
or claiming a feature is implemented; missing behavior remains a tracked gap.

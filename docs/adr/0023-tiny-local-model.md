# 0023. An embedded tiny model handles harness chores

Status: accepted
Date: 2026-09-02
Area: inference

## Context

A harness generates a steady stream of small language tasks that have nothing to do with the user's
problem: classify this prompt's difficulty, title this session, translate this notice, judge whether
the user is getting frustrated, transcribe this microphone buffer, speak this reply. Sending each of
these to the frontier model pays frontier latency and frontier cost for a 300-token job, and does so
on every turn.

Sub-billion-parameter models (the playbook names LiquidAI's LFM2 line) answer these tasks well
enough when their output is constrained to a small ladder, and local speech models already reach
state-of-the-art quality for TTS and STT. Even a harness that only ever talks to frontier models
benefits from carrying one.

## Decision

The harness MUST embed a tiny local model as an internal capability, and route classification,
title generation, translation, sentiment, and local TTS/STT through it by default.

1. The tiny model is NEVER a second agent. It has no tools, no session, and no place in the
   transcript. It is a bounded internal operation with a fixed output ladder and earliest-match
   parsing, never free prose that a downstream parser has to trust.
2. It runs in-process under the same admission, memory, cancellation, and idle-unload lifecycle as
   any other local inference, with verified, root-confined model artifacts.
3. A pinned tiny model is NEVER silently promoted to a hosted one (cost leak, privacy incident);
   fallback is an explicit caller policy.
4. Local ML runs on Rust-native runtimes (candle); C/C++ binding graphs (whisper-rs, llama-cpp) are
   prohibited (`AGENTS.md`, Runtime).

## Consequences

- Session titles, auto-thinking difficulty, memory classification, and voice cost no frontier
  tokens and add no round trip.
- A chore that would otherwise need a hosted call still works offline and in the Factorio mode
  (0001) where no interactive user is waiting.
- Cost accepted: model artifacts are downloaded and verified once; the harness carries a local
  inference runtime and its memory reservation.

## Status in omp

**Partial.** Local inference components exist in `crates/ai/src/local/`.
`crates/ai/src/local/{tiny_catalog,title}.rs` supplies revision-pinned GGUF
artifact metadata and title validation; catalog entries are not executable
text-generation engines. `crates/ai/src/local/{stt,parakeet}.rs` implements
speech recognition, `crates/ai/src/local/tts/kokoro/` implements speech synthesis,
and `crates/ai/src/local/embedding.rs` implements local embeddings. These engines
are feature-gated. `crates/ai/src/local/{runtime,artifact}.rs` owns shared
admission, memory reservations, cancellation, and verified artifact lifecycle.

`crates/ai/src/local/applefm.rs` and `crates/ai/src/local/applefm/` provide a
dynamically loaded Apple Foundation Models bridge. The framework requires
macOS 26 or later and an eligible Apple Intelligence-enabled device; OMP's
availability path additionally restricts generation to Apple Silicon
(`aarch64`). The `x86_64.s` file is ABI support code, not proof that Intel Macs
can run Apple's model. Framework/model availability is checked at runtime.
See [Apple's framework availability announcement](https://www.apple.com/ca/newsroom/2025/09/apples-foundation-models-framework-unlocks-new-intelligent-app-experiences/).

Gap: the curated GGUF catalog is not connected to a native text-generation
executor for title, memory, and classifier chores. This does not establish the
decision's default-local routing for classification, titles, translation, and
sentiment, or its offline/no-hosted-fallback guarantees. The existing speech,
embedding, and Apple framework engines do not by themselves close that gap.

## References

- The Harness Playbook, "The inference" — "Use small local models for harness work"
- LiquidAI LFM2 (`huggingface.co/LiquidAI`)
- 0001, 0018
- `crates/ai/src/local/`, `crates/ai/Cargo.toml`, `AGENTS.md` (Runtime),
  `docs/py/13-inference.md`

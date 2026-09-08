# 0037. End-relative reads and the video capability boundary

Status: accepted
Date: 2026-09-08
Area: tools

## Context

The v1 reader accepts `:-N`, including `:raw:-N` and `:-N:raw`, and resolves it
against the source line count before applying ordinary line selection. Rust
previously treated that suffix as part of a path or failed to select it. Parsing
alone would be insufficient: internal resolvers and artifact pagination must
also find the end of the source rather than silently return its head.

V1 also invokes system ffmpeg/ffprobe for video preview grids, metadata, frames,
and timestamps. The Rust reader has no video decoder or supervised video
extraction authority. Issue #31 explicitly permits documenting this omission.

## Decision

Represent tails explicitly until a source's actual line count is available.
Resolve tails into inclusive absolute ranges, clamping requests longer than the
source to its beginning. Preserve ordinary numbered-range context, snapshot
visibility, and raw-mode terminal-newline semantics. Resolve directory tails
against rendered listing rows (archives use immediate entry counts), preserving
existing listing limits and truncation diagnostics. Immutable artifacts use the
existing bounded streaming index and fetch only selected byte windows thereafter.

Share tail resolution with internal-resource slicing and the frozen Python URL
parser. Grep explicitly rejects tails because search roots require absolute line
bounds; it must not silently widen a tail into a whole-resource search.

Document video as unsupported in the model-visible read tool description,
including preview grids, metadata, frame numbers, and timestamps. Extraction
with an external video tool followed by an ordinary image/text read is the
available route. Implementing supervised extraction, decode limits, cancellation,
and image-grid rendering is separate work; do not imply that it exists.

## Verification

Regression tests cover selector bounds/compounds and literal-path precedence;
200,000-line local and artifact reads; snapshot visibility; raw/numbered,
empty/short/trailing-newline cases; archive members/listings; HTTP text; internal
resource slicing; and frozen Python parsing/artifact reads. Existing range tests
remain in place. These tests require the repository's supported build environment.
Browser-reviewable CI summaries and uploaded evidence remain required by #31;
this decision does not replace that evidence or declare an unexecuted test passed.

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
and timestamps. Although issue #31 permits documenting an omission, roadmap
#41 Appendix E requires a timestamp-extracted video frame as the stronger demo.

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

Provide local video extraction through the existing `ReadSources` environment
boundary. `omp-tools` owns the v1 frame/timestamp grammar and PNG/blob projection;
`omp-envd` owns ffprobe/ffmpeg processes. Programs receive direct argv entries,
never shell text. The demuxer is pinned from the container extension (MOV for
MP4/MOV/M4V, Matroska for MKV/WebM, AVI for AVI, ASF for WMV). Playlist autodetection
is disabled; MOV external data references and absolute aliases are disabled.
Only fd/pipe protocols are permitted, so media cannot open filesystem or network
references. Output readers
cap metadata at 256 KiB, PNG at 8 MiB, and stderr at 64 KiB. The entire operation
uses one shared 30-second deadline for probing and decoding. Children are killed
on cancellation; a bound or deadline failure kills and reaps the child with a
separate maximum 2-second cleanup allowance. Cleanup timeout is a distinct fault. Frame indices are zero-based, integer
selectors remain distinct from timestamps, and a selector-free read produces a
3x3 grid plus duration, dimensions, codecs, frame rate and container metadata.

The existing read-source authority resolves the authored path; video does not
introduce a separate workspace-access policy or grant additional paths. The host
opens that canonical source through retained directory descriptors with NOFOLLOW
on every component and verifies a regular file. The same read-only descriptor is
passed as stdin to both utilities; neither utility reopens the pathname. This
pins file identity across renames and symlink replacement. Length/mtime changes
on that held inode reject the result; this is not a cryptographic immutable
snapshot against an actor deliberately restoring metadata after in-place writes.
The descriptor protocol requires a supporting ffmpeg/ffprobe build and Unix host.

Local video extensions match v1. Remote video ingestion and attachment-time
preview generation are outside this read-tool change. Missing ffmpeg/ffprobe,
corrupt metadata, out-of-range selections, utility failures and output bounds
produce typed video faults, never successful text fallbacks. Internal subprocess
I/O/JSON sources remain typed until the serialized tool-fault boundary.

## Status in omp

**Partial.** Tail selection is implemented in `crates/tools/src/read/selector.rs`
and the source resolvers. `crates/tools/src/read/video.rs` owns video selectors
and projection; `crates/envd/src/tool_video.rs` owns confined media subprocesses.
Gap: passing production and hosted evidence for the combined implementation is
still required; the verification below describes required coverage, not a claim
that the integrated revision has passed it.

## Verification

Regression tests cover selector bounds/compounds and literal-path precedence;
200,000-line local and artifact reads; snapshot visibility; raw/numbered,
empty/short/trailing-newline cases; archive members/listings; HTTP text; internal
resource slicing; and frozen Python parsing/artifact reads. Existing range tests
remain in place. These tests require the repository's supported build environment.
Browser-reviewable CI summaries and uploaded evidence remain required by #31;
this decision does not replace that evidence or declare an unexecuted test passed.

Video verification adds normal grammar/bounds and bounded-reader Rust tests.
Real video execution is required by the production QA; it adds no ignored tests
and requires the actual production `read` result to deliver PNG data to a
vision-capable local mock provider. Generated fixtures cover frame/time parity,
preview dimensions, the exact `1h5m42s` demo timestamp, corrupt input and
out-of-range seeks. Raw request captures and decoded PNG files remain evidence;
source presence or direct ffmpeg command validation does not prove Rust runtime
execution. Production and hosted execution must pass before claiming completion.

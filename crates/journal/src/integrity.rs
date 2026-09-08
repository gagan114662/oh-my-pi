//! Exact physical-frame SHA-256 chaining, separate from semantic `prior` links.
//!
//! Unkeyed checksums detect edits without recomputed seals, not a malicious
//! writer who replaces the entire chain. An independently retained tip can
//! additionally detect a rewritten or shortened history.

use std::io::Write as _;

use omp_core::Hash32;
use serde::Serialize;
use thiserror::Error;

use crate::{
	Entry, EntryId,
	sse::{self, SseError},
};

const PREFIX: &[u8] = b"integrity: sha256-v1 ";
const DOMAIN: &[u8] = b"omp/journal/physical-frame/sha256-v1\0";

/// Inventory of a complete prefix; legacy frames are never called verified.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Verification {
	/// Frames with matching physical seals.
	pub sealed_entries:  usize,
	/// Historical frames without seals. Such journals cannot be folded or
	/// appended.
	pub legacy_entries:  usize,
	/// Complete unsealed bytes, including legacy spacer blocks.
	pub legacy_bytes:    usize,
	/// Byte immediately after the last complete frame.
	pub committed_bytes: usize,
	/// Incomplete final bytes, excluded from verification and recoverable on
	/// open.
	pub torn_tail_bytes: usize,
	/// Verified final seal, absent for empty or legacy-only journals.
	pub tip:             Option<Hash32>,
}

/// First physically divergent frame. The identity is absent if its bytes no
/// longer contain a parseable id; `offset` always names the frame start.
#[derive(Debug, Error)]
#[error("journal integrity failure at entry {id:?}, byte {offset}: {reason}")]
pub struct IntegrityError {
	/// Best-effort identity read from the divergent physical frame.
	pub id:     Option<EntryId>,
	/// Physical start of that frame, or committed EOF for a tip mismatch.
	pub offset: usize,
	/// Typed cause, not a claim about who changed the file.
	pub reason: IntegrityFailure,
}

/// A physical checksum or framing discrepancy.
#[derive(Debug, Error)]
pub enum IntegrityFailure {
	/// Seal header is malformed, duplicated or misplaced.
	#[error("malformed physical seal")]
	MalformedSeal,
	/// Sealed and unsealed frames cannot form a verified chain.
	#[error("sealed and legacy unsealed frames are mixed")]
	MixedLegacy,
	/// The predecessor field does not match the previous physical frame.
	#[error("predecessor digest mismatch")]
	Predecessor,
	/// Exact frame bytes do not match their stored digest.
	#[error("frame digest mismatch")]
	Digest,
	/// An independently retained expected tip differs from the file.
	#[error("externally expected tip mismatch")]
	ExpectedTip,
}

fn digest(previous: Hash32, body: &[u8]) -> Hash32 {
	let mut hash = Hash32::hasher();
	hash.update(DOMAIN).update(previous.as_bytes()).update(body);
	hash.finalize()
}

/// Encodes a complete sealed frame without changing the logical entry codec.
/// The digest commits to the predecessor and every byte of canonical SSE body,
/// including its blank-line commit delimiter.
pub(crate) fn encode(
	entry: &Entry,
	previous: Hash32,
	output: &mut Vec<u8>,
) -> Result<Hash32, SseError> {
	let mut body = Vec::with_capacity(entry.data.len() + 160);
	sse::encode(entry, &mut body)?;
	let seal = digest(previous, &body);
	let _ = writeln!(output, "integrity: sha256-v1 {previous} {seal}");
	output.extend_from_slice(&body);
	Ok(seal)
}

fn entry_id(raw: &[u8]) -> Option<EntryId> {
	raw.split(|byte| *byte == b'\n').find_map(|line| {
		let value = line.strip_prefix(b"id: ")?;
		std::str::from_utf8(value)
			.ok()?
			.trim_end_matches('\r')
			.parse()
			.ok()
	})
}

/// Verifies complete physical frames before payload parsing or semantic fold.
/// Legacy-only files are inspectable, but reported as wholly unverified.
///
/// # Errors
/// Returns the first divergent physical frame and its byte offset.
pub fn verify_bytes(
	bytes: &[u8],
	expected_tip: Option<Hash32>,
) -> Result<Verification, IntegrityError> {
	let mut report = Verification::default();
	let mut previous = Hash32::default();
	let mut first_legacy = None;
	let mut last_id = None;
	while let Some(end) = sse::complete_block_end(bytes, report.committed_bytes) {
		let start = report.committed_bytes;
		let raw = &bytes[start..end];
		let id = entry_id(raw);
		let fail = |reason| IntegrityError { id, offset: start, reason };
		if !raw.starts_with(PREFIX) {
			if raw
				.split(|byte| *byte == b'\n')
				.any(|line| line.starts_with(b"integrity:"))
			{
				return Err(fail(IntegrityFailure::MalformedSeal));
			}
			if report.sealed_entries != 0 {
				return Err(fail(IntegrityFailure::MixedLegacy));
			}
			// Preserve the legacy scanner's treatment of empty spacer blocks.
			if raw != b"\n" && raw != b"\r\n" {
				report.legacy_entries += 1;
			}
			report.legacy_bytes += raw.len();
			first_legacy.get_or_insert((id, start));
		} else {
			if let Some((id, offset)) = first_legacy {
				return Err(IntegrityError { id, offset, reason: IntegrityFailure::MixedLegacy });
			}
			let newline = raw
				.iter()
				.position(|byte| *byte == b'\n')
				.ok_or_else(|| fail(IntegrityFailure::MalformedSeal))?;
			let header = &raw[PREFIX.len()..newline];
			let body = &raw[newline + 1..];
			if header.len() != 129
				|| header[64] != b' '
				|| header
					.iter()
					.enumerate()
					.any(|(index, byte)| index != 64 && !matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
				|| body
					.split(|byte| *byte == b'\n')
					.any(|line| line.starts_with(b"integrity:"))
			{
				return Err(fail(IntegrityFailure::MalformedSeal));
			}
			let parse = |raw| {
				std::str::from_utf8(raw)
					.ok()
					.and_then(|text| text.parse::<Hash32>().ok())
			};
			let parent = parse(&header[..64]).ok_or_else(|| fail(IntegrityFailure::MalformedSeal))?;
			let seal = parse(&header[65..]).ok_or_else(|| fail(IntegrityFailure::MalformedSeal))?;
			if parent != previous {
				return Err(fail(IntegrityFailure::Predecessor));
			}
			if seal != digest(parent, body) {
				return Err(fail(IntegrityFailure::Digest));
			}
			previous = seal;
			report.tip = Some(seal);
			report.sealed_entries += 1;
		}
		report.committed_bytes = end;
		last_id = id;
	}
	report.torn_tail_bytes = bytes.len() - report.committed_bytes;
	if expected_tip.is_some() && expected_tip != report.tip {
		return Err(IntegrityError {
			id:     last_id,
			offset: report.committed_bytes,
			reason: IntegrityFailure::ExpectedTip,
		});
	}
	Ok(report)
}

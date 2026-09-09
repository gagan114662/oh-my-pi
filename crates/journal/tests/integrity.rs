//! Physical integrity must not depend on semantic branch selection.
use std::fs;

use omp_core::Str;
use omp_journal::{
	EntryDraft, Journal, JournalError, Kind, KindName, gc,
	integrity::{IntegrityFailure, verify_bytes},
	live_chain,
	sse::Scanner,
};

fn draft(
	kind: KindName,
	by: Option<omp_journal::EntryId>,
	prior: Option<omp_journal::EntryId>,
) -> EntryDraft {
	EntryDraft {
		kind: Kind::known(kind),
		by,
		prior,
		label: None,
		data: Str::new_static(r#"{"value":"alpha"}"#),
	}
}

fn fixture(path: &std::path::Path) -> Vec<omp_journal::Entry> {
	let mut journal = Journal::create(path).expect("create");
	let first = journal
		.append(draft(KindName::Journal, None, None))
		.expect("genesis");
	let second = journal
		.append(draft(KindName::TurnStart, Some(first.id), None))
		.expect("turn");
	let third = journal
		.append(draft(KindName::ToolResult, Some(second.id), None))
		.expect("result");
	vec![first, second, third]
}

fn spans(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
	let mut scanner = Scanner::new(bytes);
	let mut result = Vec::new();
	while let Some(frame) = scanner.next() {
		result.push(frame.expect("frame").span);
	}
	result
}

#[test]
fn valid_json_edits_in_middle_and_final_frames_name_exact_frame_before_decode() {
	let directory = tempfile::tempdir().expect("tempdir");
	let path = directory.path().join("source.oms");
	let entries = fixture(&path);
	let bytes = fs::read(&path).expect("read");
	let frames = spans(&bytes);
	let good = Journal::verify_path(&path, None).expect("verify");
	assert_eq!(good.sealed_entries, 3);
	assert_eq!(good.legacy_entries, 0);
	assert_eq!(good.torn_tail_bytes, 0);
	for index in [1, 2] {
		let mut edited = bytes.clone();
		let local = edited[frames[index].clone()]
			.windows(5)
			.position(|value| value == b"alpha")
			.expect("payload");
		edited[frames[index].start + local] = b'o';
		fs::write(&path, &edited).expect("edit");
		for error in [
			Journal::verify_path(&path, None).expect_err("verify rejects"),
			Journal::open(&path).expect_err("open rejects"),
		] {
			let JournalError::Integrity(error) = error else {
				panic!("expected physical integrity error")
			};
			assert_eq!(error.id, Some(entries[index].id));
			assert_eq!(error.offset, frames[index].start);
			assert!(matches!(error.reason, IntegrityFailure::Digest));
		}
		assert_eq!(
			fs::read(&path).expect("unchanged"),
			edited,
			"failed verification must not truncate or repair"
		);
	}
}

#[test]
fn insertion_deletion_and_reordering_fail_at_first_physical_divergence() {
	let directory = tempfile::tempdir().expect("tempdir");
	let path = directory.path().join("source.oms");
	let entries = fixture(&path);
	let bytes = fs::read(&path).expect("read");
	let frames = spans(&bytes);
	for (order, bad_index, id) in [
		([0, 1, 1, 2].as_slice(), 2, entries[1].id),
		([0, 2].as_slice(), 1, entries[2].id),
		([0, 2, 1].as_slice(), 1, entries[2].id),
	] {
		let changed: Vec<u8> = order
			.iter()
			.flat_map(|index| bytes[frames[*index].clone()].iter().copied())
			.collect();
		let error = verify_bytes(&changed, None).expect_err("physical predecessor differs");
		assert_eq!(error.id, Some(id));
		assert_eq!(
			error.offset,
			order[..bad_index]
				.iter()
				.map(|index| frames[*index].len())
				.sum::<usize>()
		);
		assert!(matches!(error.reason, IntegrityFailure::Predecessor));
	}
}

#[test]
fn torn_tail_reports_valid_prefix_and_external_tip_detects_complete_suffix_loss() {
	let directory = tempfile::tempdir().expect("tempdir");
	let path = directory.path().join("source.oms");
	fixture(&path);
	let bytes = fs::read(&path).expect("read");
	let frames = spans(&bytes);
	let tip = verify_bytes(&bytes, None).expect("valid").tip;
	for cut in frames[2].start..frames[2].end {
		let report = verify_bytes(&bytes[..cut], None).expect("valid prefix");
		assert_eq!(report.sealed_entries, 2);
		assert_eq!(report.committed_bytes, frames[2].start);
		assert_eq!(report.torn_tail_bytes, cut - frames[2].start);
		assert!(matches!(
			verify_bytes(&bytes[..cut], tip)
				.expect_err("expected full history")
				.reason,
			IntegrityFailure::ExpectedTip
		));
	}
	fs::write(&path, &bytes[..bytes.len() - 1]).expect("torn write");
	let (opened, entries) = Journal::open(&path).expect("recover prefix");
	assert_eq!(entries.len(), 2);
	assert_eq!(opened.recovered_tail_bytes() as usize, frames[2].len() - 1);
	assert_eq!(opened.verify().expect("recovered seal").sealed_entries, 2);
}

#[test]
fn branch_prior_is_not_physical_predecessor_and_prune_reseals_verified_input() {
	let directory = tempfile::tempdir().expect("tempdir");
	let path = directory.path().join("branch.oms");
	let entries = fixture(&path);
	let (mut journal, _) = Journal::open(&path).expect("open");
	let branch = journal
		.append(draft(KindName::TurnStart, Some(entries[0].id), Some(entries[0].id)))
		.expect("branch");
	let before_tip = journal.verify().expect("branch verified").tip;
	drop(journal);
	assert_eq!(
		live_chain(&Journal::scan(&path).expect("scan"))
			.map(|entry| entry.id)
			.collect::<Vec<_>>(),
		vec![entries[0].id, branch.id]
	);
	gc::prune_abandoned(&path).expect("prune verified history");
	let after = Journal::verify_path(&path, None).expect("pruned chain");
	assert_eq!(after.sealed_entries, 2);
	assert_ne!(after.tip, before_tip, "legitimate rewrite produces a new tip");
	assert_eq!(
		Journal::scan(&path)
			.expect("scan")
			.last()
			.expect("branch")
			.id,
		branch.id
	);
}

#[test]
fn legacy_is_inspectable_but_cannot_be_appended_or_laundered_by_gc() {
	let directory = tempfile::tempdir().expect("tempdir");
	let path = directory.path().join("legacy.oms");
	let entries = fixture(&path);
	let mut legacy = Vec::new();
	for entry in &entries {
		omp_journal::sse::encode(entry, &mut legacy).expect("legacy codec");
	}
	fs::write(&path, &legacy).expect("legacy");
	let report = Journal::verify_path(&path, None).expect("legacy inspection");
	assert_eq!(report.sealed_entries, 0);
	assert_eq!(report.legacy_entries, 3);
	assert_eq!(report.tip, None);
	let (mut journal, _) = Journal::open(&path).expect("inspectable legacy");
	assert!(matches!(
		journal.append(draft(KindName::TurnStart, Some(entries[0].id), None)),
		Err(JournalError::LegacyUnsealed { entries: 3, .. })
	));
	drop(journal);
	assert!(matches!(
		gc::prune_abandoned(&path),
		Err(gc::GcError::Journal(JournalError::LegacyUnsealed { entries: 3, .. }))
	));
	assert_eq!(fs::read(&path).expect("read"), legacy);
}

#[test]
fn duplicate_misplaced_or_noncanonical_seals_are_not_ignored_sse_extensions() {
	let directory = tempfile::tempdir().expect("tempdir");
	let path = directory.path().join("source.oms");
	let entries = fixture(&path);
	let bytes = fs::read(&path).expect("read");
	let newline = bytes
		.iter()
		.position(|byte| *byte == b'\n')
		.expect("header");
	let mut duplicate = bytes[..=newline].to_vec();
	duplicate.extend_from_slice(&bytes);
	let mut misplaced = b": comment before seal\n".to_vec();
	misplaced.extend_from_slice(&bytes);
	let mut noncanonical = bytes.clone();
	let letter = noncanonical[..newline]
		.iter()
		.rposition(|byte| (b'a'..=b'f').contains(byte))
		.expect("hex letter");
	noncanonical[letter].make_ascii_uppercase();
	for changed in [duplicate, misplaced, noncanonical] {
		let error = verify_bytes(&changed, None).expect_err("seal metadata is mandatory");
		assert_eq!(error.id, Some(entries[0].id));
		assert_eq!(error.offset, 0);
		assert!(matches!(error.reason, IntegrityFailure::MalformedSeal));
	}
}

#[test]
fn legacy_spacers_and_expected_tip_fail_before_destructive_recovery() {
	let directory = tempfile::tempdir().expect("tempdir");
	let path = directory.path().join("source.oms");
	fixture(&path);
	let original = fs::read(&path).expect("read");
	let tip = verify_bytes(&original, None).expect("verify").tip;
	let torn = &original[..original.len() - 1];
	fs::write(&path, torn).expect("write");
	assert!(
		matches!(Journal::open_verified(&path, tip), Err(JournalError::Integrity(error)) if matches!(error.reason, IntegrityFailure::ExpectedTip))
	);
	assert_eq!(fs::read(&path).expect("not truncated"), torn);
	fs::write(&path, b"\npartial").expect("legacy spacer");
	assert!(matches!(
		Journal::open_verified(&path, None),
		Err(JournalError::LegacyUnsealed { entries: 0, bytes: 1 })
	));
	assert_eq!(fs::read(&path).expect("not truncated"), b"\npartial");
}

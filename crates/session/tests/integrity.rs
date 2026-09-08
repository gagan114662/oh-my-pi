//! Session opening must reject physical edits before reconstructing state.
use std::fs;

use omp_journal::{Journal, JournalError, integrity::IntegrityFailure};
use omp_session::{ComponentRegistry, Session, SessionError};

#[test]
fn session_open_refuses_corruption_and_legacy_without_repairing_bytes() {
	let directory = tempfile::tempdir().expect("tempdir");
	let path = directory.path().join("session.oms");
	let mut session = Session::create(&path, ComponentRegistry::default()).expect("create");
	session.begin_turn().expect("turn");
	drop(session);
	let original = fs::read(&path).expect("bytes");
	let entries = Journal::scan(&path).expect("entries");
	Session::open(&path, ComponentRegistry::default()).expect("valid replay");
	let mut edited = original.clone();
	let start = edited
		.windows(10)
		.position(|bytes| bytes == b"\"version\":")
		.expect("genesis version");
	edited[start + 10] = b'2';
	fs::write(&path, &edited).expect("edit");
	let error = Session::open(&path, ComponentRegistry::default())
		.err()
		.expect("must refuse forged bytes");
	assert!(
		matches!(error, SessionError::Journal(JournalError::Integrity(error)) if error.id == Some(entries[0].id) && error.offset == 0 && matches!(error.reason, IntegrityFailure::Digest))
	);
	assert_eq!(fs::read(&path).expect("unchanged"), edited);
	let mut legacy = Vec::new();
	for entry in &entries {
		omp_journal::sse::encode(entry, &mut legacy).expect("encode legacy");
	}
	fs::write(&path, &legacy).expect("legacy");
	let error = Session::open(&path, ComponentRegistry::default())
		.err()
		.expect("unsealed history is not trusted");
	assert!(matches!(error, SessionError::Journal(JournalError::LegacyUnsealed { .. })));
	assert_eq!(fs::read(&path).expect("unchanged legacy"), legacy);
}

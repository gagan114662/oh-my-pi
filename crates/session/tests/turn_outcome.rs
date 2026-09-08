//! Durable terminal identity, retry, rewind, and crash-window accounting.

use std::io::Write;

use omp_dom::Handle;
use omp_journal::{
	Journal,
	data::{TurnOutcome, TurnStatus},
};
use omp_session::{ComponentRegistry, Session, SessionError};

fn turn(session: &Session) -> Handle {
	session
		.dom()
		.children(session.dom().body())
		.last()
		.copied()
		.expect("turn")
}

#[test]
fn terminal_identity_is_idempotent_and_rewind_reopens_only_selected_turn() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("turn.oms");
	let mut session = Session::create(&path, ComponentRegistry::default()).unwrap();
	let start = session.begin_turn().unwrap();
	let handle = turn(&session);
	let before = session.user("one", Vec::new()).unwrap();
	let done = session.finish_turn(handle, TurnStatus::Completed).unwrap();
	let count = session.entry_count();
	assert_eq!(session.finish_turn(handle, TurnStatus::Completed).unwrap(), done);
	assert_eq!(session.entry_count(), count, "duplicate finish must not append");
	assert!(matches!(
		session.finish_turn(handle, TurnStatus::Failed),
		Err(SessionError::ConflictingTurnOutcome)
	));
	assert_eq!(session.entry(done).unwrap().by, Some(start));
	session.rewind(before).unwrap();
	let cancelled = session
		.finish_turn(turn(&session), TurnStatus::Cancelled)
		.unwrap();
	assert_eq!(session.entry(cancelled).unwrap().prior, Some(before));
	assert_eq!(session.entry(cancelled).unwrap().by, Some(start));
	drop(session);
	let mut reopened = Session::open(&path, ComponentRegistry::default()).unwrap();
	assert_eq!(reopened.head(), Some(cancelled));
	assert_eq!(
		reopened
			.finish_turn(turn(&reopened), TurnStatus::Cancelled)
			.unwrap(),
		cancelled
	);
	assert!(matches!(
		reopened.finish_turn(turn(&reopened), TurnStatus::Completed),
		Err(SessionError::ConflictingTurnOutcome)
	));
	let stale = turn(&reopened);
	reopened.begin_turn().unwrap();
	assert!(matches!(
		reopened.finish_turn(stale, TurnStatus::Completed),
		Err(SessionError::TurnChanged)
	));
}

#[test]
fn crash_before_terminal_record_never_acquires_success_on_reopen() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("crash.oms");
	let mut session = Session::create(&path, ComponentRegistry::default()).unwrap();
	session.begin_turn().unwrap();
	session.user("incomplete", Vec::new()).unwrap();
	let last = session.head();
	drop(session);
	// A killed writer may leave a torn terminal frame; recovery drops it.
	std::fs::OpenOptions::new()
		.append(true)
		.open(&path)
		.unwrap()
		.write_all(b"event: turn.outcome@1\nid: torn")
		.unwrap();
	let session = Session::open(&path, ComponentRegistry::default()).unwrap();
	assert_eq!(session.head(), last);
	drop(session);
	let (_, entries) = Journal::open(&path).unwrap();
	assert!(
		entries
			.iter()
			.all(|entry| entry.kind.name != "turn.outcome")
	);
}

#[test]
fn all_terminal_statuses_survive_reopen_without_inference_receipts() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("statuses.oms");
	let mut session = Session::create(&path, ComponentRegistry::default()).unwrap();
	let states = [
		TurnStatus::Completed,
		TurnStatus::Incomplete,
		TurnStatus::Failed,
		TurnStatus::Cancelled,
		TurnStatus::Steered,
	];
	for status in states {
		session.begin_turn().unwrap();
		session.finish_turn(turn(&session), status).unwrap();
	}
	drop(session);
	let reopened = Session::open(&path, ComponentRegistry::default()).unwrap();
	drop(reopened);
	let (_, entries) = Journal::open(&path).unwrap();
	let outcomes: Vec<TurnStatus> = entries
		.iter()
		.filter(|entry| entry.kind.name == "turn.outcome")
		.map(|entry| {
			serde_json::from_str::<TurnOutcome>(entry.data.as_str())
				.unwrap()
				.status
		})
		.collect();
	assert_eq!(outcomes, states);
}

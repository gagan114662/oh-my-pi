//! Durable context protection uses the same journal and branch fold as turns.

use omp_core::Str;
use omp_journal::data::Compaction;
use omp_session::{
	ComponentRegistry, Session, SessionError,
	context::{ContextPinError, context_pins},
};

#[test]
fn raw_arguments_preserve_original_stream_after_canonical_repair_and_replay() {
	let directory = tempfile::tempdir().expect("directory");
	let path = directory.path().join("raw-arguments.oms");
	let mut session = Session::create(&path, ComponentRegistry::default()).expect("session");
	session.begin_turn().expect("turn");
	session.user("invoke", Vec::new()).expect("user");
	let (call, sid) = session
		.call_streaming("test", 1, "call", None)
		.expect("call");
	let event = (session.entry_count() - 1) as u64;
	session.stream_append(sid, "{key:").expect("raw chunk");
	session.stream_append(sid, " 'value'}").expect("raw chunk");
	session.stream_close(sid).expect("stream closes");
	session
		.call_ready(
			call,
			serde_json::value::to_raw_value(&serde_json::json!({"key": "value"})).expect("canonical"),
		)
		.expect("ready");
	session
		.settle(call, serde_json::value::to_raw_value(&serde_json::json!({})).expect("result"))
		.expect("settle");
	let id = format!("{call}:0");
	assert_eq!(
		session.context_raw_args(&id, event, 1).expect("raw"),
		Some(b"{key: 'value'}".to_vec())
	);
	assert!(matches!(
		session.context_item(&id, event + 1, 1),
		Err(ContextPinError::ContextGone { .. })
	));
	assert!(matches!(session.context_item(&id, event, 2), Err(ContextPinError::ContextGone { .. })));
	drop(session);
	let session = Session::open(&path, ComponentRegistry::default()).expect("replay");
	assert_eq!(
		session
			.context_raw_args(&id, event, 1)
			.expect("replayed raw"),
		Some(b"{key: 'value'}".to_vec())
	);
}

#[test]
fn pins_replay_and_enforce_owner_and_compaction_boundaries() {
	let directory = tempfile::tempdir().expect("directory");
	let path = directory.path().join("pins.oms");
	let mut session = Session::create(&path, ComponentRegistry::default()).expect("session");
	let turn = session.begin_turn().expect("turn");
	let first = session.user("protected", Vec::new()).expect("first");
	let first_id = Str::new(format!("{first}:0"));
	let first_event = session.entry_count() - 1;
	let second = session.user("other owner", Vec::new()).expect("second");
	let second_id = Str::new(format!("{second}:0"));
	assert_eq!(
		session
			.pin_context("a", &[(first_id.clone(), 3)], "retain", 10)
			.expect("pin"),
		1
	);
	let projection = omp_session::project_thread(session.dom());
	let origin = session
		.context_origin(&projection[0])
		.expect("origin")
		.expect("durable origin");
	assert_eq!(origin.id, first_id);
	assert_eq!(origin.event, first_event as u64);
	assert_eq!(origin.turn_id.as_deref(), Some(turn.to_string().as_str()));
	assert!(origin.pinned);
	assert_eq!(
		session
			.pin_context("b", &[(second_id.clone(), 3)], "retain", 10)
			.expect("pin"),
		1
	);
	let accepted = session.head();
	assert!(matches!(
		session.unpin_context("a", &[first_id.clone(), second_id.clone()]),
		Err(ContextPinError::PermissionDenied { .. })
	));
	assert_eq!(session.head(), accepted, "mixed-owner request is atomic");
	assert_eq!(context_pins(session.dom()).expect("pins").len(), 2);
	let summary = session.blobs().put(b"summary").expect("summary");
	assert!(matches!(
		session.compaction(Compaction::new(summary.clone(), first)),
		Err(SessionError::PinnedContext { .. })
	));
	assert_eq!(session.head(), accepted, "rejected compaction never appends");
	drop(session);
	let mut session = Session::open(&path, ComponentRegistry::default()).expect("replay");
	assert_eq!(context_pins(session.dom()).expect("replayed pins").len(), 2);
	assert_eq!(
		session
			.unpin_context("a", &[first_id])
			.expect("owner release"),
		1
	);
	session
		.compaction(Compaction::new(summary, first))
		.expect("only unpinned prefix removed");
	assert_eq!(
		context_pins(session.dom()).expect("remaining pin")[&second_id]
			.owner
			.as_str(),
		"b"
	);
}

#[test]
fn pins_reject_stale_ids_and_overflow_without_journaling() {
	let directory = tempfile::tempdir().expect("directory");
	let mut session =
		Session::create(directory.path().join("budget.oms"), ComponentRegistry::default())
			.expect("session");
	session.begin_turn().expect("turn");
	let first = session.user("first", Vec::new()).expect("first");
	let second = session.user("second", Vec::new()).expect("second");
	let first_id = Str::new(format!("{first}:0"));
	let second_id = Str::new(format!("{second}:0"));
	let head = session.head();
	assert!(matches!(
		session.pin_context("a", &[(Str::new_static("missing"), 1)], "retain", 10),
		Err(ContextPinError::ContextGone { .. })
	));
	assert!(matches!(
		session.pin_context("a", &[(first_id.clone(), u64::MAX), (second_id, 1)], "retain", u64::MAX),
		Err(ContextPinError::BudgetExceeded)
	));
	assert_eq!(session.head(), head);
	assert!(context_pins(session.dom()).expect("pins").is_empty());
	assert_eq!(
		session
			.pin_context("a", &[(first_id.clone(), 2), (first_id, 2)], "retain", 2)
			.expect("duplicate id"),
		1
	);
	session.rewind(head.expect("head")).expect("rewind");
	assert!(
		context_pins(session.dom())
			.expect("rewound pins")
			.is_empty()
	);
}

#[test]
fn pin_acknowledgement_replays_without_appending_and_rejects_reused_arguments() {
	use omp_session::context::ContextRequestKey;
	let directory = tempfile::tempdir().expect("directory");
	let path = directory.path().join("receipts.oms");
	let mut session = Session::create(&path, ComponentRegistry::default()).expect("session");
	session.begin_turn().expect("turn");
	let entry = session
		.user("retained history", Vec::new())
		.expect("history");
	let items = [(Str::new(format!("{entry}:0")), 8)];
	let request = ContextRequestKey {
		owner:       Str::new_static("owner"),
		key:         Str::new_static("key"),
		fingerprint: Str::new_static("pin-arguments"),
	};
	assert_eq!(
		session
			.pin_context_request("owner", &items, "keep", 100, Some(&request))
			.expect("pin"),
		1
	);
	let accepted = session.head();
	drop(session);
	let mut session = Session::open(&path, ComponentRegistry::default()).expect("reopen");
	assert_eq!(
		session
			.pin_context_request("owner", &items, "keep", 100, Some(&request))
			.expect("replay"),
		1,
		"return original acknowledgement, not zero newly added pins"
	);
	assert_eq!(session.head(), accepted, "retry does not append");
	let conflict =
		ContextRequestKey { fingerprint: Str::new_static("different-arguments"), ..request };
	assert!(matches!(
		session.pin_context_request("owner", &items, "changed", 100, Some(&conflict)),
		Err(ContextPinError::IdempotencyConflict)
	));
	assert_eq!(session.head(), accepted);
}

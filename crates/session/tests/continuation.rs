//! Settlement continuation payloads and budget survive journal replay.

use omp_core::Str;
use omp_session::{
	ComponentRegistry, Session,
	continuation::{Continuation, ContinuationRole, DEFAULT_CONTINUATION_CAP},
};

fn continuation(prompt: &str) -> Continuation {
	Continuation {
		owner:          Str::new_static("verified-owner"),
		prompt:         Str::new(prompt),
		visible:        false,
		role:           ContinuationRole::System,
		label:          Some(Str::new_static("goal")),
		collapse_prior: true,
	}
}

#[test]
fn settlement_continuations_keep_exact_payload_collapse_and_durable_cap() {
	let directory = tempfile::tempdir().expect("directory");
	let path = directory.path().join("continuations.oms");
	let mut session = Session::create(&path, ComponentRegistry::default()).expect("session");
	session.begin_turn().expect("turn");
	session.user("actual user", Vec::new()).expect("user");
	for index in 0..DEFAULT_CONTINUATION_CAP {
		assert!(
			session
				.continue_from_settlement(
					&continuation(&format!("continue {index}")),
					DEFAULT_CONTINUATION_CAP
				)
				.expect("continue")
		);
	}
	let projection = omp_session::project_thread(session.dom());
	assert_eq!(projection.len(), 2, "only user and newest continuation remain runnable");
	let origin = session
		.context_origin(&projection[1])
		.expect("origin")
		.expect("durable continuation id");
	assert!(origin.id.ends_with(":0"));
	drop(session);
	let mut session = Session::open(&path, ComponentRegistry::default()).expect("reopen");
	assert!(
		!session
			.continue_from_settlement(&continuation("over cap"), DEFAULT_CONTINUATION_CAP)
			.expect("refusal")
	);
	assert_eq!(
		omp_session::project_thread(session.dom()),
		projection,
		"refusal does not change model context"
	);
	assert_eq!(session.dom().count("notice").expect("notices"), 1);
	session
		.user("new actual user", Vec::new())
		.expect("user reset");
	assert!(
		session
			.continue_from_settlement(&continuation("new continuation"), DEFAULT_CONTINUATION_CAP)
			.expect("new user resets cap")
	);
}

#[test]
fn configured_cap_is_owner_scoped_non_widening_and_replay_stable() {
	let directory = tempfile::tempdir().expect("directory");
	let path = directory.path().join("limits.oms");
	let mut session = Session::create(&path, ComponentRegistry::default()).expect("session");
	session.begin_turn().expect("turn");
	session.user("external user", Vec::new()).expect("user");
	let request = continuation("continue");
	for _ in 0..2 {
		assert!(
			session
				.continue_from_settlement(&request, 2)
				.expect("allowed")
		);
	}
	assert!(
		!session
			.continue_from_settlement(&request, 2)
			.expect("exhausted")
	);
	let mut other = request.clone();
	other.owner = Str::new_static("other authenticated owner");
	assert!(
		session
			.continue_from_settlement(&other, 1)
			.expect("independent owner")
	);
	drop(session);
	let mut session = Session::open(&path, ComponentRegistry::default()).expect("replay");
	assert!(
		!session
			.continue_from_settlement(&request, 20)
			.expect("raise cannot replenish")
	);
	session
		.user("new external user", Vec::new())
		.expect("new scope");
	for _ in 0..12 {
		assert!(
			session
				.continue_from_settlement(&request, 12)
				.expect("above eight")
		);
	}
	assert!(
		!session
			.continue_from_settlement(&request, 12)
			.expect("new cap")
	);
	session.user("tightening", Vec::new()).expect("new scope");
	assert!(
		session
			.continue_from_settlement(&request, 12)
			.expect("first")
	);
	assert!(
		!session
			.continue_from_settlement(&request, 1)
			.expect("tighten now")
	);
	assert!(
		!session
			.continue_from_settlement(&request, 12)
			.expect("cannot undo tightening")
	);
	let before = session.entry_count();
	assert!(matches!(
		session.continue_from_settlement(&request, 1025),
		Err(omp_session::SessionError::InvalidContinuationLimit { limit: 1025 })
	));
	assert_eq!(session.entry_count(), before, "invalid configuration has no journal effects");
}

#[test]
fn synthetic_user_projection_cannot_reset_an_exhausted_ledger() {
	use omp_dom::{KnownTag, NodeSpec, Op, PropId, Txn, Value};
	let directory = tempfile::tempdir().expect("directory");
	let mut session =
		Session::create(directory.path().join("synthetic.oms"), ComponentRegistry::default())
			.expect("session");
	session.begin_turn().expect("turn");
	session.user("external user", Vec::new()).expect("user");
	let request = continuation("continue");
	assert!(
		session
			.continue_from_settlement(&request, 1)
			.expect("first")
	);
	let turn = *session
		.dom()
		.children(session.dom().body())
		.last()
		.expect("turn");
	session
		.patch(Txn {
			cause: session.head().expect("head"),
			label: None,
			ops:   vec![Op::Ins {
				parent: turn,
				after:  session.dom().children(turn).last().copied(),
				node:   NodeSpec::new(KnownTag::User)
					.with_content("synthetic user")
					.with_prop(PropId::Id, Value::Str(Str::new_static("synthetic-user-id"))),
			}],
		})
		.expect("synthetic patch");
	assert!(
		!session
			.continue_from_settlement(&request, 20)
			.expect("patch cannot reset")
	);
}

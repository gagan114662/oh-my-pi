//! Settlement continuation payloads and budget survive journal replay.

use omp_core::Str;
use omp_session::{
	ComponentRegistry, Session,
	continuation::{CONTINUATION_CAP, Continuation, ContinuationRole},
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
	for index in 0..CONTINUATION_CAP {
		assert!(
			session
				.continue_from_settlement(&continuation(&format!("continue {index}")))
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
			.continue_from_settlement(&continuation("over cap"))
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
			.continue_from_settlement(&continuation("new continuation"))
			.expect("new user resets cap")
	);
}

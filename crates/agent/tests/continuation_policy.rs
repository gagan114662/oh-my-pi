//! Host policy composition narrows authenticated owner budgets before
//! journaling.

use omp_agent::{AI_CONTINUATION_CAP, AI_CONTINUATION_OWNER_LIMITS, continuation_cap};
use omp_con::{Ctx, Kv, SetSource, Value};
use omp_core::Str;
use omp_session::{
	ComponentRegistry, Session,
	continuation::{Continuation, ContinuationRole},
};

#[test]
fn configured_owner_limit_is_inherited_and_enforced_by_session() {
	let parent = Ctx::new();
	AI_CONTINUATION_CAP.set(&parent, 20).expect("host ceiling");
	AI_CONTINUATION_OWNER_LIMITS
		.set(&parent, Kv(vec![(Str::new_static("owner"), Value::Int(2))]))
		.expect("owner policy");
	let child = Ctx::new();
	for (name, value) in parent.seed_child().iter() {
		child
			.set_value(name, value.clone(), SetSource::Code)
			.expect("inherit host policy");
	}
	assert_eq!(continuation_cap(Some(&child), "other"), 20);
	assert_eq!(continuation_cap(Some(&child), "owner"), 2);
	let directory = tempfile::tempdir().expect("directory");
	let mut session =
		Session::create(directory.path().join("policy.oms"), ComponentRegistry::default())
			.expect("session");
	session.begin_turn().expect("turn");
	session.user("external input", Vec::new()).expect("user");
	let request = Continuation {
		owner:          Str::new_static("owner"),
		prompt:         Str::new_static("continue"),
		visible:        false,
		role:           ContinuationRole::System,
		label:          None,
		collapse_prior: true,
	};
	for accepted in [true, true, false] {
		assert_eq!(
			session
				.continue_from_settlement(&request, continuation_cap(Some(&child), &request.owner))
				.expect("settlement"),
			accepted
		);
	}
	AI_CONTINUATION_CAP.set(&child, 30).expect("new ceiling");
	AI_CONTINUATION_OWNER_LIMITS
		.set(&child, Kv::new())
		.expect("remove narrower policy");
	assert!(
		!session
			.continue_from_settlement(&request, continuation_cap(Some(&child), &request.owner))
			.expect("live scope remains exhausted")
	);
}

#[test]
fn invalid_or_duplicate_owner_limits_cannot_widen_allowance() {
	let con = Ctx::new();
	assert_eq!(continuation_cap(None, "owner"), 8);
	AI_CONTINUATION_CAP.set(&con, 20).expect("host ceiling");
	for (values, expected) in [
		(vec![Value::Int(30)], 20),
		(vec![Value::Int(9), Value::Int(2)], 2),
		(vec![Value::Int(-1)], 0),
		(vec![Value::Int(1025)], 0),
		(vec![Value::Str(Str::new_static("2"))], 0),
	] {
		AI_CONTINUATION_OWNER_LIMITS
			.set(
				&con,
				Kv(values
					.into_iter()
					.map(|value| (Str::new_static("owner"), value))
					.collect()),
			)
			.expect("policy");
		assert_eq!(continuation_cap(Some(&con), "owner"), expected);
	}
	AI_CONTINUATION_CAP.set(&con, 0).expect("disable");
	assert_eq!(continuation_cap(Some(&con), "other"), 0);
}

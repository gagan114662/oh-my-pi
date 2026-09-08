//! Turns are bounded by default and dead loops are refused (#124): the
//! kernel intersects caller request budgets and deadlines with its runtime
//! flags, journals a `turn-limit` notice when one is crossed, refuses the
//! next identical tool round after `loop_guard_limit` identical executions,
//! and settles the turn after as many refusals.

use std::time::Duration;

use omp_agent::{
	DispatchPolicy, Kernel, RunControl, RuntimeFlags, StaticPrompt, TurnInput, TurnStop,
};
use omp_core::Str;
use omp_dom::{PropKey, Value};
use omp_journal::blob::BlobStore;
use omp_session::Session;

mod support;

use support::{
	ScriptedInference, fresh_session, journal_entries, registry, spec, text_script, tool_script,
};

fn input(text: &str) -> TurnInput {
	TurnInput { text: Str::new(text), attachments: Vec::new() }
}

fn policy(directory: &std::path::Path) -> DispatchPolicy {
	DispatchPolicy::new(BlobStore::open(directory.join("blobs")).expect("blob store opens"))
}

fn notices(session: &Session, name: &str) -> Vec<(Str, Str)> {
	session
		.dom()
		.select("body turn notice")
		.expect("selector parses")
		.filter_map(|handle| {
			let node = session.dom().get(handle)?;
			let named = node
				.prop(&PropKey::Custom(Str::new_static("name")))
				.and_then(Value::as_str)
				== Some(name);
			named.then(|| {
				(
					node
						.prop(&PropKey::from(omp_dom::PropId::Kind))
						.and_then(Value::as_str)
						.map(Str::new)
						.unwrap_or_default(),
					node.content.clone().unwrap_or_default(),
				)
			})
		})
		.collect()
}

/// Committed `tool.result@1` entries, i.e. calls that reached a terminal
/// outcome (executed or aborted); the payload says which.
fn tool_results(path: &std::path::Path) -> (usize, usize) {
	let mut executed = 0;
	let mut aborted = 0;
	for entry in journal_entries(path) {
		if entry.kind.name.as_str() != "tool.result" {
			continue;
		}
		if entry.data.contains("\"abort\"") || entry.data.contains("loop guard") {
			aborted += 1;
		} else {
			executed += 1;
		}
	}
	(executed, aborted)
}

#[tokio::test]
async fn identical_tool_rounds_are_refused_at_the_limit_and_the_turn_settles_at_twice_it() {
	let directory = tempfile::tempdir().expect("temporary directory");
	let journal_path = directory.path().join("loop.oms");
	// The model issues the same call with the same arguments forever and
	// never produces text; the tool always answers the same thing.
	let scripts = (0..40)
		.map(|_| tool_script("same", "echo", serde_json::json!({"probe": 1})))
		.chain(std::iter::once(text_script("never reached")));
	let (inference, requests) = ScriptedInference::new(scripts);
	let mut kernel = Kernel::new(
		inference,
		registry([spec("echo", 1, "unchanged")]),
		policy(directory.path()),
		StaticPrompt(Str::new_static("test system")),
	)
	.with_runtime_flags(RuntimeFlags { loop_guard_limit: 3, ..RuntimeFlags::default() });
	let mut session = fresh_session(&journal_path);

	let outcome = kernel
		.run_turn(&mut session, input("loop"), RunControl::new(Default::default(), None))
		.await
		.expect("the guard settles the turn instead of failing it");
	assert_eq!(outcome.stop, TurnStop::Completed);
	// limit executions, then limit refusals, then the turn settles.
	assert_eq!(requests.lock().len(), 6, "3 executed rounds + 3 refused rounds");
	let (executed, aborted) = tool_results(&journal_path);
	assert_eq!(executed, 3, "exactly loop_guard_limit identical executions");
	assert_eq!(aborted, 3, "every later identical call was refused, not run");
	let guard = notices(&session, "loop-guard");
	assert_eq!(guard.len(), 3, "{guard:?}");
	assert!(guard[..2].iter().all(|(kind, _)| kind.as_str() == "warn"), "{guard:?}");
	assert_eq!(guard[2].0.as_str(), "error", "{guard:?}");
	assert!(guard[2].1.contains("the turn stops here"), "{}", guard[2].1);
}

#[tokio::test]
async fn changing_arguments_or_results_is_progress_and_is_never_refused() {
	let directory = tempfile::tempdir().expect("temporary directory");
	let journal_path = directory.path().join("progress.oms");
	let scripts = (0..12)
		.map(|round| tool_script("step", "echo", serde_json::json!({"page": round})))
		.chain(std::iter::once(text_script("done")));
	let (inference, requests) = ScriptedInference::new(scripts);
	let mut kernel = Kernel::new(
		inference,
		registry([spec("echo", 1, "page")]),
		policy(directory.path()),
		StaticPrompt(Str::new_static("test system")),
	)
	.with_runtime_flags(RuntimeFlags { loop_guard_limit: 3, ..RuntimeFlags::default() });
	let mut session = fresh_session(&journal_path);

	let outcome = kernel
		.run_turn(&mut session, input("paginate"), RunControl::new(Default::default(), None))
		.await
		.expect("distinct calls complete normally");
	assert_eq!(outcome.stop, TurnStop::Completed);
	assert_eq!(outcome.assistant_text.as_str(), "done");
	assert_eq!(requests.lock().len(), 13);
	assert!(notices(&session, "loop-guard").is_empty());
	assert_eq!(tool_results(&journal_path), (12, 0));
}

#[tokio::test]
async fn kernel_request_cap_settles_a_turn_with_a_turn_limit_notice() {
	let directory = tempfile::tempdir().expect("temporary directory");
	let journal_path = directory.path().join("request-cap.oms");
	let scripts = (0..40)
		.map(|round| tool_script("step", "echo", serde_json::json!({"page": round})))
		.chain(std::iter::once(text_script("never reached")));
	let (inference, requests) = ScriptedInference::new(scripts);
	let mut kernel = Kernel::new(
		inference,
		registry([spec("echo", 1, "page")]),
		policy(directory.path()),
		StaticPrompt(Str::new_static("test system")),
	)
	.with_runtime_flags(RuntimeFlags {
		turn_max_requests: 4,
		loop_guard_limit: 0,
		..RuntimeFlags::default()
	});
	let mut session = fresh_session(&journal_path);

	let outcome = kernel
		.run_turn(&mut session, input("forever"), RunControl::new(Default::default(), None))
		.await
		.expect("the cap settles the turn");
	assert_eq!(outcome.stop, TurnStop::Completed);
	// The host cap counts every provider request; no extra wrap-up is sent.
	assert_eq!(requests.lock().len(), 4, "the fourth provider request exhausts the host cap");
	let limits = notices(&session, "turn-limit");
	assert_eq!(limits.len(), 1, "one terminal cap notice without another request: {limits:?}");
	assert!(limits.iter().all(|(kind, _)| kind.as_str() == "warn"), "{limits:?}");
	assert_eq!(
		limits[0].1.as_str(),
		"Turn request cap reached after 4 provider requests; the turn stops here"
	);
	assert!(
		notices(&session, "request-budget").is_empty(),
		"the kernel bound is not a subagent budget"
	);
}

#[tokio::test]
async fn caller_request_budget_is_not_loosened_by_the_kernel_default() {
	let directory = tempfile::tempdir().expect("temporary directory");
	let journal_path = directory.path().join("caller-budget.oms");
	let scripts = (0..40)
		.map(|round| tool_script("step", "echo", serde_json::json!({"page": round})))
		.chain(std::iter::once(text_script("never reached")));
	let (inference, requests) = ScriptedInference::new(scripts);
	let mut kernel = Kernel::new(
		inference,
		registry([spec("echo", 1, "page")]),
		policy(directory.path()),
		StaticPrompt(Str::new_static("test system")),
	)
	.with_runtime_flags(RuntimeFlags {
		turn_max_requests: 500,
		loop_guard_limit: 0,
		..RuntimeFlags::default()
	});
	let mut session = fresh_session(&journal_path);

	let control = RunControl::new(Default::default(), None).with_request_budget(2);
	kernel
		.run_turn(&mut session, input("forever"), control)
		.await
		.expect("the caller budget settles the turn");
	assert_eq!(requests.lock().len(), 3, "caller budget of 2 plus its wrap-up request");
	assert!(notices(&session, "turn-limit").is_empty());
	let limits = notices(&session, "request-budget");
	assert_eq!(limits.len(), 2, "one soft warning and one exhaustion notice: {limits:?}");
	assert!(limits.iter().all(|(kind, _)| kind.as_str() == "warn"), "{limits:?}");
	assert_eq!(
		limits[0].1.as_str(),
		"Soft request budget reached; use this final request to yield a concise result."
	);
	assert_eq!(limits[1].1.as_str(), "Subagent request budget exhausted before another inference");
}

#[tokio::test]
async fn kernel_wall_clock_cap_ends_a_turn_whose_tool_hangs() {
	let directory = tempfile::tempdir().expect("temporary directory");
	let journal_path = directory.path().join("wall-cap.oms");
	let scripts = (0..40)
		.map(|round| tool_script("step", "slow", serde_json::json!({"page": round})))
		.chain(std::iter::once(text_script("never reached")));
	let (inference, requests) = ScriptedInference::new(scripts);
	let mut kernel = Kernel::new(
		inference,
		registry([spec("slow", 1, "page").streaming("working", Duration::from_secs(30))]),
		policy(directory.path()),
		StaticPrompt(Str::new_static("test system")),
	)
	.with_runtime_flags(RuntimeFlags {
		turn_max_wall: Some(Duration::from_millis(300)),
		loop_guard_limit: 0,
		..RuntimeFlags::default()
	});
	let mut session = fresh_session(&journal_path);

	let started = std::time::Instant::now();
	let outcome = kernel
		.run_turn(&mut session, input("hang"), RunControl::new(Default::default(), None))
		.await
		.expect("the deadline cancels the turn cleanly");
	assert_eq!(outcome.stop, TurnStop::Cancelled);
	assert!(started.elapsed() < Duration::from_secs(10), "the 30 s tool was cut by the 300 ms cap");
	assert!(requests.lock().len() <= 2, "no request storm while the tool hangs");
	let limits = notices(&session, "turn-limit");
	assert_eq!(limits.len(), 1, "{limits:?}");
	assert!(limits[0].1.contains("wall-clock cap"), "{}", limits[0].1);
}

#[tokio::test]
async fn larger_caller_budget_cannot_escape_the_composed_host_cap() {
	let directory = tempfile::tempdir().expect("temporary directory");
	let journal_path = directory.path().join("caller-override.oms");
	let scripts =
		(0..20).map(|round| tool_script("step", "echo", serde_json::json!({"page": round})));
	let (inference, requests) = ScriptedInference::new(scripts);
	let mut kernel = Kernel::new(
		inference,
		registry([spec("echo", 1, "page")]),
		policy(directory.path()),
		StaticPrompt(Str::new_static("test system")),
	)
	.with_runtime_flags(RuntimeFlags {
		turn_max_requests: 2,
		loop_guard_limit: 0,
		..RuntimeFlags::default()
	});
	let mut session = fresh_session(&journal_path);
	// Try to widen a host-derived control after the factory returns it.
	let control = kernel
		.turn_control()
		.with_request_budget(900)
		.with_request_budget_notice(false);
	let outcome = kernel
		.run_turn(&mut session, input("continue"), control)
		.await
		.unwrap();
	assert_eq!(outcome.stop, TurnStop::Completed);
	assert_eq!(requests.lock().len(), 2, "the caller cannot raise the host request cap");
	assert_eq!(notices(&session, "turn-limit").len(), 1);
	assert!(notices(&session, "request-budget").is_empty());
}

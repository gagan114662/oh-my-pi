//! Context requests execute through the existing kernel mailbox and Session.

use std::sync::Arc;

use omp_agent::{
	DispatchPolicy, Kernel, RouteFacts, StaticPrompt, context::control::ContextControlLease,
};
use omp_core::sf;
use omp_journal::blob::BlobStore;
use omp_tool::Registry;
use serde_json::{Value, json};

mod support;
use support::{ScriptedInference, fresh_session};

#[tokio::test]
async fn idle_context_requests_observe_real_projection_and_revoke_before_commit() {
	let temp = tempfile::tempdir().expect("tempdir");
	let (inference, _) = ScriptedInference::new([]);
	let mut kernel = Kernel::new(
		inference,
		Arc::new(Registry::new()),
		DispatchPolicy::new(BlobStore::open(temp.path().join("blobs")).expect("blobs")),
		StaticPrompt(sf!("actual system prompt")),
	)
	.with_route_facts(RouteFacts { context_window: 1000, ..RouteFacts::default() });
	let mut session = fresh_session(&temp.path().join("context.oms"));
	session.begin_turn().expect("turn");
	let user = session.user("actual user text", Vec::new()).expect("user");
	let control = kernel.context_control();
	let receiver = kernel.idle_control_receiver();
	let pending = tokio::spawn({
		let control = control.clone();
		async move {
			control
				.request(
					sf!("owner"),
					sf!("omp.context.view"),
					Default::default(),
					ContextControlLease::admitted(),
				)
				.await
		}
	});
	kernel
		.handle_idle_control(&mut session, receiver.recv_async().await.expect("request"))
		.await
		.expect("actor");
	let view = pending.await.expect("task").expect("view");
	assert_eq!(view["messages"][0]["id"], format!("{user}:0"));
	assert_eq!(view["messages"][0]["preview"], "actual user text");
	assert_eq!(view["usage"]["context_window"], 1000);
	assert!(
		view["usage"]["prompt_head_tokens"]
			.as_u64()
			.expect("prompt tokens")
			> 0
	);
	let lease = ContextControlLease::admitted();
	let pending = tokio::spawn({
		let control = control.clone();
		let lease = lease.clone();
		async move {
			control
				.request(
					sf!("owner"),
					sf!("omp.context.pin"),
					json!({"ids": [format!("{user}:0")], "reason": "keep", "idempotency_key": "test"})
						.as_object()
						.expect("arguments")
						.clone(),
					lease,
				)
				.await
		}
	});
	let request = receiver.recv_async().await.expect("queued pin");
	let before = session.head();
	lease.revoke();
	kernel
		.handle_idle_control(&mut session, request)
		.await
		.expect("actor");
	assert_eq!(pending.await.expect("task").expect_err("revoked").code, "StaleGeneration");
	assert_eq!(session.head(), before, "revoked pin never appends");
	assert!(
		omp_session::context::context_pins(session.dom())
			.expect("pins")
			.is_empty()
	);
	let pending = tokio::spawn(async move {
		control
			.request(
				sf!("owner"),
				sf!("omp.context.pin"),
				json!({"ids": [format!("{user}:0")], "reason": "keep", "idempotency_key": "cancelled"})
					.as_object()
					.expect("arguments")
					.clone(),
				ContextControlLease::admitted(),
			)
			.await
	});
	let request = receiver.recv_async().await.expect("queued request");
	pending.abort();
	assert!(pending.await.expect_err("aborted task").is_cancelled());
	kernel
		.handle_idle_control(&mut session, request)
		.await
		.expect("actor");
	assert_eq!(session.head(), before);
	assert_ne!(view, Value::Null);
}

/// The compaction callback waits for an actor-owned view and optionally pins
/// its source history before releasing the summary gate. A borrowed-Session
/// implementation deadlocks here instead of reaching the final assertions.
async fn nested_compaction(pin_during_hook: bool, revoke_during_hook: bool) {
	use omp_agent::{GateDecision, HookGate, HookPhase, OnFailure, SourceRef, When};
	use omp_proto::toolhost::v1::HookEventId;
	let temp = tempfile::tempdir().expect("tempdir");
	let (gate, hooks) = HookGate::channel();
	let gate = Arc::new(gate);
	gate
		.subscribe("nested", [omp_agent::hooks::Subscription {
			host:       sf!("nested"),
			source:     SourceRef {
				layer:        0,
				publisher:    sf!("test"),
				extension_id: sf!("context"),
			},
			id:         1,
			event:      HookEventId::HookEventCompaction,
			phase:      HookPhase::Review,
			order:      0,
			on_failure: OnFailure::Defer,
			when:       When::default(),
		}])
		.expect("subscribe");
	let con = Arc::new(omp_con::Ctx::new());
	omp_ai::settings::AI_COMPACTION_KEEP_RECENT_TOKENS
		.set(&con, 0)
		.expect("keep none");
	let (inference, requests) =
		ScriptedInference::new([support::text_script("faithful history summary")]);
	let mut kernel = Kernel::new(
		inference,
		Arc::new(Registry::new()),
		DispatchPolicy::new(BlobStore::open(temp.path().join("blobs")).expect("blobs")),
		StaticPrompt(sf!("system")),
	)
	.with_route_facts(RouteFacts { context_window: 10000, ..RouteFacts::default() })
	.with_con_context(con)
	.with_hook_gate(gate.clone());
	let mut session = fresh_session(&temp.path().join("nested.oms"));
	session.begin_turn().expect("history turn");
	let history = session
		.user("history worth retaining", Vec::new())
		.expect("history");
	session.begin_turn().expect("current turn");
	session.user("continue", Vec::new()).expect("prompt");
	let before = session.head();
	let control = kernel.context_control();
	let lease = ContextControlLease::admitted();
	let callback = tokio::spawn({
		let control = control.clone();
		let lease = lease.clone();
		async move {
			let dispatch = hooks.recv_async().await.expect("compaction hook");
			assert_eq!(dispatch.event, HookEventId::HookEventCompaction);
			let view = control
				.request(
					sf!("owner"),
					sf!("omp.context.view"),
					Default::default(),
					ContextControlLease::admitted(),
				)
				.await
				.expect("nested view completes");
			assert_eq!(view["messages"][0]["id"], format!("{history}:0"));
			if pin_during_hook {
				let pin = control.request(sf!("owner"), sf!("omp.context.pin"),
					json!({"ids": [format!("{history}:0")], "reason": "needed by callback", "idempotency_key": "nested-pin"})
					.as_object().expect("args").clone(), ContextControlLease::admitted())
					.await.expect("nested pin completes");
				assert_eq!(pin, 1);
			}
			if revoke_during_hook {
				lease.revoke();
			}
			gate
				.answer(dispatch.dispatch_id, vec![(1, GateDecision::Allow)])
				.expect("answer");
		}
	});
	let pending = tokio::spawn(async move {
		control
			.request(
				sf!("owner"),
				sf!("omp.context.compact"),
				json!({"tier": "local", "idempotency_key": "compact-test"})
					.as_object()
					.expect("args")
					.clone(),
				lease,
			)
			.await
	});
	let receiver = kernel.idle_control_receiver();
	let message = receiver.recv_async().await.expect("compact queued");
	tokio::time::timeout(
		std::time::Duration::from_secs(5),
		kernel.handle_idle_control(&mut session, message),
	)
	.await
	.expect("nested hook must not deadlock")
	.expect("actor");
	callback.await.expect("callback");
	let result = pending.await.expect("request");
	let markers = session
		.dom()
		.select("compaction")
		.expect("selector")
		.count();
	if pin_during_hook || revoke_during_hook {
		assert!(
			result.is_err(),
			"final commit must reject revoked admission or newly protected history"
		);
		assert_eq!(markers, 0, "no compaction was journaled");
		if revoke_during_hook {
			assert_eq!(session.head(), before);
		}
		if pin_during_hook {
			assert!(
				omp_session::context::context_pins(session.dom())
					.expect("pins")
					.contains_key(&sf!("{history}:0")),
				"the nested pin remains durable"
			);
		}
	} else {
		let result = result.expect("compaction result");
		assert_eq!(result["epoch"], 1);
		assert_eq!(markers, 1);
	}
	assert_eq!(requests.lock().len(), 1, "real summary inference was called");
}

#[tokio::test]
async fn compaction_hook_can_await_live_context_view_without_deadlock() {
	nested_compaction(false, false).await;
}

#[tokio::test]
async fn pin_committed_inside_compaction_hook_blocks_the_final_append() {
	nested_compaction(true, false).await;
}

#[tokio::test]
async fn generation_revoked_inside_compaction_hook_blocks_the_final_append() {
	nested_compaction(false, true).await;
}

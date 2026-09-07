//! `thread_projection` gate around the request projection: allow, deny, and
//! transform, each observed on the request the inference client receives.

use std::sync::Arc;

use bytes::Bytes;
use omp_agent::{
	DispatchPolicy, GateDecision, HookGate, HookPatch, HookPhase, Kernel, OnFailure, RunControl,
	SourceRef, StaticPrompt, TurnInput, When,
};
use omp_ai::{ContentPart, Role};
use omp_core::sf;
use omp_journal::blob::BlobStore;
use omp_proto::toolhost::v1::HookEventId;
use omp_tool::Registry;
use parking_lot::Mutex;
use serde_json::Value;

mod support;

use support::{ScriptedInference, fresh_session, text_script};

fn subscription(phase: HookPhase) -> omp_agent::hooks::Subscription {
	omp_agent::hooks::Subscription {
		host: sf!("test"),
		source: SourceRef {
			layer:        0,
			publisher:    sf!("test"),
			extension_id: sf!("context"),
		},
		id: 1,
		event: HookEventId::HookEventThreadProjection,
		phase,
		order: 0,
		on_failure: OnFailure::Defer,
		when: When::default(),
	}
}

/// Runs one text turn after two history turns, answering every
/// `thread_projection` dispatch with `decide(view)`; returns the observed
/// views and the conversation texts the inference client received.
async fn projected(
	phase: HookPhase,
	decide: impl Fn(&Value) -> GateDecision + Send + 'static,
) -> (Vec<Value>, Vec<(Role, String)>) {
	let (gate, receiver) = HookGate::channel();
	let gate = Arc::new(gate);
	gate
		.subscribe("test", [subscription(phase)])
		.expect("subscription");
	let views = Arc::new(Mutex::new(Vec::new()));
	let responder = {
		let gate = Arc::clone(&gate);
		let views = Arc::clone(&views);
		tokio::spawn(async move {
			while let Ok(dispatch) = receiver.recv_async().await {
				if dispatch.event != HookEventId::HookEventThreadProjection {
					continue;
				}
				let separator = dispatch
					.payload
					.iter()
					.position(|byte| *byte == b'\n')
					.map_or(0, |at| at + 1);
				let view: Value =
					serde_json::from_slice(&dispatch.payload[separator..]).expect("view payload");
				views.lock().push(view.clone());
				gate
					.answer(dispatch.dispatch_id, vec![(1, decide(&view))])
					.expect("answer");
			}
		})
	};
	let temp = tempfile::tempdir().expect("tempdir");
	let (inference, requests) =
		ScriptedInference::new([text_script("one"), text_script("two"), text_script("three")]);
	let mut kernel = Kernel::new(
		inference,
		Arc::new(Registry::new()),
		DispatchPolicy::new(BlobStore::open(temp.path().join("blobs")).expect("blobs")),
		StaticPrompt(sf!("system")),
	)
	.with_hook_gate(Arc::clone(&gate));
	let mut session = fresh_session(&temp.path().join("projection.oms"));
	for text in ["first", "second", "third"] {
		kernel
			.run_turn(
				&mut session,
				TurnInput { text: sf!(text), attachments: Vec::new() },
				RunControl::default(),
			)
			.await
			.expect("turn");
	}
	responder.abort();
	let last = requests.lock().last().cloned().expect("third request");
	let texts = last
		.messages
		.iter()
		.filter(|message| message.role != Role::System)
		.map(|message| {
			let text = message
				.content
				.iter()
				.find_map(|part| match part {
					ContentPart::Text { text, .. } => Some(text.to_string()),
					_ => None,
				})
				.unwrap_or_default();
			(message.role, text)
		})
		.collect();
	let views = views.lock().clone();
	(views, texts)
}

#[tokio::test]
async fn allow_keeps_the_projection_and_view_matches_the_request() {
	let (views, texts) = projected(HookPhase::Review, |_| GateDecision::Allow).await;
	let view = views.last().expect("view for the third request");
	let refs = view["messages"].as_array().expect("message refs");
	assert_eq!(refs.len(), texts.len(), "one MessageRef per conversation message");
	let first_id = refs[0]["id"].as_str().expect("stable id");
	let (entry, ordinal) = first_id
		.rsplit_once(':')
		.expect("journal id and local ordinal");
	let entry: omp_journal::EntryId = entry.parse().expect("journal entry identity");
	assert_eq!(ordinal, "0");
	assert_eq!(refs[0]["created_at_ms"], entry.as_ulid().timestamp_ms());
	assert_eq!(
		refs[0]["id"], views[0]["messages"][0]["id"],
		"earlier item identity survives later turns"
	);
	assert_eq!(refs[0]["event"], views[0]["messages"][0]["event"]);
	assert_eq!(refs[0]["role"], "user");
	assert_eq!(refs[0]["preview"], "first");
	assert_eq!(refs[1]["role"], "assistant");
	assert!(view["usage"]["total_tokens"].as_u64().is_some());
	assert_eq!(view["prompt_hash"].as_str().map(str::len), Some(16));
	assert_eq!(texts, vec![
		(Role::User, "first".to_owned()),
		(Role::Assistant, "one".to_owned()),
		(Role::User, "second".to_owned()),
		(Role::Assistant, "two".to_owned()),
		(Role::User, "third".to_owned()),
	]);
}

#[tokio::test]
async fn deny_is_fail_open_and_inference_still_runs() {
	let (views, texts) =
		projected(HookPhase::Precheck, |_| GateDecision::Deny(sf!("no context for you"))).await;
	assert_eq!(views.len(), 3, "the gate ran before every request");
	assert_eq!(texts.len(), 5, "a denied domain reply leaves the projection as projected");
	assert_eq!(texts[4].1, "third");
}

#[tokio::test]
async fn transform_applies_the_context_patch_to_the_request_only() {
	let (_, texts) = projected(HookPhase::Transform, |view| {
		let mut effective = view.clone();
		// Prune the first exchange using the exact stable ids in the view and
		// pin a note right before the pending user turn.
		let refs = view["messages"].as_array().expect("refs");
		if refs.len() >= 2 {
			effective["prune"] = serde_json::json!([{"ids": [refs[0]["id"], refs[1]["id"]], "reason": "old", "keep_placeholder": true}]);
		}
		effective["insert"] = serde_json::json!([{
			"parts": [{"text": "remember the budget"}],
			"anchor": {"relation": "tail"},
			"role": "developer",
			"ephemeral": true,
		}]);
		effective["note"] = Value::String("trimmed".into());
		GateDecision::Modify(HookPatch {
			target: None,
			args:   Some(Bytes::from(serde_json::to_vec(&effective).expect("patch"))),
		})
	})
	.await;
	// The third view contains `first`, `one`, `second`, `two`, `third`.
	assert_eq!(texts, vec![
		(Role::User, "[context pruned: old]".to_owned()),
		(Role::User, "second".to_owned()),
		(Role::Assistant, "two".to_owned()),
		(Role::User, "third".to_owned()),
		(Role::Developer, "remember the budget".to_owned()),
	]);
}

#[tokio::test]
async fn transform_with_an_invalid_patch_is_rejected_atomically() {
	let (_, texts) = projected(HookPhase::Transform, |view| {
		let mut effective = view.clone();
		let refs = view["messages"].as_array().expect("refs");
		effective["prune"] = serde_json::json!([{"ids": [refs[0]["id"]]}]);
		effective["reorder"] = serde_json::json!([{"ids": [refs.last().expect("last ref")["id"]], "before": "missing-journal-item"}]);
		GateDecision::Modify(HookPatch {
			target: None,
			args:   Some(Bytes::from(serde_json::to_vec(&effective).expect("patch"))),
		})
	})
	.await;
	assert_eq!(texts.len(), 5, "an invalid patch applies nothing, not part of itself");
	assert_eq!(texts[0].1, "first");
}

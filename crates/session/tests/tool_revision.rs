//! Full tool revision provenance survives both admission paths and replay.

use omp_core::Str;
use omp_journal::{Journal, data::ToolCall};
use omp_proto::{inference::v1::value, thread::v1::item};
use omp_session::{ComponentRegistry, Session, project_thread};
use omp_tool::{Rev, TOOL_REV_PROP};

#[test]
fn full_revision_survives_complete_and_streaming_call_replay() {
	for (streaming, family) in [(false, "recorded"), (true, "recorded"), (false, ""), (true, "")] {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("revision.oms");
		let mut session = Session::create(&path, ComponentRegistry::default()).unwrap();
		session.begin_turn().unwrap();
		session.user("edit request", Vec::new()).unwrap();
		session
			.assistant_start("test-model", "test-provider", "test-route")
			.unwrap();
		let revision = Rev { family: Str::new_static(family), n: 7 };
		let args =
			serde_json::value::to_raw_value(&serde_json::json!({"input": "original"})).unwrap();
		let call = if streaming {
			let (call, sid) = session
				.call_streaming("edit", &revision, "call", None)
				.unwrap();
			session.stream_append(sid, args.get()).unwrap();
			session.call_ready(call, args).unwrap();
			call
		} else {
			session
				.call("edit", &revision, "call", None, Some(args), None)
				.unwrap()
		};
		session.assistant_end("tool_calls").unwrap();
		let verdict = serde_json::json!({"success": {"original": true}});
		session
			.settle(call, serde_json::value::to_raw_value(&verdict).unwrap())
			.unwrap();
		let live = project_thread(session.dom());
		drop(session);
		let bytes = std::fs::read(&path).unwrap();
		let sealed = Journal::verify_path(&path, None).unwrap();
		let entries = Journal::scan(&path).unwrap();
		assert_eq!(sealed.sealed_entries, entries.len());
		assert_eq!((sealed.legacy_entries, sealed.legacy_bytes, sealed.torn_tail_bytes), (0, 0, 0));
		assert!(sealed.tip.is_some());
		let recorded: ToolCall =
			serde_json::from_str(&entries.iter().find(|entry| entry.id == call).unwrap().data)
				.unwrap();
		assert_eq!(recorded.family.as_deref(), Some(family));
		assert_eq!(recorded.rev, 7);
		let reopened = Session::open(&path, ComponentRegistry::default()).unwrap();
		let projected = project_thread(reopened.dom());
		assert_eq!(projected, live);
		let pair: Vec<_> = projected
			.iter()
			.filter(|entry| {
				matches!(entry.kind, Some(item::Kind::ToolCall(_) | item::Kind::ToolResult(_)))
			})
			.collect();
		assert_eq!(pair.len(), 2);
		for entry in &pair {
			assert_eq!(
				entry.props.as_ref().unwrap().fields[TOOL_REV_PROP].kind,
				Some(value::Kind::String(revision.to_string()))
			);
		}
		let Some(item::Kind::ToolResult(result)) = pair[1].kind.as_ref() else {
			unreachable!()
		};
		assert!(result.details.is_some(), "durable verdict is available to revision lifts");
		assert_eq!(std::fs::read(&path).unwrap(), bytes);
		assert_eq!(Journal::verify_path(&path, sealed.tip).unwrap(), sealed);
	}
}

#[test]
fn historical_call_payload_does_not_invent_missing_family() {
	let payload: ToolCall =
		serde_json::from_str(r#"{"name":"edit","rev":1,"call_id":"old","args":{"edits":[]}}"#)
			.unwrap();
	assert_eq!(payload.family, None);
	assert_eq!(payload.rev, 1);
	let encoded = serde_json::to_value(payload).unwrap();
	assert!(encoded.get("family").is_none());
}

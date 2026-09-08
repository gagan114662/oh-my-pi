//! Streamed deltas are journaled in windows (#106): a burst of tokens that
//! arrives within the coalescing window is one `stream@1` entry, a delta
//! past the byte budget lands at once, and the assistant text is intact
//! either way.

use omp_agent::{DispatchPolicy, Kernel, RunControl, StaticPrompt, TurnInput};
use omp_ai::{BlockKind, ChatEvent, FinishReason};
use omp_core::Str;
use omp_dom::{PropId, PropKey, Value};
use omp_journal::blob::BlobStore;
use omp_session::Session;

mod support;

use support::{ScriptedInference, completed, fresh_session, journal_entries, registry};

fn input(text: &str) -> TurnInput {
	TurnInput { text: Str::new(text), attachments: Vec::new() }
}

fn kernel(inference: ScriptedInference, directory: &std::path::Path) -> Kernel<ScriptedInference> {
	Kernel::new(
		inference,
		registry(std::iter::empty()),
		DispatchPolicy::new(BlobStore::open(directory.join("blobs")).expect("blob store opens")),
		StaticPrompt(Str::new_static("test system")),
	)
}

/// One text block streamed as `deltas` pieces.
fn burst_script(deltas: &[&str]) -> Vec<ChatEvent> {
	let mut events = vec![ChatEvent::BlockStarted { index: 0, kind: BlockKind::Text }];
	events.extend(
		deltas
			.iter()
			.map(|delta| ChatEvent::TextDelta { index: 0, text: Str::new(delta) }),
	);
	events.push(completed(FinishReason::Stop, 1));
	events
}

fn stream_appends(path: &std::path::Path) -> usize {
	journal_entries(path)
		.into_iter()
		.filter(|entry| entry.kind.name.as_str() == "stream" && entry.data.contains("\"text\""))
		.count()
}

fn assistant_text(session: &Session) -> String {
	session
		.dom()
		.select("body turn assistant")
		.expect("selector")
		.filter_map(|handle| session.dom().get(handle))
		.filter_map(|node| {
			node
				.prop(&PropKey::from(PropId::Text))
				.and_then(Value::as_str)
				.map(str::to_owned)
		})
		.collect::<Vec<_>>()
		.join("")
}

#[tokio::test]
async fn a_burst_of_tokens_within_the_window_is_one_journal_entry() {
	let directory = tempfile::tempdir().expect("temporary directory");
	let journal_path = directory.path().join("burst.oms");
	let deltas: Vec<String> = (0..200).map(|index| format!("tok{index} ")).collect();
	let refs: Vec<&str> = deltas.iter().map(String::as_str).collect();
	let (inference, _) = ScriptedInference::new([burst_script(&refs)]);
	let mut kernel = kernel(inference, directory.path());
	let mut session = fresh_session(&journal_path);

	let outcome = kernel
		.run_turn(&mut session, input("stream"), RunControl::default())
		.await
		.expect("turn completes");
	let expected = deltas.concat();
	assert_eq!(outcome.assistant_text.as_str(), expected, "every delta reaches the assistant text");
	assert_eq!(assistant_text(&session), expected, "and the tree");
	let appends = stream_appends(&journal_path);
	assert!(
		appends <= 2,
		"200 deltas delivered at once must land as at most 2 stream entries, got {appends}"
	);
}

#[tokio::test]
async fn a_delta_past_the_byte_budget_lands_immediately() {
	let directory = tempfile::tempdir().expect("temporary directory");
	let journal_path = directory.path().join("bytes.oms");
	let big = "x".repeat(5000);
	let (inference, _) = ScriptedInference::new([burst_script(&["head ", big.as_str(), " tail"])]);
	let mut kernel = kernel(inference, directory.path());
	let mut session = fresh_session(&journal_path);

	let outcome = kernel
		.run_turn(&mut session, input("stream"), RunControl::default())
		.await
		.expect("turn completes");
	assert_eq!(outcome.assistant_text.len(), 5000 + "head ".len() + " tail".len());
	let appends = stream_appends(&journal_path);
	assert!((2..=3).contains(&appends), "the 5000-byte delta flushes on its own, got {appends}");
}

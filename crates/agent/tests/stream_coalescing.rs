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

struct StalledInference {
	started: Option<tokio::sync::oneshot::Sender<()>>,
	release: Option<tokio::sync::oneshot::Receiver<()>>,
	dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl omp_agent::Inference for StalledInference {
	fn chat(
		&mut self,
		_request: omp_ai::ChatRequest,
	) -> impl std::future::Future<Output = Result<omp_ai::ChatStream, omp_ai::Error>> + Send {
		use futures::StreamExt as _;
		let started = self.started.take().expect("one inference request");
		let release = self.release.take().expect("one release gate");
		let dropped = self.dropped.clone();
		std::future::ready(Ok(omp_ai::ChatStream::ordinary(Box::pin(async_stream::stream! {
			struct DropGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);
			impl Drop for DropGuard {
				fn drop(&mut self) { self.0.store(true, std::sync::atomic::Ordering::Release); }
			}
			let _guard = DropGuard(dropped);
			let mut initial = support::streaming(vec![
				ChatEvent::BlockStarted { index: 0, kind: BlockKind::Text },
				ChatEvent::TextDelta { index: 0, text: Str::new_static("stalled durable prefix") },
			]);
			while let Some(event) = initial.next().await { yield event; }
			started.send(()).expect("observer remains alive");
			if release.await.is_ok() {
				yield Ok(ChatEvent::TextDelta { index: 0, text: Str::new_static(" released suffix") });
				yield Ok(completed(FinishReason::Stop, 1));
			}
		}))))
	}
}

async fn stalled_prefix_is_durable_before_release(cancel: bool) {
	use std::{
		sync::{
			Arc,
			atomic::{AtomicBool, Ordering},
		},
		time::Duration,
	};
	let directory = tempfile::tempdir().expect("temporary directory");
	let path = directory.path().join("stalled.oms");
	let (started, observed) = tokio::sync::oneshot::channel();
	let (release, gate) = tokio::sync::oneshot::channel();
	let dropped = Arc::new(AtomicBool::new(false));
	let inference = StalledInference {
		started: Some(started),
		release: Some(gate),
		dropped: Arc::clone(&dropped),
	};
	let mut kernel = Kernel::new(
		inference,
		registry(std::iter::empty()),
		DispatchPolicy::new(BlobStore::open(directory.path().join("blobs")).unwrap()),
		StaticPrompt(Str::new_static("test system")),
	);
	let mut session = fresh_session(&path);
	let cancellation = tokio_util::sync::CancellationToken::new();
	let turn =
		kernel.run_turn(&mut session, input("stall"), RunControl::new(cancellation.clone(), None));
	let observer = async {
		tokio::time::timeout(Duration::from_secs(3), observed)
			.await
			.expect("provider started")
			.unwrap();
		let prefix = tokio::time::timeout(Duration::from_secs(1), async {
			loop {
				let bytes = std::fs::read(&path).expect("live journal exists");
				if String::from_utf8_lossy(&bytes).contains("stalled durable prefix") {
					break bytes;
				}
				tokio::time::sleep(Duration::from_millis(5)).await;
			}
		})
		.await
		.expect("idle stream flushes within the bound without another provider event");
		assert!(!String::from_utf8_lossy(&prefix).contains("released suffix"));
		assert_eq!(stream_appends(&path), 1, "exactly the buffered prefix was committed");
		if cancel {
			cancellation.cancel();
		} else {
			release.send(()).unwrap();
		}
		prefix
	};
	let (outcome, prefix) =
		tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(turn, observer) })
			.await
			.expect("stalled turn settles after release or cancellation");
	let outcome = outcome.expect("turn settles");
	assert!(dropped.load(Ordering::Acquire), "provider stream dropped on both paths");
	assert!(
		std::fs::read(&path).unwrap().starts_with(&prefix),
		"durable prefix remains append-only"
	);
	if cancel {
		assert_eq!(outcome.stop, omp_agent::TurnStop::Cancelled);
		assert!(!String::from_utf8_lossy(&std::fs::read(&path).unwrap()).contains("released suffix"));
	} else {
		assert_eq!(outcome.assistant_text.as_str(), "stalled durable prefix released suffix");
		assert_eq!(stream_appends(&path), 2, "suffix closes as a separate entry");
	}
}

#[tokio::test]
async fn stalled_stream_flushes_without_another_delta() {
	stalled_prefix_is_durable_before_release(false).await;
}

#[tokio::test]
async fn cancelling_after_idle_flush_retains_prefix_and_drops_provider_stream() {
	stalled_prefix_is_durable_before_release(true).await;
}

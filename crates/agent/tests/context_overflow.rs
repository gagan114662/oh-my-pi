//! A provider context overflow is recovered once per turn (#112): the kernel
//! journals the window it observed, compacts everything before the current
//! prompt, and retries. A second overflow in the same turn ends the turn
//! with an error notice instead of looping.

use std::{
	collections::VecDeque,
	sync::{Arc, Mutex},
};

use omp_agent::{
	DispatchPolicy, Inference, Kernel, KernelError, RunControl, StaticPrompt, TurnInput,
};
use omp_ai::{
	ChatRequest, ChatStream, Error, ErrorKind, ErrorPhase, ExecutionReceipt, RetryAction,
};
use omp_core::Str;
use omp_dom::{PropKey, Value};
use omp_journal::blob::BlobStore;
use omp_session::Session;

mod support;

use support::{fresh_session, journal_entries, registry, streaming, text_script};

fn overflow() -> Error {
	Error::new(
		ErrorKind::ContextOverflow,
		ErrorPhase::Handshake,
		RetryAction::Never,
		ExecutionReceipt::default(),
	)
	.status(Some(400))
}

/// Answers each request from a script: `Err` is a context overflow at the
/// handshake, `Ok(text)` streams a text reply.
struct Scripted {
	script:   VecDeque<Result<&'static str, ()>>,
	requests: Arc<Mutex<Vec<ChatRequest>>>,
}

impl Scripted {
	fn new(
		script: impl IntoIterator<Item = Result<&'static str, ()>>,
	) -> (Self, Arc<Mutex<Vec<ChatRequest>>>) {
		let requests = Arc::new(Mutex::new(Vec::new()));
		(Self { script: script.into_iter().collect(), requests: Arc::clone(&requests) }, requests)
	}
}

impl Inference for Scripted {
	fn chat(
		&mut self,
		request: ChatRequest,
	) -> impl Future<Output = Result<ChatStream, Error>> + Send {
		self.requests.lock().expect("requests").push(request);
		let next = self
			.script
			.pop_front()
			.expect("one script per inference request");
		std::future::ready(match next {
			Ok(text) => Ok(streaming(text_script(text))),
			Err(()) => Err(overflow()),
		})
	}
}

fn kernel<C: Inference>(inference: C, directory: &std::path::Path) -> Kernel<C> {
	Kernel::new(
		inference,
		registry(std::iter::empty()),
		DispatchPolicy::new(BlobStore::open(directory.join("blobs")).expect("blob store opens")),
		StaticPrompt(Str::new_static("test system")),
	)
}

fn input(text: &str) -> TurnInput {
	TurnInput { text: Str::new(text), attachments: Vec::new() }
}

fn notices(session: &Session, name: &str) -> Vec<Str> {
	session
		.dom()
		.select("body turn notice")
		.expect("selector parses")
		.filter(|handle| {
			session
				.dom()
				.get(*handle)
				.and_then(|node| node.prop(&PropKey::Custom(Str::new_static("name"))))
				.and_then(Value::as_str)
				== Some(name)
		})
		.filter_map(|handle| {
			session
				.dom()
				.get(handle)
				.and_then(|node| node.content.clone())
		})
		.collect()
}

fn compactions(path: &std::path::Path) -> Vec<Option<Str>> {
	journal_entries(path)
		.into_iter()
		.filter(|entry| entry.kind.name.as_str() == "compaction")
		.map(|entry| {
			serde_json::from_str::<serde_json::Value>(&entry.data)
				.ok()
				.and_then(|payload| payload.get("method")?.as_str().map(Str::new))
		})
		.collect()
}

#[tokio::test]
async fn first_overflow_compacts_journals_the_window_and_retries_once() {
	let directory = tempfile::tempdir().expect("temporary directory");
	let journal_path = directory.path().join("overflow.oms");
	// Turn 1 succeeds; turn 2 overflows, the summariser answers, the retry
	// answers.
	let (inference, requests) = Scripted::new([
		Ok("first reply"),
		Err(()),
		Ok("history summary"),
		Ok("done after compaction"),
	]);
	let mut kernel = kernel(inference, directory.path());
	let mut session = fresh_session(&journal_path);

	kernel
		.run_turn(&mut session, input("first"), RunControl::new(Default::default(), None))
		.await
		.expect("first turn completes");
	let outcome = kernel
		.run_turn(&mut session, input("second"), RunControl::new(Default::default(), None))
		.await
		.expect("the overflowing turn recovers");
	assert_eq!(outcome.assistant_text.as_str(), "done after compaction");

	assert_eq!(requests.lock().expect("requests").len(), 4, "reply, overflow, summary, retry");
	assert_eq!(
		compactions(&journal_path),
		vec![Some(Str::new_static("overflow"))],
		"exactly one compaction, journaled as the overflow method"
	);
	let observed = notices(&session, "context-window-observed");
	assert_eq!(observed.len(), 1, "{observed:?}");
	assert!(observed[0].starts_with("context window observed: "), "{}", observed[0]);
	assert!(observed[0].ends_with(" tokens"), "{}", observed[0]);
	let overflows = notices(&session, "context-overflow");
	assert_eq!(overflows.len(), 1, "{overflows:?}");
	assert!(overflows[0].contains("compacting and retrying once"), "{}", overflows[0]);

	// The retry carried the summary, not the first turn verbatim.
	let retry = &requests.lock().expect("requests")[3];
	let text = retry
		.messages
		.iter()
		.filter_map(|message| {
			message.content.iter().find_map(|part| match part {
				omp_ai::ContentPart::Text { text, .. } => Some(text.to_string()),
				_ => None,
			})
		})
		.collect::<Vec<_>>()
		.join("\n");
	assert!(text.contains("history summary"), "{text}");
	assert!(text.contains("second"), "the triggering prompt is kept verbatim: {text}");
	assert!(!text.contains("first reply"), "the first turn is hidden behind the summary: {text}");

	drop(session);
	let reopened =
		Session::open(&journal_path, omp_session::ComponentRegistry::default()).expect("replays");
	assert_eq!(notices(&reopened, "context-window-observed").len(), 1, "durable across resume");
}

#[tokio::test]
async fn second_overflow_in_the_same_turn_fails_the_turn_with_a_notice() {
	let directory = tempfile::tempdir().expect("temporary directory");
	let journal_path = directory.path().join("overflow-twice.oms");
	let (inference, requests) =
		Scripted::new([Ok("first reply"), Err(()), Ok("history summary"), Err(())]);
	let mut kernel = kernel(inference, directory.path());
	let mut session = fresh_session(&journal_path);

	kernel
		.run_turn(&mut session, input("first"), RunControl::new(Default::default(), None))
		.await
		.expect("first turn completes");
	let error = kernel
		.run_turn(&mut session, input("second"), RunControl::new(Default::default(), None))
		.await
		.expect_err("a second overflow after the forced compaction fails the turn");
	let KernelError::Inference(inference) = error else {
		panic!("expected the provider overflow to surface, got {error:?}");
	};
	assert_eq!(inference.kind, ErrorKind::ContextOverflow);

	assert_eq!(requests.lock().expect("requests").len(), 4, "reply, overflow, summary, overflow");
	assert_eq!(compactions(&journal_path).len(), 1, "no second forced compaction");
	let overflows = notices(&session, "context-overflow");
	assert_eq!(overflows.len(), 2, "{overflows:?}");
	assert!(overflows[1].contains("again after 1 overflow compaction"), "{}", overflows[1]);
}
